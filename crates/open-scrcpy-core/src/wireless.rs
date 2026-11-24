//! USB-to-wireless switch for Android 11+ devices.
//!
//! The flow probes the phone's Wi-Fi IPv4, restarts adbd in TCP mode, waits
//! for it to come back, and connects over the network. It is one-shot and
//! diagnostic: every step can be logged by the shell through `log`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::{paths, process, validate};

/// Pull the first dotted-quad after `src`/`inet` out of `ip` output. Skips
/// IPv6 candidates (`inet6 fe80::.../64` yields nothing valid), so the caller
/// always gets a wlan v4 address or `None`.
pub fn find_wlan_ipv4(text: &str) -> Option<String> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    for (i, tok) in tokens.iter().enumerate() {
        if *tok != "src" && *tok != "inet" {
            continue;
        }
        let Some(raw) = tokens.get(i + 1) else {
            continue;
        };
        let candidate = raw.split('/').next().unwrap_or("");
        let segs: Vec<&str> = candidate.split('.').collect();
        let is_v4 = segs.len() == 4
            && segs
                .iter()
                .all(|s| !s.is_empty() && s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit()));
        if is_v4 {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Switches the active USB device to wireless debugging. Errors that adb
/// reports are returned as `{ success: false, message }` to match the old
/// command contract; only validation/process failures are `Err`.
pub async fn goto_wireless(
    log: impl Fn(String),
    device: &str,
    custom_path: Option<&str>,
) -> Result<Value, String> {
    validate::device(device)?;
    let adb_path = paths::binary_path("adb", custom_path);
    log("[SYSTEM] Switching to wireless...".to_string());

    // The phone's Wi-Fi IPv4. `addr show wlan0` reads the Wi-Fi interface
    // directly, so a VPN tunnel on the phone cannot steal the answer the way
    // it does with the default-route source address; `ip route get` stays as
    // fallback (a few OEMs rename the interface).
    let mut ip = String::new();
    for probe in ["ip -f inet addr show wlan0", "ip route get 1.1.1.1"] {
        let mut cmd = process::create_command(&adb_path);
        cmd.args(["-s", device, "shell", probe]);
        let output = process::output_with_timeout(cmd, Duration::from_secs(5)).await?;
        if let Some(found) = find_wlan_ipv4(&String::from_utf8_lossy(&output.stdout)) {
            ip = found;
            break;
        }
    }
    if ip.is_empty() {
        log(
            "[SYSTEM] Could not read the Wi-Fi IP — is the phone actually on a Wi-Fi network?"
                .to_string(),
        );
        return Ok(json!({ "success": false, "message": "could not read the device Wi-Fi IPv4" }));
    }

    log(format!(
        "[SYSTEM] adbd restarting in TCP mode (Wi-Fi IP {}).",
        ip
    ));
    let mut cmd = process::create_command(&adb_path);
    cmd.args(["-s", device, "tcpip", "5555"]);
    let output = process::output_with_timeout(cmd, Duration::from_secs(5)).await?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        log(format!("[ADB ERROR] {}", err));
        return Ok(json!({
            "success": false,
            "message": if err.is_empty() { "adb tcpip failed".to_string() } else { err }
        }));
    }

    // adbd needs a beat to come back up before the connect can land.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let target = format!("{}:5555", ip);
    let mut cmd = process::create_command(&adb_path);
    cmd.args(["connect", &target]);
    let output = process::output_with_timeout(cmd, Duration::from_secs(7)).await?;
    let out_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !out_text.is_empty() {
        log(format!("[ADB] {}", out_text));
    }
    let success = output.status.success()
        && !out_text.contains("cannot connect")
        && !out_text.contains("failed");
    if success {
        log(format!(
            "[OK] Wireless ready at {} — you can unplug the cable.",
            target
        ));
    }
    Ok(json!({ "success": success, "ip": ip, "port": 5555, "message": out_text }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_scanning_past_a_trailing_src() {
        assert_eq!(
            find_wlan_ipv4("inet 10.0.0.5/24 brd 10.0.0.255 scope global wlan0 src"),
            Some("10.0.0.5".to_string())
        );
    }

    #[test]
    fn ignores_ipv6_and_junk() {
        assert_eq!(find_wlan_ipv4("inet fe80::1/64 src ::1"), None);
        assert_eq!(find_wlan_ipv4(""), None);
        assert_eq!(find_wlan_ipv4("src"), None);
    }
}
