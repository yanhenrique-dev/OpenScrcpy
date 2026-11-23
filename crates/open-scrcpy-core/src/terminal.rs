//! Terminal-panel argument parsing and bounded output.
//!
//! The panel accepts free-form `adb`/`scrcpy` commands. Parsing is deliberately
//! small and predictable (quotes, escapes, Windows paths), and output is capped
//! so a command that never exits cannot grow the Rust side without bound.

use std::time::Duration;

use serde_json::{json, Value};

use crate::{paths, process, validate};

/// Keep the first 256 KiB and mark the cut.
pub const MAX_TERMINAL_OUTPUT_BYTES: usize = 256 * 1024;

pub fn cap_terminal_output(mut text: String) -> String {
    if text.len() <= MAX_TERMINAL_OUTPUT_BYTES {
        return text;
    }
    let mut end = MAX_TERMINAL_OUTPUT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str("\n[output truncated]");
    text
}

pub fn terminal_output(bytes: &[u8], truncated: bool) -> String {
    let mut text = cap_terminal_output(String::from_utf8_lossy(bytes).to_string());
    if truncated && !text.ends_with("[output truncated]") {
        text.push_str("\n[output truncated]");
    }
    text
}

/// Splits a command line into arguments. Only `\"` is an escape inside double
/// quotes because a backslash is a path separator on Windows; single quotes
/// are literal. Unclosed quotes are an error, not a best-effort split.
pub fn split_args(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = input.chars();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            // Only \" is an escape: a backslash is a path separator on
            // Windows, so \\ must survive (a UNC path starts with two).
            Some('"') if c == '\\' && chars.clone().next() == Some('"') => {
                current.push(chars.next().unwrap());
            }
            // Inside single quotes is POSIX-style literal: no escapes, so a
            // Windows path keeps every backslash.
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            // Escapes cover separators and quotes only; a doubled backslash
            // stays literal so \\server\share paths survive unquoted.
            None if c == '\\'
                && matches!(
                    chars.clone().next(),
                    Some(n) if n.is_whitespace() || n == '"' || n == '\''
                ) =>
            {
                current.push(chars.next().unwrap());
            }
            None if c.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            None => current.push(c),
        }
    }
    if let Some(q) = quote {
        return Err(format!("Unclosed {} quote", q));
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

/// Runs a free-form `adb`/`scrcpy` command from the terminal panel. The
/// caller passes the active device so a serial is injected unless the command
/// already names one, matching the old Tauri command.
pub async fn run_command(device: Option<&str>, input: &str, custom_path: Option<&str>) -> Value {
    let mut parts = match split_args(input) {
        Ok(parts) => parts,
        Err(error) => return json!({ "success": false, "message": error }),
    };
    if parts.is_empty() {
        return json!({ "success": false, "message": "No command provided" });
    }

    let first_part = parts[0].to_lowercase();
    let is_scrcpy = first_part == "scrcpy";
    let is_adb = first_part == "adb";
    let binary_name = if is_scrcpy { "scrcpy" } else { "adb" };
    let exe_path = paths::binary_path(binary_name, custom_path);

    if is_adb || is_scrcpy {
        parts.remove(0);
    }

    let mut args = Vec::new();
    let has_serial_flag =
        parts.contains(&"-s".to_string()) || parts.contains(&"--serial".to_string());
    if !has_serial_flag {
        if let Some(device) = device.filter(|device| !device.is_empty()) {
            if let Err(error) = validate::device(device) {
                return json!({ "success": false, "message": error });
            }
            let is_global_adb = binary_name == "adb"
                && !parts.is_empty()
                && (parts[0] == "devices" || parts[0] == "connect" || parts[0] == "pair");
            if !is_global_adb {
                args.push("-s".to_string());
                args.push(device.to_string());
            }
        }
    }
    args.extend(parts);

    let mut command = process::create_command(&exe_path);
    command.args(&args);
    let output = process::output_with_timeout_capped(
        command,
        Duration::from_secs(30),
        MAX_TERMINAL_OUTPUT_BYTES,
    )
    .await;

    match output {
        Ok((output, stdout_truncated, stderr_truncated)) => json!({
            "success": output.status.success(),
            "binary": binary_name,
            "stdout": terminal_output(&output.stdout, stdout_truncated),
            "stderr": terminal_output(&output.stderr, stderr_truncated)
        }),
        Err(error) => json!({ "success": false, "message": error }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_keeps_small_output() {
        assert_eq!(cap_terminal_output("ok".to_string()), "ok");
    }

    #[test]
    fn cap_truncates_on_char_boundary() {
        // Two-byte chars: a naive byte cut would slice one in half and panic
        // on the next String operation.
        let text = "é".repeat(MAX_TERMINAL_OUTPUT_BYTES / 2 + 1);
        let capped = cap_terminal_output(text);
        assert!(capped.ends_with("[output truncated]"));
        assert!(capped.len() <= MAX_TERMINAL_OUTPUT_BYTES + "\n[output truncated]".len());
    }

    #[test]
    fn split_supports_single_quotes() {
        let args = split_args("push 'Minha Foto.jpg' /sdcard/").unwrap();
        assert_eq!(
            args,
            vec![
                "push".to_string(),
                "Minha Foto.jpg".to_string(),
                "/sdcard/".to_string()
            ]
        );
    }

    #[test]
    fn split_supports_escaped_spaces() {
        let args = split_args(r"push Minha\ Foto.jpg /sdcard/").unwrap();
        assert_eq!(
            args,
            vec![
                "push".to_string(),
                "Minha Foto.jpg".to_string(),
                "/sdcard/".to_string()
            ]
        );
    }

    #[test]
    fn split_keeps_windows_paths_inside_quotes() {
        let args = split_args(r#"push "C:\Users\me\foto.jpg" /sdcard/"#).unwrap();
        assert_eq!(
            args,
            vec![
                "push".to_string(),
                r"C:\Users\me\foto.jpg".to_string(),
                "/sdcard/".to_string()
            ]
        );
    }

    #[test]
    fn split_keeps_unc_paths() {
        let args = split_args(r"push \\server\share\foto.jpg /sdcard/").unwrap();
        assert_eq!(
            args,
            vec![
                "push".to_string(),
                r"\\server\share\foto.jpg".to_string(),
                "/sdcard/".to_string()
            ]
        );
    }

    #[test]
    fn split_rejects_unclosed_single_quote() {
        assert!(split_args("push 'Minha Foto.jpg").is_err());
    }
}
