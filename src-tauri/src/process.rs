use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// Runs `cmd` to completion. If it exceeds `limit`, the child is killed and a
/// timeout error is returned. `kill_on_drop` is what turns the cancelled wait
/// into a real SIGKILL instead of an orphan process.
pub async fn output_with_timeout(mut cmd: Command, limit: Duration) -> Result<Output, String> {
    cmd.kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to start process: {e}"))?;

    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("command timed out after {}s", limit.as_secs())),
    }
}

/// Like [`output_with_timeout`], but stops collecting each stream after
/// `max_bytes` and closes that pipe. This keeps a runaway command such as
/// `adb logcat` from buffering hundreds of MB before the caller can truncate
/// it. Returns `(output, stdout_truncated, stderr_truncated)`.
pub async fn output_with_timeout_capped(
    mut cmd: Command,
    limit: Duration,
    max_bytes: usize,
) -> Result<(Output, bool, bool), String> {
    cmd.kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start process: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture stderr".to_string())?;

    let work = async move {
        // Drain both pipes concurrently: reading one sequentially can deadlock
        // when the child fills the other pipe's buffer.
        let (stdout_result, stderr_result) = tokio::join!(
            read_capped(stdout, max_bytes),
            read_capped(stderr, max_bytes)
        );
        let (stdout, stdout_truncated) = stdout_result?;
        let (stderr, stderr_truncated) = stderr_result?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((
            Output {
                status,
                stdout,
                stderr,
            },
            stdout_truncated,
            stderr_truncated,
        ))
    };

    match tokio::time::timeout(limit, work).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("command timed out after {}s", limit.as_secs())),
    }
}

/// Reads at most `cap` bytes. If the stream still has data after that, the
/// reader is dropped so the child gets a closed pipe instead of filling RAM.
async fn read_capped<R: AsyncRead + Unpin>(
    mut reader: R,
    cap: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut out = Vec::with_capacity(cap.min(64 * 1024));
    let mut buf = [0u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buf).await?;
        if read == 0 {
            return Ok((out, false));
        }
        let remaining = cap.saturating_sub(out.len());
        if read > remaining {
            out.extend_from_slice(&buf[..remaining]);
            return Ok((out, true));
        }
        out.extend_from_slice(&buf[..read]);
        if out.len() >= cap {
            return Ok((out, true));
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;
    // The timeout tests below drive a POSIX shell and `kill`; they are not
    // portable to the Windows CI runner.
    #[cfg(unix)]
    use std::process::Stdio;
    #[cfg(unix)]
    use std::time::Duration;

    #[cfg(unix)]
    async fn pid_is_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_the_child() {
        let pid_file =
            std::env::temp_dir().join(format!("openscrcpy-timeout-{}", std::process::id()));
        let script = format!("echo $$ > {}; sleep 30", pid_file.display());
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &script]);

        // Long enough for `sh` to write the pid even on a loaded runner: at
        // 100ms the kill could win the race and the file never appeared.
        let err = output_with_timeout(cmd, Duration::from_secs(1))
            .await
            .expect_err("the command should time out");
        assert!(err.contains("timed out"));

        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("pid file")
            .trim()
            .parse()
            .expect("pid");
        let _ = std::fs::remove_file(&pid_file);

        // Drop triggers start_kill; give the kernel a moment to reap it.
        for _ in 0..20 {
            if !pid_is_alive(pid).await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("child {pid} survived the timeout");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn returns_output_when_command_finishes_in_time() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo ok"]);
        let out = output_with_timeout(cmd, Duration::from_secs(2))
            .await
            .expect("command should finish");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capped_output_keeps_small_output_intact() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf ok"]);
        let (out, stdout_truncated, stderr_truncated) =
            output_with_timeout_capped(cmd, Duration::from_secs(2), 1024)
                .await
                .expect("command should finish");
        assert_eq!(out.stdout, b"ok");
        assert!(!stdout_truncated);
        assert!(!stderr_truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capped_output_stops_at_the_limit() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "head -c 100000 /dev/zero"]);
        let (out, stdout_truncated, _) =
            output_with_timeout_capped(cmd, Duration::from_secs(2), 1024)
                .await
                .expect("command should finish");
        assert_eq!(out.stdout.len(), 1024);
        assert!(stdout_truncated);
    }
}
