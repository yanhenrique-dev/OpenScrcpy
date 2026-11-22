use crate::audio::{
    codec_failure_is_terminal, codec_failure_message, is_audio_capture_error, is_audio_codec_error,
    AudioErrorState, AUDIO_FALLBACK_CHAIN,
};
use crate::capabilities::Capabilities;
use crate::download;
use crate::process;
use crate::process::create_command;
#[cfg(target_os = "windows")]
use crate::process::CREATE_NO_WINDOW;
use crate::validate;
use crate::{ScrcpyState, SessionHandle, StopRequest};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
#[cfg(unix)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager, State, Window};
use tokio::io::{AsyncBufReadExt, BufReader};
#[cfg(all(test, unix))]
use tokio::process::Command as TokioCommand;
use tokio::sync::{oneshot, Notify};
use tokio::time::{timeout, Duration};

// Screen-off startup: the ADB wake must be quick, but a wireless device can
// take a little longer to report the new display state.
const ADB_WAKE_TIMEOUT: Duration = Duration::from_secs(5);
const DISPLAY_POWER_QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const DISPLAY_WAKE_SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
const DISPLAY_WAKE_POLL_INTERVAL: Duration = Duration::from_millis(150);

fn get_binary_path(binary_name: &str, custom_folder: Option<String>) -> String {
    let exe_ext = std::env::consts::EXE_EXTENSION;
    let binary_filename = if exe_ext.is_empty() {
        binary_name.to_string()
    } else {
        format!("{}.{}", binary_name, exe_ext)
    };

    // 1. Check custom folder if provided
    if let Some(folder) = custom_folder {
        if !folder.trim().is_empty() {
            let full_path = Path::new(&folder).join(&binary_filename);
            if full_path.exists() && full_path.is_file() {
                return full_path.to_string_lossy().to_string();
            }
        }
    }

    // 2. Check local scrcpy-bin folder (relative to executable for portable/production)
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let local_bin = exe_dir.join("scrcpy-bin").join(&binary_filename);
            if local_bin.exists() && local_bin.is_file() {
                return local_bin.to_string_lossy().to_string();
            }
        }
    }

    // 3. Check the directory resolved at startup (app data when the exe
    // directory is read-only, e.g. /usr/bin or Program Files).
    if let Some(dir) = download::install_dir() {
        let installed = dir.join(&binary_filename);
        if installed.exists() && installed.is_file() {
            return installed.to_string_lossy().to_string();
        }
    }

    // 4. Fallback to current working directory scrcpy-bin
    if let Ok(current_dir) = std::env::current_dir() {
        let local_bin = current_dir.join("scrcpy-bin").join(&binary_filename);
        if local_bin.exists() && local_bin.is_file() {
            return local_bin.to_string_lossy().to_string();
        }
    }

    // 5. Return simple name to rely on system PATH
    binary_name.to_string()
}

/// Client for the GitHub release calls. The connect deadline is what keeps a
/// stalled TCP/TLS handshake from hanging the download or update UI forever;
/// individual requests still set their own total timeout.
fn http_client(user_agent: &str) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_scrcpy(custom_path: Option<String>) -> serde_json::Value {
    let exe_path = get_binary_path("scrcpy", custom_path);

    // Check version to verify it exists and is runnable
    let mut cmd = create_command(&exe_path);
    cmd.arg("--version");
    let output = process::output_with_timeout(cmd, Duration::from_secs(5)).await;

    match output {
        Ok(o) if o.status.success() => {
            json!({ "found": true, "message": "Scrcpy Ready" })
        }
        Ok(_) => {
            json!({ "found": false, "message": "Failed to start scrcpy (Exit Code != 0)" })
        }
        Err(_) => {
            json!({ "found": false, "message": "Scrcpy not found" })
        }
    }
}

/// ADB device states that can appear in the second column of `adb devices -l`
/// output. Only "device" means the device is connected and authorized; every
/// other state (offline, mid-pairing, unauthorized, ...) is not usable yet.
const KNOWN_ADB_STATES: &[&str] = &[
    "device",
    "offline",
    "unauthorized",
    "authorizing",
    "unknown",
    "bootloader",
    "recovery",
    "sideload",
    "host",
    "connecting",
];

/// Parses one data line of `adb devices -l` output into `(serial, state)`.
///
/// The serial and state are whitespace-separated, but the serial itself can
/// contain a space: an mDNS wireless-debugging serial in "conflict" form
/// (when two devices would otherwise resolve to the same mDNS name) looks
/// like `adb-SERIAL (2)._adb-tls-connect._tcp`. Splitting on the first
/// whitespace run, or assuming a specific serial shape, would misparse that
/// case. Instead this finds the first token that is a *known ADB state* and
/// treats everything before it as the serial, so it corresponds to the state
/// ADB actually reports, regardless of the serial's shape (USB ID, IP:port,
/// or mDNS name). Any further `-l` fields (`product:... model:...
/// transport_id:...`) are simply ignored.
fn parse_adb_device_line(line: &str) -> Option<(String, String)> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let state_idx = tokens.iter().position(|t| KNOWN_ADB_STATES.contains(t))?;
    if state_idx == 0 {
        return None; // no serial before the state
    }
    Some((tokens[..state_idx].join(" "), tokens[state_idx].to_string()))
}

/// Extracts the `model:` field from a `adb devices -l` line, if present, for
/// a more user-friendly device label than the raw serial (e.g. "SM S911B"
/// instead of a USB serial or IP:port). Adb replaces spaces in the model name
/// with underscores in this raw form; those are turned back into spaces,
/// which is as far as this goes without an extra `getprop` round-trip per
/// device -- still closer to a real name than a serial.
fn extract_device_model(line: &str) -> Option<String> {
    let model = line
        .split_whitespace()
        .find_map(|t| t.strip_prefix("model:"))?;
    if model.is_empty() {
        return None;
    }
    Some(model.replace('_', " "))
}

#[tauri::command]
pub async fn get_devices(custom_path: Option<String>) -> serde_json::Value {
    let adb_path = get_binary_path("adb", custom_path);

    let mut cmd = create_command(&adb_path);
    cmd.arg("devices").arg("-l");
    let output = process::output_with_timeout(cmd, Duration::from_secs(10)).await;

    match output {
        Ok(o) => {
            if o.status.success() {
                let out_str = String::from_utf8_lossy(&o.stdout);
                let mut devices: Vec<String> = Vec::new();
                let mut device_models = serde_json::Map::new();
                let mut unavailable = Vec::new();
                for line in out_str.lines().skip(1) {
                    // Skip "List of devices attached"
                    let Some((serial, state)) = parse_adb_device_line(line) else {
                        continue;
                    };
                    if state != "device" {
                        // An unauthorized phone is physically connected and
                        // showing the "Allow USB debugging?" prompt; dropping
                        // it here made a first run look like no device at all.
                        unavailable.push(json!({
                            "serial": serial,
                            "state": state,
                            "model": extract_device_model(line),
                        }));
                        continue;
                    }
                    if let Some(model) = extract_device_model(line) {
                        device_models.insert(serial.clone(), json!(model));
                    }
                    devices.push(serial);
                }

                json!({
                    "error": false,
                    "devices": devices,
                    "deviceModels": device_models,
                    "unavailable": unavailable,
                })
            } else {
                json!({ "error": true, "message": "ADB returned error" })
            }
        }
        Err(e) => {
            json!({ "error": true, "message": e.to_string() })
        }
    }
}

#[tauri::command]
pub async fn get_mdns_devices(custom_path: Option<String>) -> serde_json::Value {
    let adb_path = get_binary_path("adb", custom_path);

    let mut cmd = create_command(&adb_path);
    cmd.arg("mdns").arg("services");
    let output = process::output_with_timeout(cmd, Duration::from_secs(10)).await;

    match output {
        Ok(o) => {
            if o.status.success() {
                let out_str = String::from_utf8_lossy(&o.stdout);
                let mut services = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for line in out_str.lines().skip(1) {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 3 {
                        let name = parts[0].trim();
                        let service = parts[1].trim();
                        let address = parts[2].trim();
                        let key = format!("{}|{}|{}", name, service, address);
                        if !seen.contains(&key) {
                            services.push(json!({
                                "name": name,
                                "service": service,
                                "address": address
                            }));
                            seen.insert(key);
                        }
                    }
                }
                json!({ "error": false, "services": services })
            } else {
                json!({ "error": true, "message": "ADB mdns returned error" })
            }
        }
        Err(e) => {
            json!({ "error": true, "message": e.to_string() })
        }
    }
}

#[tauri::command]
pub async fn adb_connect(
    window: Window,
    ip: String,
    custom_path: Option<String>,
    silent: Option<bool>,
) -> Result<serde_json::Value, String> {
    validate::adb_address(&ip)?;
    let silent = silent.unwrap_or(false);
    let adb_path = get_binary_path("adb", custom_path);
    if !silent {
        let _ = window.emit(
            "scrcpy-log",
            format!("[SYSTEM] Attempting wireless connection to {}...", ip),
        );
    }

    let mut cmd = create_command(&adb_path);
    cmd.arg("connect").arg(&ip);
    let output_res = process::output_with_timeout(cmd, Duration::from_secs(5)).await;

    match output_res {
        Ok(output) => {
            let out_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let err_text = String::from_utf8_lossy(&output.stderr).trim().to_string();

            if !silent {
                // Log everything to terminal for visibility
                if !out_text.is_empty() {
                    let _ = window.emit("scrcpy-log", format!("[ADB] {}", out_text));
                }
                if !err_text.is_empty() {
                    let _ = window.emit("scrcpy-log", format!("[ADB ERROR] {}", err_text));
                }
            }

            let success = output.status.success()
                && !out_text.contains("cannot connect")
                && !out_text.contains("failed");
            Ok(
                json!({ "success": success, "message": if out_text.is_empty() { err_text } else { out_text } }),
            )
        }
        Err(e) => {
            if !silent {
                let _ = window.emit(
                    "scrcpy-log",
                    format!("[SYSTEM] Connection to {} failed: {}.", ip, e),
                );
            }
            Ok(json!({ "success": false, "message": e }))
        }
    }
}

#[tauri::command]
pub async fn adb_pair(
    window: Window,
    ip: String,
    code: String,
    custom_path: Option<String>,
) -> Result<serde_json::Value, String> {
    validate::adb_address(&ip)?;
    let adb_path = get_binary_path("adb", custom_path);
    let _ = window.emit("scrcpy-log", format!("[SYSTEM] Pairing with {}...", ip));

    let mut cmd = create_command(&adb_path);
    cmd.arg("pair").arg(&ip).arg(&code);
    let output = process::output_with_timeout(cmd, Duration::from_secs(10)).await?;

    let out_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let err_text = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !out_text.is_empty() {
        let _ = window.emit("scrcpy-log", format!("[ADB] {}", out_text));
    }
    if !err_text.is_empty() {
        let _ = window.emit("scrcpy-log", format!("[ADB ERROR] {}", err_text));
    }

    let success = output.status.success()
        && (out_text.contains("Successfully paired") || err_text.contains("Successfully paired"));

    Ok(
        json!({ "success": success, "message": if out_text.is_empty() { err_text } else { out_text } }),
    )
}

#[tauri::command]
pub async fn adb_shell(
    device: String,
    command: String,
    custom_path: Option<String>,
) -> serde_json::Value {
    if let Err(e) = validate::device(&device) {
        return json!({ "success": false, "message": e });
    }
    let adb_path = get_binary_path("adb", custom_path);

    let mut cmd = create_command(&adb_path);
    cmd.arg("-s").arg(&device).arg("shell").arg(&command);
    let output = process::output_with_timeout_capped(
        cmd,
        Duration::from_secs(10),
        MAX_TERMINAL_OUTPUT_BYTES,
    )
    .await;

    match output {
        Ok((o, stdout_truncated, _)) => {
            json!({
                "success": o.status.success(),
                "output": terminal_output(&o.stdout, stdout_truncated)
            })
        }
        Err(e) => json!({ "success": false, "message": e.to_string() }),
    }
}

/// The terminal panel renders whatever comes back from the command; a
/// runaway command (adb logcat never exits) must not grow the Rust side
/// without bound. Keep the first 256 KiB and mark the cut.
const MAX_TERMINAL_OUTPUT_BYTES: usize = 256 * 1024;

fn cap_terminal_output(mut text: String) -> String {
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

fn terminal_output(bytes: &[u8], truncated: bool) -> String {
    let mut text = cap_terminal_output(String::from_utf8_lossy(bytes).to_string());
    if truncated && !text.ends_with("[output truncated]") {
        text.push_str("\n[output truncated]");
    }
    text
}

#[tauri::command]
pub async fn run_terminal_command(
    device: Option<String>,
    cmd: String,
    custom_path: Option<String>,
) -> serde_json::Value {
    let mut parts = match split_args(&cmd) {
        Ok(parts) => parts,
        Err(e) => return json!({ "success": false, "message": e }),
    };
    if parts.is_empty() {
        return json!({ "success": false, "message": "No command provided" });
    }

    let first_part = parts[0].to_lowercase();
    let is_scrcpy = first_part == "scrcpy";
    let is_adb = first_part == "adb";

    let binary_name = if is_scrcpy { "scrcpy" } else { "adb" };
    let exe_path = get_binary_path(binary_name, custom_path);

    // If they explicitly typed "adb" or "scrcpy", remove it from arguments
    if is_adb || is_scrcpy {
        parts.remove(0);
    }

    let mut args = Vec::new();

    // Auto-inject device ID for ADB/Scrcpy commands if a device is active and not already specified
    let has_serial_flag =
        parts.contains(&"-s".to_string()) || parts.contains(&"--serial".to_string());

    if !has_serial_flag {
        if let Some(ref d) = device {
            if !d.is_empty() {
                if let Err(e) = validate::device(d) {
                    return json!({ "success": false, "message": e });
                }
                // For ADB, don't inject for certain global commands
                let is_global_adb = binary_name == "adb"
                    && !parts.is_empty()
                    && (parts[0] == "devices" || parts[0] == "connect" || parts[0] == "pair");

                if !is_global_adb {
                    args.push("-s".to_string());
                    args.push(d.clone());
                }
            }
        }
    }

    for part in parts {
        args.push(part);
    }

    let mut command = create_command(&exe_path);
    command.args(&args);
    let output = process::output_with_timeout_capped(
        command,
        Duration::from_secs(30),
        MAX_TERMINAL_OUTPUT_BYTES,
    )
    .await;

    match output {
        Ok((o, stdout_truncated, stderr_truncated)) => {
            json!({
                "success": o.status.success(),
                "binary": binary_name,
                "stdout": terminal_output(&o.stdout, stdout_truncated),
                "stderr": terminal_output(&o.stderr, stderr_truncated)
            })
        }
        Err(e) => json!({ "success": false, "message": e }),
    }
}

fn split_args(s: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = s.chars();

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

/// Allowlist for release asset file names (used by `download_scrcpy`).
/// Both the API path and the fallback construct this name, but the API path
/// echoes a server-provided string, so it is validated before downloading.
fn is_expected_asset_name(name: &str, extension: &str) -> bool {
    name.starts_with("scrcpy-")
        && name.ends_with(extension)
        && name.len() < 200
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Keep only the final path component of a frontend-provided file name, so a
/// crafted `name` (absolute path or `..` segments) cannot escape the target
/// directory (used by `save_report`).
fn safe_file_name(name: &str) -> Option<String> {
    std::path::Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
}

/// Sanitize an arbitrary identifier (e.g. a device id from user-typed IPs or
/// ADB output) for use inside a file name (used for the `--record` filename).
fn sanitize_filename_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() {
        "device".to_string()
    } else {
        trimmed.to_string()
    }
}

#[tauri::command]
pub async fn push_file(
    device: String,
    file_path: String,
    custom_path: Option<String>,
) -> serde_json::Value {
    if let Err(e) = validate::device(&device) {
        return json!({ "success": false, "message": e });
    }
    let adb_path = get_binary_path("adb", custom_path);

    let output = create_command(&adb_path)
        .arg("-s")
        .arg(&device)
        .arg("push")
        .arg(&file_path)
        .arg("/sdcard/Download/")
        .output()
        .await;

    match output {
        Ok(o) => {
            if o.status.success() {
                json!({ "success": true, "message": "File pushed to Downloads" })
            } else {
                // adb reports the real cause on stderr ("device offline",
                // "No space left"); collapsing every failure to one string
                // left the log unable to tell them apart.
                let stderr = String::from_utf8_lossy(&o.stderr);
                let stdout = String::from_utf8_lossy(&o.stdout);
                let detail = if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                };
                let message = if detail.is_empty() {
                    format!("Transfer failed (exit code {:?})", o.status.code())
                } else {
                    format!("Transfer failed: {}", detail)
                };
                json!({ "success": false, "message": message })
            }
        }
        Err(e) => json!({ "success": false, "message": e.to_string() }),
    }
}

#[tauri::command]
pub async fn install_apk(
    device: String,
    file_path: String,
    custom_path: Option<String>,
) -> serde_json::Value {
    if let Err(e) = validate::device(&device) {
        return json!({ "success": false, "message": e });
    }
    let adb_path = get_binary_path("adb", custom_path);

    let output = create_command(&adb_path)
        .arg("-s")
        .arg(&device)
        .arg("install")
        .arg(&file_path)
        .output()
        .await;

    match output {
        Ok(o) => {
            let out_text = String::from_utf8_lossy(&o.stdout);
            let err_text = String::from_utf8_lossy(&o.stderr);

            if o.status.success() {
                json!({ "success": true, "message": out_text.trim() })
            } else {
                json!({ "success": false, "message": err_text.trim() })
            }
        }
        Err(e) => json!({ "success": false, "message": e.to_string() }),
    }
}

#[tauri::command]
pub async fn kill_adb(
    window: Window,
    custom_path: Option<String>,
) -> Result<serde_json::Value, String> {
    let adb_path = get_binary_path("adb", custom_path);
    let _ = window.emit(
        "scrcpy-log",
        "[SYSTEM] Terminating ADB stack...".to_string(),
    );

    let mut cmd = create_command(&adb_path);
    cmd.arg("kill-server");
    match process::output_with_timeout(cmd, Duration::from_secs(5)).await {
        Ok(output) if output.status.success() => {
            let _ = window.emit("scrcpy-log", "[SYSTEM] ADB Stack Terminated.".to_string());
            Ok(json!({ "success": true, "message": "ADB Stack Terminated" }))
        }
        Ok(output) => {
            let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if err.is_empty() {
                "adb kill-server failed".to_string()
            } else {
                err
            };
            let _ = window.emit("scrcpy-log", format!("[ADB ERROR] {message}"));
            Ok(json!({ "success": false, "message": message }))
        }
        Err(e) => Ok(json!({ "success": false, "message": e })),
    }
}

#[tauri::command]
pub async fn list_scrcpy_options(
    device: String,
    arg: String,
    custom_path: Option<String>,
) -> serde_json::Value {
    if let Err(e) = validate::device(&device) {
        return json!({ "success": false, "message": e });
    }
    let exe_path = get_binary_path("scrcpy", custom_path);

    let mut command = create_command(&exe_path);
    command.arg("-s").arg(&device).arg(&arg);

    let output = process::output_with_timeout(command, Duration::from_secs(15)).await;

    match output {
        Ok(o) => {
            let out_text = String::from_utf8_lossy(&o.stdout);
            let err_text = String::from_utf8_lossy(&o.stderr); // scrcpy often prints lists to stderr
            let combined = format!("{}{}", out_text, err_text);
            json!({ "success": o.status.success(), "output": combined })
        }
        Err(e) => json!({ "success": false, "message": e.to_string() }),
    }
}

fn detect_host_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

fn render_driver_label(driver: &str) -> &'static str {
    match driver {
        "direct3d" => "D3D11 (Direct3D)",
        "opengl" => "OpenGL",
        "opengles2" => "OpenGL ES 2",
        "opengles" => "OpenGL ES",
        "metal" => "Metal",
        "software" => "Software",
        "vulkan" => "Vulkan",
        _ => "Custom",
    }
}

fn is_driver_allowed_on_os(driver: &str, host_os: &str) -> bool {
    match host_os {
        "windows" => driver != "metal",
        "macos" => driver != "direct3d",
        "linux" => driver != "direct3d" && driver != "metal",
        _ => true,
    }
}

fn detect_render_drivers_from_help(help_output: &str) -> Vec<String> {
    let known = [
        "direct3d",
        "opengl",
        "opengles2",
        "opengles",
        "metal",
        "software",
        "vulkan",
    ];

    let lower = help_output.to_lowercase();
    let render_context = if let Some(start) = lower.find("--render-driver") {
        let mut end = (start + 1600).min(lower.len());
        // The 1600-byte window can land in the middle of a multibyte
        // character when a custom build prints localized help; slicing there
        // would panic.
        while end > start && !lower.is_char_boundary(end) {
            end -= 1;
        }
        lower[start..end].to_string()
    } else {
        String::new()
    };

    known
        .iter()
        .filter(|driver| {
            let quoted = render_context.contains(&format!("\"{}\"", driver))
                || render_context.contains(&format!("'{}'", driver));
            // Match whole tokens too: "opengl" must not be detected just
            // because the help mentions "opengles2".
            let token = render_context
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|part| part == **driver);
            quoted || token
        })
        .map(|driver| (*driver).to_string())
        .collect()
}

#[tauri::command]
pub async fn get_render_drivers(custom_path: Option<String>) -> serde_json::Value {
    let exe_path = get_binary_path("scrcpy", custom_path);
    let host_os = detect_host_os();

    let mut cmd = create_command(&exe_path);
    cmd.arg("--help");
    let output = process::output_with_timeout(cmd, Duration::from_secs(5)).await;

    match output {
        Ok(o) => {
            let out_text = String::from_utf8_lossy(&o.stdout);
            let err_text = String::from_utf8_lossy(&o.stderr);
            let combined = format!("{}{}", out_text, err_text);
            let lower = combined.to_lowercase();

            let supports_render_driver = lower.contains("--render-driver");
            if !supports_render_driver {
                return json!({
                    "success": true,
                    "hostOs": host_os,
                    "supportsRenderDriver": false,
                    "detectedDrivers": [],
                    "supportedDrivers": [],
                    "diagnostics": "scrcpy does not advertise --render-driver in --help output"
                });
            }

            let detected_drivers = detect_render_drivers_from_help(&combined);
            let supported_drivers: Vec<serde_json::Value> = detected_drivers
                .iter()
                .filter(|driver| is_driver_allowed_on_os(driver, host_os))
                .map(|driver| {
                    json!({
                        "id": driver,
                        "label": render_driver_label(driver)
                    })
                })
                .collect();

            json!({
                "success": o.status.success(),
                "hostOs": host_os,
                "supportsRenderDriver": true,
                "detectedDrivers": detected_drivers,
                "supportedDrivers": supported_drivers,
                "diagnostics": "render drivers parsed from scrcpy --help"
            })
        }
        Err(e) => json!({
            "success": false,
            "hostOs": host_os,
            "supportsRenderDriver": false,
            "detectedDrivers": [],
            "supportedDrivers": [],
            "message": e.to_string()
        }),
    }
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScrcpyConfig {
    device: String,
    session_mode: String,
    // ... other fields matching JS ...
    bitrate: Option<u32>,
    fps: Option<u32>,
    stay_awake: Option<bool>,
    turn_off: Option<bool>,
    audio_enabled: Option<bool>,
    audio_codec: Option<String>,
    /// Mirror mode: which device audio scrcpy captures. `output` is the
    /// default and mutes the phone; `playback` captures app media audio.
    mirror_audio_source: Option<String>,
    /// Mirror + playback only: keep the audio playing on the device.
    audio_dup: Option<bool>,
    /// Make scrcpy fail (instead of silently mirroring video only) when the
    /// device cannot capture audio. Off by default so older devices keep
    /// working; the UI surfaces the failure either way.
    require_audio: Option<bool>,
    always_on_top: Option<bool>,
    fullscreen: Option<bool>,
    borderless: Option<bool>,
    record: Option<bool>,
    record_path: Option<String>,
    scrcpy_path: Option<String>,
    otg_pure: Option<bool>,
    camera_facing: Option<String>,
    camera_id: Option<String>,
    codec: Option<String>,
    camera_ar: Option<String>,
    camera_high_speed: Option<bool>,
    vd_width: Option<u32>,
    vd_height: Option<u32>,
    vd_dpi: Option<u32>,
    rotation: Option<String>,
    res: Option<String>,
    hid_keyboard: Option<bool>,
    hid_mouse: Option<bool>,
    render_driver: Option<String>,
    // v4 features
    flex_display: Option<bool>,
    camera_torch: Option<bool>,
    camera_zoom: Option<f32>,
    /// Camera mode: route the phone microphone into the OpenScrcpy Mic virtual
    /// sink so OBS/Discord can select it as a recording device.
    camera_mic_sink: Option<bool>,
    background_color: Option<String>,
    keep_active: Option<bool>,
    /// Retry a session that ends unexpectedly (USB/ADB drop or crash) with
    /// exponential backoff. Deliberate stops are never retried.
    auto_reconnect: Option<bool>,
    /// Force SDL renderer VSync on/off (anti-tearing). scrcpy 4.0 (SDL3)
    /// disables renderer VSync by default; this re-asserts it via the SDL hint.
    vsync: Option<bool>,
    /// Last known screen position of the scrcpy mirror window. Captured (client
    /// area top-left) while a windowed session runs and restored via
    /// --window-x / --window-y on the next launch, so the window reopens where
    /// the user left it, including when relaunching borderless.
    window_x: Option<i32>,
    window_y: Option<i32>,
    // v4.1 features
    /// Ignore video encoder size constraints entirely (scrcpy v4.1+).
    /// Useful when the device reports incorrect encoder limits that prevent
    /// scrcpy from starting.
    ignore_video_encoder_constraints: Option<bool>,
    // Mic bridge (audio-only capture source)
    /// Audio source: "mic" (raw) or "mic-voice-communication" (Android-side
    /// noise suppression + AGC + echo cancellation), for Android 12+.
    /// The legacy "voice-communication" value is still accepted and mapped.
    mic_source: Option<String>,
    /// Lossy codec for the bridge: "opus" (default) or "aac".
    mic_codec: Option<String>,
    /// Bitrate in kbps for the lossy mic codecs (64..256).
    mic_bitrate: Option<u32>,
    /// Experimental: create the virtual sink through a PipeWire RNNoise
    /// filter-chain (PC-side noise suppression). Best effort with fallback.
    #[cfg_attr(not(unix), allow(dead_code))]
    mic_denoise: Option<bool>,
}

fn resolve_audio_codec_flag<'a>(
    config: &'a ScrcpyConfig,
    audio_codec_override: Option<&'a str>,
) -> Option<&'a str> {
    if let Some(c) = audio_codec_override {
        let trimmed = c.trim();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    config.audio_codec.as_deref().and_then(|c| {
        let trimmed = c.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(trimmed)
        }
    })
}

/// Parses `KEY=value` shell output (as printed by `xdotool ... --shell`) and
/// returns the integer value for `key`. Only compiled where it is used (the
/// Linux capture path, and the unit tests).
#[cfg(any(test, target_os = "linux"))]
fn parse_shell_var_int(text: &str, key: &str) -> Option<i32> {
    let prefix = format!("{}=", key);
    text.lines()
        .find(|l| l.starts_with(&prefix))
        .and_then(|l| l[prefix.len()..].trim().parse().ok())
}

/// Windows: finds the first visible, unowned top-level window belonging to
/// process `pid` (matches .NET's `MainWindowHandle` heuristic), i.e. scrcpy's
/// SDL mirror window rather than some owned tool/popup window. Shared by the
/// position-capture and recenter paths so both agree on which window they
/// mean.
#[cfg(target_os = "windows")]
fn find_window_by_pid(pid: u32) -> Option<windows::Win32::Foundation::HWND> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, GW_OWNER,
    };

    // State threaded through EnumWindows via LPARAM.
    struct Search {
        target_pid: u32,
        found: HWND,
    }

    // WNDENUMPROC: BOOL(1) keeps enumerating, BOOL(0) stops.
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        if GetWindow(hwnd, GW_OWNER)
            .map(|o| !o.0.is_null())
            .unwrap_or(false)
        {
            return BOOL(1);
        }
        let mut wnd_pid: u32 = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut wnd_pid as *mut u32));
        if wnd_pid == search.target_pid {
            search.found = hwnd;
            return BOOL(0); // found it -> stop enumeration
        }
        BOOL(1)
    }

    let mut search = Search {
        target_pid: pid,
        found: HWND::default(),
    };
    // EnumWindows returns Err both on real failure and when the callback
    // returns BOOL(0) (our intentional early stop), so ignore the Result and
    // read the captured handle instead.
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut search as *mut Search as isize));
    }
    (!search.found.0.is_null()).then_some(search.found)
}

/// Queries the on-screen position of the scrcpy SDL window owned by process
/// `pid`, returning the client-area top-left in screen pixels. Returns `None`
/// silently whenever the window can't be read (not created yet, minimized,
/// tooling missing, permission denied); every failure mode is non-fatal.
///
/// The reference frame differs per platform and is reconciled afterwards by
/// [`resolve_window_pos_to_persist`], rather than by hardcoding decoration
/// sizes:
/// - Windows: Win32 `ClientToScreen` -> client origin (matches scrcpy/SDL).
/// - Linux/X11 (+XWayland): `xdotool` -> window origin (usually the client area).
/// - macOS: `osascript`/System Events -> frame origin (title-bar top).
fn try_capture_window_pos(pid: u32) -> Option<(i32, i32)> {
    #[cfg(target_os = "windows")]
    {
        // Native Win32: this replaces spawning PowerShell + `Add-Type` (a C#
        // JIT) on every poll, it is a couple of direct syscalls with no child
        // process.
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::ClientToScreen;
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsZoomed};

        if let Some(hwnd) = find_window_by_pid(pid) {
            // A minimized window reports an off-screen sentinel origin
            // (~ -32000,-32000) and a maximized one the monitor edge; neither is
            // the windowed position we want, so skip them and keep the last good
            // sample.
            let special = unsafe { IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() };
            if !special {
                // ClientToScreen maps the client-area origin (0,0) into screen
                // pixels; GetWindowRect would return the outer frame and creep
                // the window up by the title-bar height on every relaunch.
                let mut pt = POINT { x: 0, y: 0 };
                if unsafe { ClientToScreen(hwnd, &mut pt) }.as_bool() {
                    return Some((pt.x, pt.y));
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let pid_str = pid.to_string();
        // X11/XWayland: xdotool can see the window directly. On native
        // Wayland it finds nothing, so fall through to KDE's reported cache.
        if let Ok(output) = StdCommand::new("xdotool")
            .args(["search", "--pid", &pid_str, "getwindowgeometry", "--shell"])
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if let (Some(x), Some(y)) = (
                    parse_shell_var_int(&text, "X"),
                    parse_shell_var_int(&text, "Y"),
                ) {
                    return Some((x, y));
                }
            }
        }
        if let Some(position) = crate::kwin::cached_position(pid) {
            return Some(position);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // System Events needs Accessibility permission for the app (one-time
        // grant in System Settings). The try block swallows every error (no
        // window yet, permission denied, ...) so this stays a silent no-op
        // until the permission is granted.
        let script = format!(
            concat!(
                "tell application \"System Events\"\n",
                "try\n",
                "set proc to first process whose unix id is {}\n",
                "set pos to position of first window of proc\n",
                "((item 1 of pos) as string) & \" \" & ((item 2 of pos) as string)\n",
                "end try\n",
                "end tell"
            ),
            pid
        );
        let output = StdCommand::new("osascript")
            .args(["-e", &script])
            .output()
            .ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut parts = text.split_whitespace();
            let x: i32 = parts.next()?.parse().ok()?;
            let y: i32 = parts.next()?.parse().ok()?;
            return Some((x, y));
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    let _ = pid;
    None
}

/// Moves the scrcpy mirror window owned by `pid` to the centre of the primary
/// screen and brings it to the front. A safety net for a persisted position
/// that no longer lands on any connected monitor (e.g. it was saved while a
/// second monitor was attached, which has since been unplugged) -- the window
/// can reopen fully off-screen, with no title bar to drag back if borderless.
/// Best-effort and silent: does nothing if the window can't be found, is
/// already maximized (it fills its monitor; there is nothing to recover), or
/// the move fails.
fn recenter_window(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, GetWindowRect, IsIconic, IsZoomed, SetForegroundWindow, SetWindowPos,
            ShowWindow, SM_CXSCREEN, SM_CYSCREEN, SWP_NOSIZE, SWP_NOZORDER, SW_RESTORE,
        };

        let Some(hwnd) = find_window_by_pid(pid) else {
            return;
        };
        if unsafe { IsZoomed(hwnd) }.as_bool() {
            return;
        }
        if unsafe { IsIconic(hwnd) }.as_bool() {
            let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return;
        }
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let (screen_w, screen_h) =
            unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                (screen_w - w) / 2,
                (screen_h - h) / 2,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER,
            )
        };
        let _ = unsafe { SetForegroundWindow(hwnd) };
    }
    #[cfg(target_os = "linux")]
    {
        // X11/XWayland: xdotool is the direct path. Native Wayland hides the
        // window from X11 tooling, so hand it to KWin's scripting interface.
        if !recenter_window_xdotool(pid) {
            crate::kwin::recenter(pid);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Same Accessibility-permission requirement as the position capture
        // above; a silent no-op until it is granted.
        let script = format!(
            concat!(
                "tell application \"System Events\"\n",
                "try\n",
                "set proc to first process whose unix id is {}\n",
                "set win to first window of proc\n",
                "set {{winW, winH}} to size of win\n",
                "tell application \"Finder\" to set screenBounds to bounds of window of desktop\n",
                "set screenW to (item 3 of screenBounds)\n",
                "set screenH to (item 4 of screenBounds)\n",
                "set position of win to {{((screenW - winW) / 2), ((screenH - winH) / 2)}}\n",
                "set frontmost of proc to true\n",
                "end try\n",
                "end tell"
            ),
            pid
        );
        let _ = StdCommand::new("osascript").args(["-e", &script]).output();
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    let _ = pid;
}

/// X11/XWayland recentre via xdotool. Returns false when the window is not
/// visible to X11 (native Wayland), so the caller can try KWin instead.
#[cfg(target_os = "linux")]
fn recenter_window_xdotool(pid: u32) -> bool {
    let pid_str = pid.to_string();
    let Ok(search_out) = StdCommand::new("xdotool")
        .args(["search", "--pid", &pid_str])
        .output()
    else {
        return false;
    };
    let Some(window_id) = String::from_utf8_lossy(&search_out.stdout)
        .lines()
        .next()
        .map(str::to_string)
    else {
        return false;
    };

    let Ok(geom_out) = StdCommand::new("xdotool")
        .args(["getwindowgeometry", "--shell", &window_id])
        .output()
    else {
        return false;
    };
    let geom_text = String::from_utf8_lossy(&geom_out.stdout);
    let (Some(w), Some(h)) = (
        parse_shell_var_int(&geom_text, "WIDTH"),
        parse_shell_var_int(&geom_text, "HEIGHT"),
    ) else {
        return false;
    };

    let Ok(display_out) = StdCommand::new("xdotool")
        .arg("getdisplaygeometry")
        .output()
    else {
        return false;
    };
    let display_text = String::from_utf8_lossy(&display_out.stdout);
    let mut parts = display_text.split_whitespace();
    let (Some(Ok(screen_w)), Some(Ok(screen_h))) = (
        parts.next().map(str::parse::<i32>),
        parts.next().map(str::parse::<i32>),
    ) else {
        return false;
    };

    let x = ((screen_w - w).max(0) / 2).to_string();
    let y = ((screen_h - h).max(0) / 2).to_string();
    let _ = StdCommand::new("xdotool")
        .args(["windowmove", &window_id, &x, &y])
        .output();
    let _ = StdCommand::new("xdotool")
        .args(["windowactivate", &window_id])
        .output();
    true
}

/// Re-centres the running mirror window for `device` on the primary screen.
/// Bound to a global OS shortcut (Ctrl+Alt+Shift+C, see `shortcuts.rs`) so a
/// window whose saved position ends up off-screen (see [`recenter_window`])
/// can be recovered without being able to see or focus it directly. A no-op
/// if `device` has no tracked process (e.g. nothing is running).
pub fn recenter_device(state: &ScrcpyState, device: &str) {
    let pid = state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device)
        .map(|s| s.pid.load(Ordering::Relaxed))
        .filter(|pid| *pid != 0);
    if let Some(pid) = pid {
        recenter_window(pid);
    }
}

#[tauri::command]
pub fn recenter_scrcpy_window(state: State<'_, ScrcpyState>, device: String) -> Result<(), String> {
    recenter_device(&state, &device);
    Ok(())
}

/// Records which device the GUI currently considers selected, so the global
/// recentre shortcut (which fires with no window necessarily focused) knows
/// which mirror window to act on.
#[tauri::command]
pub fn set_active_device(
    state: State<'_, ScrcpyState>,
    device: Option<String>,
) -> Result<(), String> {
    *state
        .active_device
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = device;
    Ok(())
}

/// Converts a raw captured position into the value to persist as
/// --window-x/--window-y so the window round-trips exactly on the next launch.
///
/// [`try_capture_window_pos`] reports coordinates in each OS tool's native
/// frame, which may differ from scrcpy's own (SDL) reference by the window
/// decorations, a difference that varies by platform, window manager, theme
/// and SDL version. Instead of hardcoding decoration sizes we learn the offset
/// live: when we launched at an explicit position (`requested`), the delta
/// between it and the *first* sample (`first`, taken before the user can move
/// the window) is the decoration offset; subtracting it from the last sample
/// yields a value that round-trips. Offsets larger than any plausible window
/// decoration are ignored (the window was moved before the first sample, or was
/// placed by the WM), falling back to the raw `last` position.
fn resolve_window_pos_to_persist(
    requested: Option<(i32, i32)>,
    first: Option<(i32, i32)>,
    last: (i32, i32),
) -> (i32, i32) {
    if let (Some((rx, ry)), Some((fx, fy))) = (requested, first) {
        let (dx, dy) = (fx - rx, fy - ry);
        if dx.abs() <= 80 && dy.abs() <= 120 {
            return (last.0 - dx, last.1 - dy);
        }
    }
    last
}

/// Only windowed mirror sessions have a persisted window position. Camera and
/// desktop windows must not inherit the mirror's saved position, and a
/// fullscreen session has nothing to restore.
fn tracks_window_position(config: &ScrcpyConfig) -> bool {
    config.session_mode == "mirror" && !config.fullscreen.unwrap_or(false)
}

fn session_mode_label(mode: &str) -> &'static str {
    match mode {
        "camera" => "Camera",
        "mic" => "Microphone bridge",
        "desktop" => "Desktop",
        _ => "Mirror",
    }
}

/// Fails before spawning when the selected mode needs a flag this scrcpy build
/// does not have. Cosmetic flags are filtered separately with a warning.
fn ensure_mode_supported(config: &ScrcpyConfig, caps: &Capabilities) -> Result<(), String> {
    let mut required: Vec<&str> = match config.session_mode.as_str() {
        "camera" => vec!["video-source", "camera-size"],
        "mic" => vec!["no-window", "no-video", "audio-source"],
        "desktop" => vec!["new-display"],
        _ => Vec::new(),
    };
    if config.audio_enabled.unwrap_or(true) && config.session_mode == "camera" {
        required.push("audio-source");
    }

    let hid = config.session_mode != "mic"
        && (config.hid_keyboard.unwrap_or(false) || config.hid_mouse.unwrap_or(false));
    let otg_pure = config.otg_pure.unwrap_or(false);
    let tcp = config.device.contains('.') || config.device.contains(':');
    if config.session_mode == "mirror" && hid && otg_pure {
        if tcp {
            required.extend(["no-video", "no-audio", "keyboard", "mouse"]);
        } else {
            required.push("otg");
        }
    } else if hid {
        if config.hid_keyboard.unwrap_or(false) {
            required.push("keyboard");
        }
        if config.hid_mouse.unwrap_or(false) {
            required.push("mouse");
        }
    }

    let missing: Vec<String> = required
        .iter()
        .filter(|flag| !caps.supports(flag))
        .map(|flag| format!("--{flag}"))
        .collect();
    if !missing.is_empty() {
        let hint = if config.session_mode == "mirror" {
            "Update scrcpy to enable these options."
        } else {
            "Update scrcpy or use Mirror mode."
        };
        return Err(format!(
            "{} mode requires options missing from scrcpy {}: {}. {hint}",
            session_mode_label(&config.session_mode),
            caps.version(),
            missing.join(", ")
        ));
    }

    if config.audio_enabled.unwrap_or(true) && !caps.supports("audio-codec") {
        return Err(format!(
            "Audio forwarding is not supported by scrcpy {} (missing --audio-codec). Disable audio or update scrcpy.",
            caps.version()
        ));
    }
    if config.require_audio.unwrap_or(false)
        && config.audio_enabled.unwrap_or(true)
        && !caps.supports("require-audio")
    {
        return Err(format!(
            "Require audio is not supported by scrcpy {} (missing --require-audio). Disable it or update scrcpy.",
            caps.version()
        ));
    }
    Ok(())
}

/// Replaces the denoised microphone source when the detected build does not
/// offer it, so an old custom binary keeps recording instead of failing.
fn normalize_mic_source(config: &mut ScrcpyConfig, caps: &Capabilities) -> Option<String> {
    if config.session_mode != "mic" {
        return None;
    }
    let source = validate::normalize_mic_source(config.mic_source.as_deref().unwrap_or("mic"));
    if source == "mic-voice-communication" && !caps.supports_audio_source(source) {
        config.mic_source = Some("mic".to_string());
        return Some(format!(
            "[SYSTEM] scrcpy {} does not support the denoised microphone source; using the raw microphone.",
            caps.version()
        ));
    }
    None
}

/// Falls back to the device output when the selected mirror audio source is
/// not offered by this scrcpy build. `--audio-dup` is only meaningful with
/// `--audio-source=playback`, so it is disabled when the source falls back or
/// the binary does not support the flag.
fn normalize_mirror_audio(config: &mut ScrcpyConfig, caps: &Capabilities) -> Option<String> {
    if config.session_mode != "mirror" || !config.audio_enabled.unwrap_or(true) {
        return None;
    }
    let source = config
        .mirror_audio_source
        .as_deref()
        .unwrap_or("output")
        .trim()
        .to_string();
    if source != "output" && !caps.supports_audio_source(&source) {
        config.mirror_audio_source = Some("output".to_string());
        config.audio_dup = Some(false);
        return Some(format!(
            "[AUDIO] scrcpy {} does not support audio source '{}'; using the device output instead.",
            caps.version(),
            source
        ));
    }
    if source == "playback" && config.audio_dup.unwrap_or(false) && !caps.supports("audio-dup") {
        config.audio_dup = Some(false);
        return Some(format!(
            "[AUDIO] scrcpy {} does not support --audio-dup; playback will mute the device while mirroring.",
            caps.version()
        ));
    }
    None
}

/// Why the monitor loop is ending. The frontend uses this to distinguish a
/// deliberate stop from a crash or a user closing the scrcpy window.
#[derive(Clone, Copy)]
enum SessionEndReason {
    Stopped,
    Exited { success: bool },
    Error,
}

fn session_end_payload(device: &str, reason: SessionEndReason) -> serde_json::Value {
    match reason {
        SessionEndReason::Stopped => {
            json!({ "device": device, "running": false, "reason": "stopped" })
        }
        SessionEndReason::Exited { success } => json!({
            "device": device,
            "running": false,
            "reason": "exited",
            "success": success
        }),
        SessionEndReason::Error => {
            json!({ "device": device, "running": false, "reason": "error" })
        }
    }
}

/// Emits the resolved window position (when one was captured) followed by the
/// session-stopped status. Shared by every monitor-loop exit path.
fn emit_window_pos_and_stop(
    window: &Window,
    device: &str,
    requested: Option<(i32, i32)>,
    first: Option<(i32, i32)>,
    last: Option<(i32, i32)>,
    reason: SessionEndReason,
) {
    if let Some(last) = last {
        let (x, y) = resolve_window_pos_to_persist(requested, first, last);
        let _ = window.emit(
            "scrcpy-window-pos",
            json!({ "device": device, "x": x, "y": y }),
        );
    }
    let _ = window.emit("scrcpy-status", session_end_payload(device, reason));
}

/// True when this session should send audio into the shared virtual sink.
/// Camera mode already captures the microphone; opting into the sink makes it
/// selectable as a recording device instead of only playing on the default
/// output. The microphone bridge always uses the sink on Linux.
fn wants_virtual_mic(config: &ScrcpyConfig) -> bool {
    config.session_mode == "mic"
        || (config.session_mode == "camera"
            && config.camera_mic_sink.unwrap_or(false)
            && config.audio_enabled.unwrap_or(true))
}

fn build_scrcpy_args(
    config: &ScrcpyConfig,
    video_dir_fallback: Option<String>,
    audio_codec_override: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::new();

    // Construct arguments based on config
    if !config.device.is_empty() {
        args.push("-s".to_string());
        args.push(config.device.clone());
    }

    let codec = config.codec.as_deref().unwrap_or("h264");
    args.push(format!("--video-codec={}", codec));

    let otg_pure = config.otg_pure.unwrap_or(false);
    let hid_keyboard = config.hid_keyboard.unwrap_or(false);
    let hid_mouse = config.hid_mouse.unwrap_or(false);

    if config.session_mode == "mirror" && (hid_keyboard || hid_mouse) && otg_pure {
        if config.device.contains('.') || config.device.contains(':') {
            args.push("--no-video".to_string());
            args.push("--no-audio".to_string());
            args.push("--keyboard=uhid".to_string());
            args.push("--mouse=uhid".to_string());
        } else {
            args.push("--otg".to_string());
        }
    } else {
        if config.session_mode == "mic" {
            // Audio-only bridge: the phone's mic reaches the PC as a
            // recording device, with no video pipeline and no window. Every
            // video-specific knob below is irrelevant here, so this branch
            // builds a minimal set and returns early.
            args.push("--no-window".to_string());
            args.push("--no-video".to_string());
            let source = config.mic_source.as_deref().unwrap_or("").trim();
            let source = if source.is_empty() { "mic" } else { source };
            // Older builds persisted "voice-communication"; scrcpy expects
            // "mic-voice-communication". Normalize here so a saved config
            // keeps working after the rename.
            let source = validate::normalize_mic_source(source);
            args.push(format!("--audio-source={}", source));
            // The bridge has its own codec choice; the automatic repair
            // chain's override (when present) still wins so recovery can try
            // alternatives after a codec failure.
            let codec = audio_codec_override
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    config
                        .mic_codec
                        .as_deref()
                        .map(str::trim)
                        .filter(|c| !c.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "opus".to_string());
            args.push(format!("--audio-codec={}", codec));
            if codec == "opus" || codec == "aac" {
                let kbps = config.mic_bitrate.unwrap_or(128).clamp(32, 256);
                args.push(format!("--audio-bit-rate={}k", kbps));
            }
            return args;
        }
        if hid_keyboard {
            args.push("--keyboard=uhid".to_string());
        }
        if hid_mouse {
            args.push("--mouse=uhid".to_string());
        }

        if let Some(render_driver) = &config.render_driver {
            let selected_driver = render_driver.trim();
            if !selected_driver.is_empty() && selected_driver != "auto" {
                args.push("--render-driver".to_string());
                args.push(selected_driver.to_string());
            }
        }

        if let Some(bitrate) = config.bitrate {
            args.push("--video-bit-rate".to_string());
            args.push(format!("{}M", bitrate));
        }

        let audio_enabled = config.audio_enabled.unwrap_or(true);
        if !audio_enabled {
            args.push("--no-audio".to_string());
        }
        if audio_enabled {
            // A webcam needs the phone's microphone; the default `output`
            // source would capture media audio and mute the device.
            if config.session_mode == "camera" {
                args.push("--audio-source=mic".to_string());
            }
            if config.session_mode == "mirror" {
                let source = config
                    .mirror_audio_source
                    .as_deref()
                    .unwrap_or("output")
                    .trim();
                if source != "output" && validate::MIRROR_AUDIO_SOURCES.contains(&source) {
                    args.push(format!("--audio-source={}", source));
                }
                // scrcpy only accepts --audio-dup together with
                // --audio-source=playback.
                if source == "playback" && config.audio_dup.unwrap_or(false) {
                    args.push("--audio-dup".to_string());
                }
            }
            if let Some(codec) = resolve_audio_codec_flag(config, audio_codec_override) {
                args.push(format!("--audio-codec={}", codec));
            }
            // By default scrcpy degrades to video-only when audio capture
            // fails. The opt-in makes that failure visible (scrcpy exits),
            // which this module then reports in the console.
            if config.require_audio.unwrap_or(false) {
                args.push("--require-audio".to_string());
            }
        }
        if let Some(aot) = config.always_on_top {
            if aot {
                args.push("--always-on-top".to_string());
            }
        }
        if let Some(fs) = config.fullscreen {
            if fs {
                args.push("--fullscreen".to_string());
            }
        }
        if let Some(bl) = config.borderless {
            if bl {
                args.push("--window-borderless".to_string());
            }
        }

        // Restore the last saved window position (client-area origin), for
        // mirror sessions only. Skipped in fullscreen (scrcpy ignores it) and
        // for camera/desktop, whose windows must not share, or overwrite, the
        // mirror window's remembered position.
        if config.session_mode == "mirror" && !config.fullscreen.unwrap_or(false) {
            if let Some(wx) = config.window_x {
                args.push(format!("--window-x={}", wx));
            }
            if let Some(wy) = config.window_y {
                args.push(format!("--window-y={}", wy));
            }
        }

        if let Some(rot) = &config.rotation {
            if rot != "0" {
                args.push("--orientation".to_string());
                args.push(rot.clone());
            }
        }

        let can_control = config.session_mode != "camera";
        if can_control {
            if let Some(sa) = config.stay_awake {
                if sa {
                    args.push("--stay-awake".to_string());
                }
            }
            if let Some(ka) = config.keep_active {
                if ka {
                    args.push("--keep-active".to_string());
                }
            }
            if let Some(to) = config.turn_off {
                if to {
                    // Keep --no-power-on off as a fallback. The startup path
                    // also wakes the display over ADB before launching (see
                    // `wake_display_before_screen_off`) to avoid the async
                    // power-on/power-off race, but scrcpy must still be able
                    // to wake a phone whose ADB wake failed.
                    args.push("--turn-screen-off".to_string());
                }
            }
        }

        if config.session_mode == "camera" {
            args.push("--video-source=camera".to_string());
            if let Some(cid) = &config.camera_id {
                if !cid.is_empty() {
                    args.push(format!("--camera-id={}", cid));
                } else if let Some(facing) = &config.camera_facing {
                    args.push(format!("--camera-facing={}", facing));
                }
            } else if let Some(facing) = &config.camera_facing {
                args.push(format!("--camera-facing={}", facing));
            }

            // Resolution logic: Map res (e.g., "1920") to standard camera sizes (e.g., "1920x1080")
            if let Some(res) = &config.res {
                if res != "0" {
                    let camera_size = match res.as_str() {
                        "3840" => "3840x2160",
                        "2560" => "2560x1440",
                        "1920" => "1920x1080",
                        "1600" => "1600x900",
                        "1280" => "1280x720",
                        "1024" => "1024x576",
                        "800" => "800x480",
                        _ => "1920x1080",
                    };
                    args.push(format!("--camera-size={}", camera_size));
                } else {
                    // Safe fallback default to avoid camera configuration error on non-standard sensor photo sizes (e.g. 4080x3060)
                    args.push("--camera-size=1920x1080".to_string());
                }
            } else {
                args.push("--camera-size=1920x1080".to_string());
            }

            if let Some(ar) = &config.camera_ar {
                if ar != "0" {
                    args.push(format!("--camera-ar={}", ar));
                }
            }
            if let Some(chs) = config.camera_high_speed {
                if chs {
                    args.push("--camera-high-speed".to_string());
                }
            }
            // v4: camera torch and zoom
            if let Some(true) = config.camera_torch {
                args.push("--camera-torch".to_string());
            }
            if let Some(zoom) = config.camera_zoom {
                if zoom > 1.005 {
                    // avoid floating point noise at 1.0
                    args.push(format!("--camera-zoom={:.1}", zoom));
                }
            }
            // fps handled generically below
        } else if config.session_mode == "desktop" {
            let w = config.vd_width.unwrap_or(1920);
            let h = config.vd_height.unwrap_or(1080);
            let dpi = config.vd_dpi.unwrap_or(420);
            args.push(format!("--new-display={}x{}/{}", w, h, dpi));
            args.push("--video-buffer=100".to_string());
            // v4: flex display (resize virtual display with window)
            if let Some(true) = config.flex_display {
                args.push("--flex-display".to_string());
            }
        }

        if let Some(fps) = config.fps {
            if fps > 0 {
                if config.session_mode == "camera" {
                    args.push("--camera-fps".to_string());
                } else {
                    args.push("--max-fps".to_string());
                }
                args.push(fps.to_string());
            }
        } else if config.session_mode == "camera" && config.camera_high_speed.unwrap_or(false) {
            args.push("--camera-fps".to_string());
            args.push("60".to_string());
        }

        // Shared resolution logic (applies to mirror and desktop)
        if config.session_mode != "camera" {
            if let Some(res) = &config.res {
                if res != "0" {
                    args.push("--max-size".to_string());
                    args.push(res.clone());
                }
            }
        }

        if let Some(rec) = config.record {
            if rec {
                let mut path = config.record_path.clone().unwrap_or_default();

                // If path is empty, try to get Video dir fallback
                if path.trim().is_empty() {
                    path = video_dir_fallback.unwrap_or_else(|| ".".to_string());
                }

                // The device id can come from user-typed IPs or ADB output,
                // so sanitize it before using it inside a file name.
                let safe_device = sanitize_filename_component(&config.device);
                let filename = format!(
                    "scrcpy_{}_{}.mkv",
                    safe_device,
                    chrono::Local::now().format("%Y%m%d_%H%M%S")
                );
                let full_path = std::path::Path::new(&path).join(filename);
                args.push(format!("--record={}", full_path.to_string_lossy()));
            }
        }

        // v4: background color (all modes)
        if let Some(ref color) = config.background_color {
            let trimmed = color.trim();
            if !trimmed.is_empty() {
                args.push(format!("--background-color={}", trimmed));
            }
        }

        // v4.1: ignore video encoder size constraints
        if let Some(true) = config.ignore_video_encoder_constraints {
            args.push("--ignore-video-encoder-constraints".to_string());
        }
    }

    args
}

/// Screen-off sessions are the only ones where scrcpy must power the device
/// on and then off again. Camera and mic sessions never use the flag (see
/// `build_scrcpy_args`), so they must not pay the extra ADB round-trip.
fn wakes_before_screen_off(config: &ScrcpyConfig) -> bool {
    config.turn_off.unwrap_or(false)
        && !config.device.is_empty()
        && matches!(config.session_mode.as_str(), "mirror" | "desktop")
}

/// Reads the display state from a `dumpsys power` snapshot. The most specific
/// signal wins: on recent Android versions `mWakefulness=Awake` can coexist
/// with a display that was just powered off, so the explicit display state
/// (and the legacy `mScreenOn` flag) must be checked first.
fn display_power_is_on(dumpsys_power: &str) -> bool {
    if let Some(state) = dumpsys_power.split("Display Power: state=").nth(1) {
        return state.trim_start().starts_with("ON");
    }
    if let Some(state) = dumpsys_power.split("mScreenOn=").nth(1) {
        return state.trim_start().starts_with("true");
    }
    if let Some(state) = dumpsys_power.split("mWakefulness=").nth(1) {
        return state.trim_start().starts_with("Awake");
    }
    false
}

async fn read_display_power_state(adb_exe_path: &str, device: &str) -> bool {
    let mut cmd = create_command(adb_exe_path);
    cmd.args(["-s", device, "shell", "dumpsys", "power"]);
    match process::output_with_timeout(cmd, DISPLAY_POWER_QUERY_TIMEOUT).await {
        Ok(output) if output.status.success() => {
            display_power_is_on(&String::from_utf8_lossy(&output.stdout))
        }
        _ => false,
    }
}

/// Wakes the device display before starting a screen-off session.
///
/// scrcpy powers the device on by itself, but it does so asynchronously and
/// then sends the `--turn-screen-off` request right after setup. On slower
/// devices the power-off can win the race and be overwritten by the pending
/// power-on, which is why the phone's screen sometimes only turns off after
/// the user wakes it by hand. Waking it here (and waiting for Android to
/// report the display as on) makes scrcpy start from a known state, so its
/// screen-off request lands last.
async fn wake_display_before_screen_off(window: &Window, adb_exe_path: &str, device: &str) {
    let _ = window.emit(
        "scrcpy-log",
        "[SYSTEM] Screen off requested: waking the device display...".to_string(),
    );

    let mut wake = create_command(adb_exe_path);
    wake.args(["-s", device, "shell", "input", "keyevent", "224"]);
    let wake_ok = process::output_with_timeout(wake, ADB_WAKE_TIMEOUT)
        .await
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !wake_ok {
        let _ = window.emit(
            "scrcpy-log",
            "[SYSTEM] Could not wake the device display; scrcpy will try to power it on."
                .to_string(),
        );
        return;
    }

    let deadline = tokio::time::Instant::now() + DISPLAY_WAKE_SETTLE_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if read_display_power_state(adb_exe_path, device).await {
            let _ = window.emit(
                "scrcpy-log",
                "[SYSTEM] Device display is awake.".to_string(),
            );
            return;
        }
        tokio::time::sleep(DISPLAY_WAKE_POLL_INTERVAL).await;
    }

    let _ = window.emit(
        "scrcpy-log",
        "[SYSTEM] Device display did not report as awake; scrcpy will try to power it on."
            .to_string(),
    );
}

async fn spawn_scrcpy_streams(
    window: &Window,
    exe_path: &str,
    adb_exe_path: &str,
    server_path: Option<&str>,
    args: &[String],
    vsync: bool,
    pulse_sink: Option<&str>,
) -> Result<(tokio::process::Child, AudioErrorState), String> {
    let command_str = format!("> scrcpy {}", args.join(" "));
    let _ = window.emit("scrcpy-log", command_str);

    let mut command = create_command(exe_path);
    command.args(args);
    command.env("ADB", adb_exe_path);
    // Make the mirror window identify itself as OpenScrcpy instead of the
    // bundled "scrcpy" binary. KDE then matches the launcher's desktop entry
    // (icon, grouping, pinning) instead of logging
    // `Failed to find service for Unity Launcher "scrcpy.desktop"`.
    // SDL3 (scrcpy 4+) reads SDL_APP_ID; the two legacy names cover SDL2.
    #[cfg(target_os = "linux")]
    {
        command.env("SDL_APP_ID", "openscrcpy");
        command.env("SDL_VIDEO_WAYLAND_WMCLASS", "openscrcpy");
        command.env("SDL_VIDEO_X11_WMCLASS", "openscrcpy");
    }
    // Mic bridge on Linux: libpulse honours PULSE_SINK as the client's
    // default output, which lands scrcpy's stream inside our virtual sink.
    if let Some(sink) = pulse_sink {
        command.env("PULSE_SINK", sink);
    }
    if let Some(sp) = server_path {
        if Path::new(sp).exists() {
            command.env("SCRCPY_SERVER_PATH", sp);
        }
    }
    // scrcpy 4.0 migrated from SDL2 to SDL3, which leaves the renderer's VSync
    // disabled by default and reintroduced screen tearing for users who had
    // none on scrcpy 3.x (SDL2). Re-assert VSync through SDL's hint env var so
    // the renderer syncs to the display refresh; "0" lets the user opt out
    // (slightly lower input latency) via the in-app toggle.
    command.env("SDL_RENDER_VSYNC", if vsync { "1" } else { "0" });
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|e| e.to_string())?;

    // Both streams are piped just above, so take() cannot be None here.
    let stdout = child.stdout.take().expect("Failed to capture stdout");
    let stderr = child.stderr.take().expect("Failed to capture stderr");

    let audio_state = AudioErrorState::default();
    spawn_log_reader(stdout, window.clone(), None);
    spawn_log_reader(stderr, window.clone(), Some(audio_state.clone()));

    Ok((child, audio_state))
}

/// Buffers scrcpy's output for 100 ms so a burst of lines reaches the GUI as
/// one log event. The stderr reader also watches for audio failures: codec
/// errors drive the automatic fallback chain, capture failures are surfaced
/// to the user (scrcpy itself keeps mirroring video when capture fails).
fn spawn_log_reader<R>(reader: R, window: Window, audio_state: Option<AudioErrorState>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        let mut buffer = Vec::new();
        let mut interval = tokio::time::interval(Duration::from_millis(100));

        loop {
            tokio::select! {
                line_res = lines.next_line() => {
                    match line_res {
                        Ok(Some(line)) => {
                            if let Some(state) = &audio_state {
                                if is_audio_capture_error(&line) {
                                    state.note_capture();
                                } else if is_audio_codec_error(&line) {
                                    state.note_codec();
                                }
                            }
                            buffer.push(line);
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
                _ = interval.tick() => {
                    if !buffer.is_empty() {
                        let _ = window.emit("scrcpy-log", buffer.join("\n"));
                        buffer.clear();
                    }
                }
            }
        }
        if !buffer.is_empty() {
            let _ = window.emit("scrcpy-log", buffer.join("\n"));
        }
    });
}

enum AttemptOutcome {
    Exited { description: String, success: bool },
    WaitError(String),
}

/// Asks the child to exit, waits `grace`, then forces it. Returns whether the
/// graceful path was enough.
async fn terminate_child(child: &mut tokio::process::Child, grace: Duration) -> bool {
    #[cfg(target_os = "windows")]
    if let Some(pid) = child.id() {
        let _ = StdCommand::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    if let Some(pid) = child.id() {
        let _ = StdCommand::new("kill").arg(pid.to_string()).output();
    }

    match tokio::time::timeout(grace, child.wait()).await {
        Ok(Ok(_)) => true,
        Ok(Err(_)) | Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            false
        }
    }
}

/// Last-resort kill used when the monitor task cannot run or misses the
/// deadline. SIGKILL/TerminateProcess cannot be ignored, so no wait is needed.
fn force_kill_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        let _ = StdCommand::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = StdCommand::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

// ── Microphone bridge helpers (Linux / PipeWire-PulseAudio) ─────────
// For the phone mic to become a *recording device* on the PC, we create a
// dedicated null sink ("OpenScrcpy Mic"): scrcpy's stream plays into it and
// the sink's monitor shows up to every app as a microphone input. The sink
// is service-owned: created at bridge start, unloaded when nothing uses it.
#[cfg(unix)]
const MIC_SINK_NAME: &str = "openscrcpy_mic";
#[cfg(unix)]
const MIC_SINK_LABEL: &str = "OpenScrcpy Mic";

#[cfg(unix)]
struct PactlOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

#[cfg(unix)]
async fn pactl_run(args: &[&str]) -> Option<PactlOut> {
    let mut cmd = create_command("pactl");
    cmd.args(args);
    let output = process::output_with_timeout(cmd, Duration::from_secs(4))
        .await
        .ok()?;
    Some(PactlOut {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

// Returns true when the virtual sink exists (created now, if absent).
// With mic_denoise (experimental) the sink is born from a PipeWire RNNoise
// filter-chain, so apps recording its monitor pick up suppressed noise.
#[cfg(unix)]
async fn ensure_mic_sink(window: &Window, denoise: bool) -> bool {
    let Some(sinks) = pactl_run(&["list", "short", "sinks"]).await else {
        let _ = window.emit(
            "scrcpy-log",
            "[MIC] pactl not found; routing skipped (audio goes to the default output)."
                .to_string(),
        );
        return false;
    };
    if sinks
        .stdout
        .lines()
        .any(|l| l.split('\t').nth(1) == Some(MIC_SINK_NAME))
    {
        let message = if denoise {
            "[MIC] Shared virtual sink OpenScrcpy Mic already loaded; reusing it (noise suppression is only applied when the bridge creates the sink)."
        } else {
            "[MIC] Virtual sink OpenScrcpy Mic already loaded."
        };
        let _ = window.emit("scrcpy-log", message.to_string());
        return true;
    }

    if denoise {
        // RNNoise ships with PipeWire's example filter-chain configs; try the
        // known install paths before falling back to the plain sink.
        let candidates = [
            "/usr/share/pipewire/filter-chain/sink-rnnoise.conf",
            "/usr/share/pipewire/filter-chain/rnnoise-sink.conf",
        ];
        match candidates.iter().find(|p| Path::new(p).exists()) {
            Some(conf) => {
                let load = pactl_run(&[
                    "load-module",
                    "module-filter-chain",
                    &format!("x={}", conf),
                    &format!("sink_name={}", MIC_SINK_NAME),
                    &format!("sink_properties=device.description=\"{}\"", MIC_SINK_LABEL),
                ])
                .await;
                match load {
                    Some(o) if o.ok => {
                        let _ = window.emit(
                            "scrcpy-log",
                            format!(
                                "[MIC] Noise-suppressed sink \"OpenScrcpy Mic\" created (RNNoise: {}).",
                                conf
                            ),
                        );
                        return true;
                    }
                    Some(o) => {
                        let _ = window.emit(
                            "scrcpy-log",
                            format!("[MIC] RNNoise sink rejected by PipeWire: {}.", o.stderr),
                        );
                    }
                    None => {
                        let _ = window.emit(
                            "scrcpy-log",
                            "[MIC] RNNoise sink failed to load; trying the plain sink.".to_string(),
                        );
                    }
                }
            }
            None => {
                let _ = window.emit(
                    "scrcpy-log",
                    "[MIC] RNNoise filter-chain file not found; using the unfiltered sink."
                        .to_string(),
                );
            }
        }
    }

    let created = pactl_run(&[
        "load-module",
        "module-null-sink",
        &format!("sink_name={}", MIC_SINK_NAME),
        &format!("sink_properties=device.description=\"{}\"", MIC_SINK_LABEL),
    ])
    .await;
    match created {
        Some(o) if o.ok => {
            let _ = window.emit(
                "scrcpy-log",
                format!(
                    "[MIC] Virtual sink \"{}\" created — select \"Monitor of {}\" (or the matching monitor source) as the microphone in Discord/OBS/Recorder.",
                    MIC_SINK_LABEL, MIC_SINK_LABEL
                ),
            );
            true
        }
        Some(o) => {
            let _ = window.emit(
                "scrcpy-log",
                format!("[MIC] Could not create the virtual sink: {}.", o.stderr),
            );
            false
        }
        None => {
            let _ = window.emit(
                "scrcpy-log",
                "[MIC] Could not create the virtual sink; audio goes to the default output."
                    .to_string(),
            );
            false
        }
    }
}

#[cfg(unix)]
async fn unload_mic_sink(window: &Window) {
    let Some(sinks) = pactl_run(&["list", "short", "sinks"]).await else {
        return;
    };
    if !sinks
        .stdout
        .lines()
        .any(|l| l.split('\t').nth(1) == Some(MIC_SINK_NAME))
    {
        return;
    }
    let Some(modules) = pactl_run(&["list", "short", "modules"]).await else {
        return;
    };
    let module_idx = modules
        .stdout
        .lines()
        .find(|l| l.contains(&format!("sink_name={}", MIC_SINK_NAME)))
        .and_then(|l| l.split('\t').next().map(str::to_string));
    if let Some(idx) = module_idx {
        let _ = pactl_run(&["unload-module", &idx]).await;
        let _ = window.emit(
            "scrcpy-log",
            "[MIC] Virtual sink unloaded (no active session is using it).".to_string(),
        );
    }
}

/// Reserves one user of the shared virtual sink. The caller must hold
/// `ScrcpyState::mic_sink_lock` across creation and this reservation, so a
/// session stopping at the same time cannot unload the sink in between.
#[cfg(unix)]
fn reserve_mic_sink_user(users: &AtomicUsize) {
    users.fetch_add(1, Ordering::Relaxed);
}

/// Releases one user. Returns true when the caller must unload the sink
/// because this was the last one; never underflows on a stray release.
/// Callers must hold `ScrcpyState::mic_sink_lock`, like the reservation path.
#[cfg(unix)]
fn release_mic_sink_user(users: &AtomicUsize) -> bool {
    let current = users.load(Ordering::Relaxed);
    if current == 0 {
        return false;
    }
    users.store(current - 1, Ordering::Relaxed);
    current == 1
}

/// Releases this session's reservation and unloads the sink when it was the
/// last user. Serialized with start-up, so a replacement can reserve its own
/// user before the previous session's monitor gets here.
#[cfg(unix)]
async fn release_mic_sink(state: &ScrcpyState, window: &Window) {
    let _guard = state.mic_sink_lock.lock().await;
    if release_mic_sink_user(&state.mic_sink_users) {
        unload_mic_sink(window).await;
    }
}

/// Bump the logical session generation for a device. run_scrcpy calls this
/// before replacing a previous process, so sidecars spawned for the old
/// session can see they are stale even when the registry key is re-occupied
/// by the replacement before their next liveness check.
#[cfg_attr(not(unix), allow(dead_code))]
fn bump_session_generation(generations: &mut HashMap<String, u64>, device: &str) -> u64 {
    let generation = generations.entry(device.to_string()).or_insert(0);
    *generation += 1;
    *generation
}

#[cfg_attr(not(unix), allow(dead_code))]
fn session_generation_is_current(
    generations: &HashMap<String, u64>,
    device: &str,
    generation: u64,
) -> bool {
    generations.get(device) == Some(&generation)
}

/// True when no replacement has taken over this device. A replaced session
/// must not publish a stop event: the new session already owns the UI state.
fn generation_is_current(app_handle: &tauri::AppHandle, device: &str, generation: u64) -> bool {
    let state = app_handle.state::<ScrcpyState>();
    let generations = state
        .session_generations
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    session_generation_is_current(&generations, device, generation)
}

/// Sidecar liveness: the device is still registered AND the generation this
/// loop was spawned with is still the current one. Both locks are taken one
/// after the other, never together, so this cannot deadlock with the
/// run_scrcpy paths (which also never hold both at once).
#[cfg_attr(not(unix), allow(dead_code))]
fn session_alive(app_handle: &tauri::AppHandle, device: &str, generation: u64) -> bool {
    let state = app_handle.state::<ScrcpyState>();
    let in_registry = state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(device);
    if !in_registry {
        return false;
    }
    let generations = state
        .session_generations
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    session_generation_is_current(&generations, device, generation)
}

// Watcher that keeps the scrcpy stream inside the virtual sink for the
// whole session. The microphone bridge also reports the current destination
// to the GUI; camera sessions only move the stream, because the `mic-route`
// event belongs to the bridge's MicMeter and must not carry a camera's state.
// PULSE_SINK does not reach SDL3 clients and the stream shows up only after
// scrcpy's server handshake (seconds later), so a one-shot check is not enough.
// Camera sessions pass a slower steady interval because they can stay open
// for hours; the mic bridge keeps the tighter default.
#[cfg(unix)]
async fn spawn_mic_route_watcher(
    window: Window,
    device: String,
    app_handle: tauri::AppHandle,
    session_pid: Arc<AtomicU32>,
    generation: u64,
    steady_interval: Duration,
    report_route: bool,
) {
    tokio::spawn(async move {
        let mut attempts: u32 = 0;
        // Route state of the latest poll we saw (None = nothing yet): drives
        // the pacing exactly like the change-only version did, without
        // gating the emit.
        let mut last_routed: Option<bool> = None;
        loop {
            if !session_alive(&app_handle, &device, generation) {
                break;
            }
            attempts += 1;
            let pid = session_pid.load(Ordering::Relaxed);
            if let Some((display, routed)) = poll_mic_route(&window, pid).await {
                last_routed = Some(routed);
                // Emit every pass instead of only on change: the GUI only
                // subscribes while the mic tab is mounted, so a remounted tab
                // would otherwise stay without destination info until the
                // route actually changes. Camera sessions skip this event:
                // a bridge on another device owns the MicMeter, and a camera
                // "searching" state must not overwrite its destination.
                if report_route {
                    let _ = window.emit("mic-route", (display, routed));
                }
            }
            // Fast cadence while hunting the fresh stream (the handshake
            // lands in ~1.3s) or while a routed stream is MIA (<10 attempts),
            // slow down once healthy or after the hunt window closes.
            let steady = last_routed == Some(true) || attempts > 10;
            if steady {
                tokio::time::sleep(steady_interval).await;
            } else {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    });
}

/// True when a `pactl list sink-inputs` block belongs to the scrcpy process
/// that owns the mic bridge. The PID is unique; the application name is not
/// reliable when the user points the app at a renamed custom binary.
#[cfg(unix)]
fn sink_input_is_scrcpy_pid(block: &str, pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let process = format!("application.process.id = \"{}\"", pid);
    block.lines().any(|line| line.trim() == process)
}

// One routing pass. Returns the destination for the GUI: (text, in_virtual)
// where empty text means "no scrcpy stream yet". Maintains the route when the
// stream landed on some other output.
#[cfg(unix)]
async fn poll_mic_route(window: &Window, scrcpy_pid: u32) -> Option<(String, bool)> {
    if scrcpy_pid == 0 {
        return Some((String::new(), false));
    }
    // Verbose sinks: entries start at "Sink #N" and carry Name/Description.
    let mut sink_map: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let sinks = pactl_run(&["list", "sinks"]).await?;
    for block in sinks.stdout.split("Sink #").skip(1) {
        let idx = block.lines().next().unwrap_or("").trim().to_string();
        if idx.is_empty() {
            continue;
        }
        let name = block
            .lines()
            .find(|l| l.trim().starts_with("Name: "))
            .and_then(|l| l.split_once("Name: ").map(|(_, v)| v.trim().to_string()));
        let desc = block
            .lines()
            .find(|l| l.trim().starts_with("Description: "))
            .and_then(|l| {
                l.split_once("Description: ")
                    .map(|(_, v)| v.trim().to_string())
            });
        if let Some(name) = name {
            sink_map.insert(idx, (name, desc.unwrap_or_default()));
        }
    }
    let (target_idx, _) = sink_map
        .iter()
        .find(|(_, (name, _))| name == MIC_SINK_NAME)
        .map(|(k, v)| (k.clone(), v.1.clone()))?;

    let inputs = pactl_run(&["list", "sink-inputs"]).await?;
    for block in inputs.stdout.split("\n\n") {
        if !sink_input_is_scrcpy_pid(block, scrcpy_pid) {
            continue;
        }
        // move-sink-input takes the CLIENT (stream) index first, then the
        // sink. Stream index lives in the block header: "Sink Input #1340".
        let stream_idx = block
            .lines()
            .next()
            .and_then(|l| l.trim().strip_prefix("Sink Input #"))
            .map(|v| v.trim().to_string());
        let current_idx = block
            .lines()
            .find(|l| l.trim().starts_with("Sink: "))
            .and_then(|l| l.split(':').nth(1).map(|v| v.trim().to_string()));
        let Some(stream_idx) = stream_idx else {
            return Some((String::new(), false));
        };
        let Some(current_idx) = current_idx else {
            return Some((String::new(), false));
        };
        let (cur_name, cur_desc) = sink_map
            .get(&current_idx)
            .cloned()
            .unwrap_or_else(|| (String::new(), String::new()));

        if cur_name == MIC_SINK_NAME {
            return Some((MIC_SINK_LABEL.to_string(), true));
        }
        // The stream went somewhere else: bring it into the virtual sink.
        let moved = pactl_run(&["move-sink-input", &stream_idx, &target_idx]).await;
        let ok = moved.as_ref().map(|m| m.ok).unwrap_or(false);
        let _ = window.emit(
            "scrcpy-log",
            if ok {
                "[MIC] Audio stream moved into the virtual sink.".to_string()
            } else {
                format!(
                    "[MIC] move-sink-input failed: {}.",
                    moved.map(|m| m.stderr).unwrap_or_default()
                )
            },
        );
        return Some((
            if ok {
                MIC_SINK_LABEL.to_string()
            } else {
                cur_desc
            },
            ok,
        ));
    }
    // scrcpy stream not there yet (server handshake in progress).
    Some((String::new(), false))
}

// Live level feedback: parec taps the virtual sink's monitor and the RMS of
// each 100 ms chunk is emitted to the GUI as dB. The loop ends when parec
// exits or the owning scrcpy session leaves the process registry.
#[cfg(unix)]
async fn spawn_mic_level_meter(
    window: Window,
    device: String,
    app_handle: tauri::AppHandle,
    generation: u64,
) {
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let monitor = format!("{}.monitor", MIC_SINK_NAME);
        let Some(mut child) = create_command("parec")
            .args([
                "-d",
                &monitor,
                "--format=s16le",
                "--channels=1",
                "--rate=44100",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()
        else {
            let _ = window.emit(
                "scrcpy-log",
                "[MIC] Level meter unavailable (parec not found).".to_string(),
            );
            return;
        };
        let _ = window.emit(
            "scrcpy-log",
            format!("[MIC] Level meter started (parec on {}).", monitor),
        );
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill().await;
            let _ = window.emit(
                "scrcpy-log",
                "[MIC] Level meter ended (no stdout).".to_string(),
            );
            return;
        };
        // 88200 bytes/s -> one 100 ms chunk at s16le mono 44.1 kHz.
        let mut buf = vec![0u8; 8820];
        let mut chunks: u32 = 0;
        loop {
            if !session_alive(&app_handle, &device, generation) {
                break;
            }
            // Generous timeout: chunks arrive ~every 100 ms while the stream
            // plays; a 3s drought means parec stalled.
            let read_next = timeout(Duration::from_secs(3), stdout.read(&mut buf));
            let n = match read_next.await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => n,
                Ok(Err(_)) | Err(_) => break,
            };
            chunks += 1;
            let samples: Vec<f64> = buf[..(n / 2) * 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| {
                    let v = i16::from_le_bytes(*c) as f64 / 32768.0;
                    v * v
                })
                .collect();
            let mean_sq = if samples.is_empty() {
                0.0
            } else {
                samples.iter().sum::<f64>() / samples.len() as f64
            };
            let rms = mean_sq.sqrt();
            let db = if rms < 0.0016 {
                -60.0
            } else {
                (20.0 * rms.log10()).floor().clamp(-60.0, 0.0)
            };
            let _ = window.emit("mic-level", db);
        }
        let _ = child.kill().await;
        let _ = window.emit(
            "scrcpy-log",
            format!("[MIC] Level meter ended after {} chunks.", chunks),
        );
    });
}

fn validate_scrcpy_config(config: &ScrcpyConfig) -> Result<(), String> {
    validate::device(&config.device)?;
    validate::video_codec(config.codec.as_deref())?;
    validate::audio_codec(config.audio_codec.as_deref())?;
    validate::mirror_audio_source(config.mirror_audio_source.as_deref())?;
    validate::mic_source(config.mic_source.as_deref())?;
    validate::mic_codec(config.mic_codec.as_deref())?;
    validate::rotation(config.rotation.as_deref())?;
    validate::fps(config.fps)?;
    validate::bitrate(config.bitrate)?;
    Ok(())
}

/// Mutable state owned by the session monitor between retries.
struct SessionRuntime {
    child: tokio::process::Child,
    audio_state: AudioErrorState,
    pid: Arc<AtomicU32>,
    first_window_pos: Option<(i32, i32)>,
    last_window_pos: Option<(i32, i32)>,
    chain_index: usize,
    capture_reported: bool,
    codec_terminal_reported: bool,
}

/// Backoff for automatic reconnection. The last delay repeats until the
/// session is stopped or the device comes back.
const RECONNECT_DELAYS_SECS: &[u64] = &[2, 4, 8, 16, 30];

fn reconnect_delay(attempt: u32) -> Duration {
    let index = (attempt as usize).min(RECONNECT_DELAYS_SECS.len() - 1);
    Duration::from_secs(RECONNECT_DELAYS_SECS[index])
}

/// Only an unexpected exit with no known capture/codec failure is retried.
/// `success == true` is a deliberate window close, and capture/codec failures
/// would repeat identically.
fn should_auto_reconnect(
    enabled: bool,
    has_serial: bool,
    success: bool,
    capture_failed: bool,
    codec_failed: bool,
) -> bool {
    enabled && has_serial && !success && !capture_failed && !codec_failed
}

/// Parses `adb devices` output. Only a ready device is online; `offline` and
/// `unauthorized` entries keep the reconnect loop waiting. `parse_adb_device_line`
/// handles mDNS serials that contain spaces.
fn adb_device_is_online(output: &str, serial: &str) -> bool {
    output.lines().any(|line| {
        parse_adb_device_line(line)
            .map(|(name, state)| name == serial && state == "device")
            .unwrap_or(false)
    })
}

async fn device_is_online(adb_exe_path: &str, serial: &str) -> bool {
    let mut command = create_command(adb_exe_path);
    command.arg("devices");
    let Ok(output) = process::output_with_timeout(command, Duration::from_secs(2)).await else {
        return false;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    output.status.success() && adb_device_is_online(&stdout, serial)
}

/// Finalizes a user-initiated stop. Also used while the monitor is in the
/// reconnect backoff, where the child has already exited.
async fn finish_stopped_session(
    runtime: &mut SessionRuntime,
    window: &Window,
    device: &str,
    requested: Option<(i32, i32)>,
    stop_position: Option<(i32, i32)>,
    publish: bool,
) {
    // Always terminate the child. Dropping a Tokio Child does not kill the
    // process, so a replacement must still reap the previous scrcpy even when
    // it must not publish a stop event for the new session.
    let graceful = match runtime.child.try_wait() {
        Ok(Some(_)) => true,
        _ => terminate_child(&mut runtime.child, Duration::from_secs(2)).await,
    };
    if !publish {
        return;
    }
    let _ = window.emit(
        "scrcpy-audio-status",
        json!({ "device": device, "status": "off" }),
    );
    emit_window_pos_and_stop(
        window,
        device,
        requested,
        runtime.first_window_pos,
        stop_position.or(runtime.last_window_pos),
        SessionEndReason::Stopped,
    );
    let _ = window.emit(
        "scrcpy-log",
        format!(
            "[SYSTEM] Session stopped ({}).",
            if graceful { "graceful" } else { "forced" }
        ),
    );
}

/// Returns true while this generation still owns the session. When the handle
/// was removed by a normal Stop, publishes the stop event; when a replacement
/// bumped the generation, leaves the UI untouched for the new session.
async fn finish_if_session_gone(
    runtime: &mut SessionRuntime,
    window: &Window,
    app_handle: &tauri::AppHandle,
    device: &str,
    generation: u64,
    requested: Option<(i32, i32)>,
) -> bool {
    if session_alive(app_handle, device, generation) {
        return true;
    }
    finish_stopped_session(
        runtime,
        window,
        device,
        requested,
        None,
        generation_is_current(app_handle, device, generation),
    )
    .await;
    false
}

/// Immutable session context needed to respawn scrcpy with another codec.
struct RetryContext<'a> {
    window: &'a Window,
    device: &'a str,
    exe_path: &'a str,
    adb_exe_path: &'a str,
    server_path: Option<&'a str>,
    config: &'a ScrcpyConfig,
    video_dir: &'a Option<String>,
    pulse_sink: Option<&'a str>,
    capabilities: &'a Capabilities,
    // Only read by the KWin bridge; the field stays for a single context shape.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    track_position: bool,
}

#[derive(Clone, Copy)]
enum RetryOrigin {
    Running,
    Exited,
}

fn release_starting_device(state: &ScrcpyState, device: &str) {
    state
        .starting_devices
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(device);
}

/// Replaces the current child with one using `next_codec`, updating the PID,
/// KWin tracking and position samples. Callers handle the error path because
/// they own the stop notification.
async fn retry_audio_codec(
    runtime: &mut SessionRuntime,
    ctx: &RetryContext<'_>,
    origin: RetryOrigin,
    next_codec: &'static str,
) -> Result<(), String> {
    let _ = ctx.window.emit(
        "scrcpy-log",
        format!(
            "[SYSTEM] Default audio codec failed, retrying with {}...",
            next_codec.to_uppercase()
        ),
    );
    let _ = ctx.window.emit(
        "scrcpy-audio-status",
        json!({ "device": ctx.device, "status": "warning" }),
    );

    #[cfg(target_os = "linux")]
    let old_pid = match origin {
        RetryOrigin::Running => runtime.child.id().unwrap_or(0),
        RetryOrigin::Exited => runtime.pid.load(Ordering::Relaxed),
    };
    if matches!(origin, RetryOrigin::Running) {
        let _ = terminate_child(&mut runtime.child, Duration::from_secs(2)).await;
    }

    let new_args = build_scrcpy_args(ctx.config, ctx.video_dir.clone(), Some(next_codec));
    let (new_args, _) = crate::capabilities::filter_args(new_args, ctx.capabilities);
    let (new_child, new_state) = spawn_scrcpy_streams(
        ctx.window,
        ctx.exe_path,
        ctx.adb_exe_path,
        ctx.server_path,
        &new_args,
        ctx.config.vsync.unwrap_or(true),
        ctx.pulse_sink,
    )
    .await?;

    let new_pid = new_child.id().unwrap_or(0);
    runtime.pid.store(new_pid, Ordering::Relaxed);
    #[cfg(target_os = "linux")]
    if ctx.track_position {
        crate::kwin::stop_tracking(old_pid);
        crate::kwin::track_window(new_pid, ctx.config.window_x.zip(ctx.config.window_y));
    }
    runtime.child = new_child;
    runtime.audio_state = new_state;
    // New window: re-learn its decoration offset by resampling from scratch.
    runtime.first_window_pos = None;
    runtime.last_window_pos = None;
    runtime.capture_reported = false;
    runtime.chain_index += 1;
    let _ = ctx.window.emit(
        "scrcpy-audio-status",
        json!({ "device": ctx.device, "status": "ok" }),
    );
    Ok(())
}

#[tauri::command]
pub async fn run_scrcpy(
    window: Window,
    state: State<'_, ScrcpyState>,
    config: ScrcpyConfig,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    validate_scrcpy_config(&config)?;
    let mut config = config;

    // Reserve the device before any await. Without this, two concurrent
    // invocations can both see no session, spawn two children, and let the
    // last registry insert overwrite (and orphan) the first. The same lock
    // lets download_scrcpy refuse a swap while a start is being prepared.
    {
        let _swap_guard = state.binary_swap.lock().await;
        let mut starting = state
            .starting_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !starting.insert(config.device.clone()) {
            return Err("A session for this device is already starting".to_string());
        }
    }

    let video_dir = app_handle
        .path()
        .video_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());

    let exe_path = get_binary_path("scrcpy", config.scrcpy_path.clone());

    // Ask this exact binary which options it supports. If the help output is
    // unrecognizable, detection stays permissive and the real scrcpy error is
    // allowed through.
    let capabilities = crate::capabilities::detect(&exe_path).await;
    if let Err(error) = ensure_mode_supported(&config, &capabilities) {
        release_starting_device(state.inner(), &config.device);
        return Err(error);
    }
    if let Some(message) = normalize_mic_source(&mut config, &capabilities) {
        let _ = window.emit("scrcpy-log", message);
    }
    if let Some(message) = normalize_mirror_audio(&mut config, &capabilities) {
        let _ = window.emit("scrcpy-log", message);
    }
    let capabilities = Arc::new(capabilities);

    // Log the session details for the user
    let mode_label = match config.session_mode.as_str() {
        "camera" => "Camera Mode",
        "desktop" => "Desktop Mode",
        "mic" => "Microphone Bridge",
        _ => "Screen Mirroring",
    };

    let _ = window.emit(
        "scrcpy-log",
        format!("[SYSTEM] Starting {} session...", mode_label),
    );
    if config.session_mode == "mic" {
        let _ = window.emit("scrcpy-log", format!("[SYSTEM] Target: {}", config.device));
        let source = config.mic_source.as_deref().unwrap_or("").trim();
        let source = if source.is_empty() { "mic" } else { source };
        let source = validate::normalize_mic_source(source);
        let _ = window.emit(
            "scrcpy-log",
            format!(
                "[SYSTEM] Audio-only bridge (--no-window --no-video --audio-source={}).",
                source
            ),
        );
    } else {
        let res_label = config
            .res
            .as_ref()
            .map(|r| if r == "0" { "Original" } else { r })
            .unwrap_or("Original");
        let bitrate_label = format!("{}Mbps", config.bitrate.unwrap_or(8));
        let fps_label = format!("{}fps", config.fps.unwrap_or(60));
        let _ = window.emit(
            "scrcpy-log",
            format!(
                "[SYSTEM] Target: {} | Config: {} @ {}, {}",
                config.device, res_label, bitrate_label, fps_label
            ),
        );
        if config.session_mode == "mirror" && config.audio_enabled.unwrap_or(true) {
            let source = config
                .mirror_audio_source
                .as_deref()
                .unwrap_or("output")
                .trim();
            let dup = source == "playback" && config.audio_dup.unwrap_or(false);
            let _ = window.emit(
                "scrcpy-log",
                format!(
                    "[SYSTEM] Audio source: {}{}.",
                    source,
                    if dup { " (kept on device)" } else { "" }
                ),
            );
        }
    }

    if config.record.unwrap_or(false) {
        let path = config
            .record_path
            .as_ref()
            .map(|p| if p.is_empty() { "Videos" } else { p })
            .unwrap_or("Videos");
        let _ = window.emit(
            "scrcpy-log",
            format!("[SYSTEM] Recording enabled -> output to {}", path),
        );
    }

    let adb_exe_path = get_binary_path("adb", config.scrcpy_path.clone());
    let server_path = if !exe_path.is_empty() && exe_path != "scrcpy" {
        Path::new(&exe_path)
            .parent()
            .map(|p| p.join("scrcpy-server").to_string_lossy().to_string())
    } else {
        None
    };

    let _ = window.emit("scrcpy-log", format!("[SYSTEM] Using scrcpy: {}", exe_path));
    let _ = window.emit(
        "scrcpy-log",
        format!("[SYSTEM] Using adb: {}", adb_exe_path),
    );

    // Bump the logical session generation BEFORE replacing the previous
    // process. Mic sidecars check this once per pass; the monitor task also
    // uses it to avoid deleting a newer session's registry entry.
    let session_generation = {
        let mut generations = state
            .session_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        bump_session_generation(&mut generations, &config.device)
    };

    // One session per device: stop the previous one before spawning the new
    // child, so a fast double-start cannot leave an untracked process behind.
    let existing_to_replace = state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&config.device);
    if let Some(existing) = existing_to_replace {
        let _ = window.emit(
            "scrcpy-log",
            "[SYSTEM] Replacing the previous session for this device.".to_string(),
        );
        let notified = existing.done.notified();
        let _ = existing.stop.send(StopRequest { position: None });
        if tokio::time::timeout(Duration::from_secs(3), notified)
            .await
            .is_err()
        {
            force_kill_pid(existing.pid.load(Ordering::Relaxed));
        }
    }

    // `--turn-screen-off` is applied right after the connection is set up, so
    // the display must already be on when scrcpy starts. Otherwise, on slower
    // devices, scrcpy's own asynchronous power-on can land after its
    // power-off request and leave the phone screen on until the user wakes it
    // manually. Wake it here, after any previous session has been stopped, so
    // the power-off request is the last one to reach the device.
    if wakes_before_screen_off(&config) {
        wake_display_before_screen_off(&window, &adb_exe_path, &config.device).await;
    }

    let audio_enabled = config.audio_enabled.unwrap_or(true);
    let wants_virtual_mic = wants_virtual_mic(&config);

    // Virtual microphone on Linux: create/refresh the shared sink and hand
    // the scrcpy client to it through the libpulse default-sink env. Camera
    // mode can use the same route, so webcam video and a selectable phone
    // microphone come from one session. Other platforms get an honest note.
    // Start-up and teardown take `mic_sink_lock` so the user count cannot be
    // overtaken by a session stopping at the same time.
    #[cfg(unix)]
    let (pulse_sink, uses_mic_sink): (Option<String>, bool) = if wants_virtual_mic {
        let _sink_guard = state.mic_sink_lock.lock().await;
        // Remove a sink left behind by a crashed session. Only the first
        // active user does this; later users reuse the live sink.
        if state.mic_sink_users.load(Ordering::Relaxed) == 0 {
            unload_mic_sink(&window).await;
        }
        let denoise = config.session_mode == "mic" && config.mic_denoise.unwrap_or(false);
        if ensure_mic_sink(&window, denoise).await {
            reserve_mic_sink_user(&state.mic_sink_users);
            (Some(MIC_SINK_NAME.to_string()), true)
        } else {
            (None, false)
        }
    } else {
        (None, false)
    };

    #[cfg(not(unix))]
    let pulse_sink: Option<String> = if wants_virtual_mic {
        let _ = window.emit(
            "scrcpy-log",
            "[MIC] Virtual-microphone routing requires Linux; audio goes to the default output."
                .to_string(),
        );
        None
    } else {
        None
    };

    // First attempt: honour the user's audio codec choice. In "auto" mode this
    // launches without --audio-codec, matching the previous default behaviour.
    let (initial_args, skipped) = crate::capabilities::filter_args(
        build_scrcpy_args(&config, video_dir.clone(), None),
        &capabilities,
    );
    if let Some(message) = crate::capabilities::skipped_message(&capabilities, &skipped) {
        let _ = window.emit("scrcpy-log", message);
    }

    let spawned = spawn_scrcpy_streams(
        &window,
        &exe_path,
        &adb_exe_path,
        server_path.as_deref(),
        &initial_args,
        config.vsync.unwrap_or(true),
        pulse_sink.as_deref(),
    )
    .await;
    let (child, audio_state) = match spawned {
        Ok(spawned) => spawned,
        Err(e) => {
            // The sink was reserved before the spawn attempt; release it
            // before allowing another start for this device.
            #[cfg(unix)]
            if uses_mic_sink {
                release_mic_sink(state.inner(), &window).await;
            }
            release_starting_device(state.inner(), &config.device);
            return Err(e);
        }
    };

    // Decide whether automatic audio codec fallback should kick in on errors.
    let audio_codec_mode = config.audio_codec.as_deref().unwrap_or("auto").trim();
    let should_auto_fallback = audio_enabled
        && (audio_codec_mode.is_empty() || audio_codec_mode.eq_ignore_ascii_case("auto"));

    let initial_scrcpy_pid = child.id().unwrap_or(0);
    // Only windowed mirror sessions have a persisted position; camera, desktop
    // and fullscreen windows must not inherit it.
    let track_position = tracks_window_position(&config);
    // KDE Wayland: let KWin report (and, on launch, apply) the mirror window
    // position. On X11 this is a no-op and xdotool remains in charge.
    #[cfg(target_os = "linux")]
    if track_position {
        crate::kwin::track_window(initial_scrcpy_pid, config.window_x.zip(config.window_y));
    }
    let session_pid = Arc::new(AtomicU32::new(initial_scrcpy_pid));
    let session_done = Arc::new(Notify::new());
    let (stop_tx, stop_rx) = oneshot::channel();
    state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            config.device.clone(),
            SessionHandle {
                generation: session_generation,
                track_position,
                pid: session_pid.clone(),
                stop: stop_tx,
                done: session_done.clone(),
            },
        );
    release_starting_device(state.inner(), &config.device);
    let _ = window.emit(
        "scrcpy-status",
        json!({ "device": config.device, "running": true }),
    );
    // The audio badge starts optimistic; log detection flips it to a warning
    // (or an error with --require-audio) if capture/codec trouble shows up.
    let _ = window.emit(
        "scrcpy-audio-status",
        json!({
            "device": config.device,
            "status": if audio_enabled { "ok" } else { "off" }
        }),
    );

    // Virtual-sink sidecars. Started only after the registry insert above, so
    // their contains_key liveness checks see this session on the first poll
    // (spawning them earlier raced the insert and the loops died instantly).
    // The route watcher is needed for camera sessions too: SDL3 does not
    // reliably honour PULSE_SINK, so the stream is moved after it appears.
    #[cfg(unix)]
    if uses_mic_sink {
        tokio::spawn(spawn_mic_route_watcher(
            window.clone(),
            config.device.clone(),
            app_handle.clone(),
            session_pid.clone(),
            session_generation,
            if config.session_mode == "camera" {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(2)
            },
            config.session_mode == "mic",
        ));
        if config.session_mode == "mic" {
            tokio::spawn(spawn_mic_level_meter(
                window.clone(),
                config.device.clone(),
                app_handle.clone(),
                session_generation,
            ));
        }
    }

    // Monitor for exit (and orchestrate the audio codec fallback chain)
    let device_mon = config.device.clone();
    let window_mon = window.clone();
    let app_handle_mon = window.app_handle().clone();
    let video_dir_mon = video_dir;
    let exe_path_mon = exe_path;
    let adb_exe_path_mon = adb_exe_path;
    let server_path_mon = server_path;
    let config_mon = config.clone();
    let pulse_sink_mon = pulse_sink;

    // Position we asked scrcpy to open at this launch (if any). Used to learn
    // the decoration offset so the saved position round-trips exactly. See
    // resolve_window_pos_to_persist.
    let requested_pos: Option<(i32, i32)> = match (config_mon.window_x, config_mon.window_y) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => None,
    };
    // `track_position` (computed before the child was tracked) keeps sampling
    // limited to windowed mirror sessions.

    tokio::spawn(async move {
        let mut runtime = SessionRuntime {
            child,
            audio_state,
            pid: session_pid.clone(),
            first_window_pos: None,
            last_window_pos: None,
            chain_index: 0,
            // Capture and terminal codec failures do not improve on retry, so
            // report each one once per attempt instead of every 2 s tick.
            capture_reported: false,
            codec_terminal_reported: false,
        };
        let retry_ctx = RetryContext {
            window: &window_mon,
            device: &device_mon,
            exe_path: &exe_path_mon,
            adb_exe_path: &adb_exe_path_mon,
            server_path: server_path_mon.as_deref(),
            config: &config_mon,
            video_dir: &video_dir_mon,
            pulse_sink: pulse_sink_mon.as_deref(),
            capabilities: &capabilities,
            track_position,
        };
        let mut stop_rx = stop_rx;
        let mut reconnect_attempt: u32 = 0;
        let mut healthy_ticks: u32 = 0;

        let emit_audio_status = |status: &str| {
            let _ = window_mon.emit(
                "scrcpy-audio-status",
                json!({ "device": &device_mon, "status": status }),
            );
        };
        let capture_message = || {
            if config_mon.require_audio.unwrap_or(false) {
                "[AUDIO] Audio capture failed and \"require audio\" is enabled: the session stops. Unlock the device (Android 11) and use Android 12+; some apps block audio capture."
            } else {
                "[AUDIO] Audio capture failed on the device; video keeps running without sound. Android 11+ is required, unlock the device on Android 11, and some apps block audio capture."
            }
        };

        'monitor: loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(2000)) => {}
                request = &mut stop_rx => {
                    let position = match request {
                        Ok(request) => request.position,
                        Err(_) => None,
                    };
                    finish_stopped_session(
                        &mut runtime,
                        &window_mon,
                        &device_mon,
                        requested_pos,
                        position,
                        generation_is_current(&app_handle_mon, &device_mon, session_generation),
                    )
                    .await;
                    break 'monitor;
                }
            }

            // Audio can fail while scrcpy keeps mirroring the video, so the
            // process may never exit. React while it is still running instead
            // of only checking on exit (which made the failure silent).
            if runtime.audio_state.capture_failed() {
                if !runtime.capture_reported {
                    runtime.capture_reported = true;
                    let _ = window_mon.emit("scrcpy-log", capture_message().to_string());
                    emit_audio_status(if config_mon.require_audio.unwrap_or(false) {
                        "error"
                    } else {
                        "warning"
                    });
                }
            } else if runtime.audio_state.codec_failed() {
                if !codec_failure_is_terminal(should_auto_fallback, runtime.chain_index) {
                    let next_codec = AUDIO_FALLBACK_CHAIN[runtime.chain_index];
                    match retry_audio_codec(
                        &mut runtime,
                        &retry_ctx,
                        RetryOrigin::Running,
                        next_codec,
                    )
                    .await
                    {
                        Ok(()) => continue,
                        Err(e) => {
                            let _ = window_mon.emit(
                                "scrcpy-log",
                                format!("[SYSTEM] Failed to spawn audio retry: {}", e),
                            );
                            emit_audio_status("error");
                            emit_window_pos_and_stop(
                                &window_mon,
                                &device_mon,
                                requested_pos,
                                runtime.first_window_pos,
                                runtime.last_window_pos,
                                SessionEndReason::Error,
                            );
                            break;
                        }
                    }
                } else if !runtime.codec_terminal_reported {
                    runtime.codec_terminal_reported = true;
                    let _ = window_mon.emit(
                        "scrcpy-log",
                        codec_failure_message(should_auto_fallback).to_string(),
                    );
                    emit_audio_status("error");
                }
            }

            // Sample the live window position on every tick. This is the only
            // path that can catch a move made right before the user closes the
            // mirror window itself (rather than through stop_scrcpy, which
            // grabs its own guaranteed-fresh sample): SDL destroys the window
            // before the process exits, so by the time that exit is detected
            // below the position is unreadable, and this periodic sample is
            // all that is left to persist. Two seconds is a good trade: on
            // Linux the sample spawns xdotool, on macOS an osascript call,
            // and the stop path takes its own fresh sample anyway.
            if track_position {
                if let Some(pid) = runtime.child.id() {
                    if let Some(pos) = try_capture_window_pos(pid) {
                        if runtime.first_window_pos.is_none() {
                            runtime.first_window_pos = Some(pos);
                        }
                        runtime.last_window_pos = Some(pos);
                    }
                }
            }

            let outcome = match runtime.child.try_wait() {
                Ok(Some(status)) => AttemptOutcome::Exited {
                    description: status.to_string(),
                    success: status.success(),
                },
                Ok(None) => {
                    healthy_ticks = healthy_ticks.saturating_add(1);
                    if healthy_ticks >= 5 {
                        reconnect_attempt = 0;
                    }
                    continue;
                }
                Err(e) => AttemptOutcome::WaitError(e.to_string()),
            };

            match outcome {
                AttemptOutcome::WaitError(e) => {
                    let _ = window_mon.emit(
                        "scrcpy-log",
                        format!("[SYSTEM] Error waiting for scrcpy: {}", e),
                    );
                    emit_audio_status("off");
                    emit_window_pos_and_stop(
                        &window_mon,
                        &device_mon,
                        requested_pos,
                        runtime.first_window_pos,
                        runtime.last_window_pos,
                        SessionEndReason::Error,
                    );
                    break;
                }
                AttemptOutcome::Exited {
                    description: status,
                    success,
                } => {
                    let _ = window_mon.emit(
                        "scrcpy-log",
                        format!("[SYSTEM] Scrcpy process exited with status: {}", status),
                    );

                    // Give stream reader tasks a moment to flush remaining stderr
                    // lines (the audio error pattern may still be in flight).
                    tokio::time::sleep(Duration::from_millis(250)).await;

                    let capture_failed = runtime.audio_state.capture_failed();
                    let codec_failed = runtime.audio_state.codec_failed();

                    if capture_failed {
                        // Changing codecs cannot fix a device-side capture
                        // failure, so stop instead of burning the fallback chain.
                        if !runtime.capture_reported {
                            let _ = window_mon.emit("scrcpy-log", capture_message().to_string());
                        }
                        emit_audio_status(if config_mon.require_audio.unwrap_or(false) {
                            "error"
                        } else {
                            "warning"
                        });
                        emit_window_pos_and_stop(
                            &window_mon,
                            &device_mon,
                            requested_pos,
                            runtime.first_window_pos,
                            runtime.last_window_pos,
                            SessionEndReason::Error,
                        );
                        break;
                    }

                    if codec_failed {
                        if !codec_failure_is_terminal(should_auto_fallback, runtime.chain_index) {
                            let next_codec = AUDIO_FALLBACK_CHAIN[runtime.chain_index];
                            match retry_audio_codec(
                                &mut runtime,
                                &retry_ctx,
                                RetryOrigin::Exited,
                                next_codec,
                            )
                            .await
                            {
                                Ok(()) => continue,
                                Err(e) => {
                                    let _ = window_mon.emit(
                                        "scrcpy-log",
                                        format!("[SYSTEM] Failed to spawn audio retry: {}", e),
                                    );
                                    emit_audio_status("error");
                                    emit_window_pos_and_stop(
                                        &window_mon,
                                        &device_mon,
                                        requested_pos,
                                        runtime.first_window_pos,
                                        runtime.last_window_pos,
                                        SessionEndReason::Error,
                                    );
                                    break;
                                }
                            }
                        }

                        if !runtime.codec_terminal_reported {
                            let _ = window_mon.emit(
                                "scrcpy-log",
                                codec_failure_message(should_auto_fallback).to_string(),
                            );
                        }
                        emit_audio_status("error");
                    } else {
                        emit_audio_status("off");
                    }

                    healthy_ticks = 0;
                    if should_auto_reconnect(
                        config_mon.auto_reconnect.unwrap_or(false),
                        !device_mon.is_empty(),
                        success,
                        capture_failed,
                        codec_failed,
                    ) {
                        if !finish_if_session_gone(
                            &mut runtime,
                            &window_mon,
                            &app_handle_mon,
                            &device_mon,
                            session_generation,
                            requested_pos,
                        )
                        .await
                        {
                            break 'monitor;
                        }
                        if reconnect_attempt == 0 {
                            let _ = window_mon.emit(
                                "scrcpy-log",
                                "[RECONNECT] scrcpy exited unexpectedly; trying to restore the session."
                                    .to_string(),
                            );
                        }
                        loop {
                            let attempt = reconnect_attempt.saturating_add(1);
                            let delay = reconnect_delay(reconnect_attempt);
                            reconnect_attempt = attempt;
                            let _ = window_mon.emit(
                                "scrcpy-log",
                                format!(
                                    "[RECONNECT] Retrying in {}s (attempt {}).",
                                    delay.as_secs(),
                                    attempt
                                ),
                            );
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {}
                                request = &mut stop_rx => {
                                    let position = match request {
                                        Ok(request) => request.position,
                                        Err(_) => None,
                                    };
                                    finish_stopped_session(
                                        &mut runtime,
                                        &window_mon,
                                        &device_mon,
                                        requested_pos,
                                        position,
                                        generation_is_current(
                                            &app_handle_mon,
                                            &device_mon,
                                            session_generation,
                                        ),
                                    )
                                    .await;
                                    break 'monitor;
                                }
                            }
                            if !finish_if_session_gone(
                                &mut runtime,
                                &window_mon,
                                &app_handle_mon,
                                &device_mon,
                                session_generation,
                                requested_pos,
                            )
                            .await
                            {
                                break 'monitor;
                            }
                            if !device_is_online(&adb_exe_path_mon, &device_mon).await {
                                let _ = window_mon.emit(
                                    "scrcpy-log",
                                    format!(
                                        "[RECONNECT] Device {} is not available yet; waiting.",
                                        device_mon
                                    ),
                                );
                                continue;
                            }
                            // A replacement or a manual stop may have happened
                            // while `adb devices` was running.
                            if !finish_if_session_gone(
                                &mut runtime,
                                &window_mon,
                                &app_handle_mon,
                                &device_mon,
                                session_generation,
                                requested_pos,
                            )
                            .await
                            {
                                break 'monitor;
                            }

                            let reconnect_args =
                                build_scrcpy_args(&config_mon, video_dir_mon.clone(), None);
                            let (reconnect_args, skipped) =
                                crate::capabilities::filter_args(reconnect_args, &capabilities);
                            if let Some(message) =
                                crate::capabilities::skipped_message(&capabilities, &skipped)
                            {
                                let _ = window_mon.emit("scrcpy-log", message);
                            }
                            match spawn_scrcpy_streams(
                                &window_mon,
                                &exe_path_mon,
                                &adb_exe_path_mon,
                                server_path_mon.as_deref(),
                                &reconnect_args,
                                config_mon.vsync.unwrap_or(true),
                                pulse_sink_mon.as_deref(),
                            )
                            .await
                            {
                                Ok((new_child, new_state)) => {
                                    let new_pid = new_child.id().unwrap_or(0);
                                    #[cfg(target_os = "linux")]
                                    if track_position {
                                        crate::kwin::stop_tracking(
                                            runtime.pid.load(Ordering::Relaxed),
                                        );
                                        crate::kwin::track_window(
                                            new_pid,
                                            config_mon.window_x.zip(config_mon.window_y),
                                        );
                                    }
                                    runtime.pid.store(new_pid, Ordering::Relaxed);
                                    runtime.child = new_child;
                                    runtime.audio_state = new_state;
                                    runtime.first_window_pos = None;
                                    runtime.last_window_pos = None;
                                    runtime.capture_reported = false;
                                    runtime.codec_terminal_reported = false;
                                    runtime.chain_index = 0;
                                    emit_audio_status(if audio_enabled { "ok" } else { "off" });
                                    let _ = window_mon.emit(
                                        "scrcpy-log",
                                        format!(
                                            "[RECONNECT] Session restored after {} attempt(s).",
                                            attempt
                                        ),
                                    );
                                    break;
                                }
                                Err(e) => {
                                    let _ = window_mon.emit(
                                        "scrcpy-log",
                                        format!("[RECONNECT] Attempt {} failed: {}", attempt, e),
                                    );
                                }
                            }
                        }
                        continue 'monitor;
                    }

                    // A terminal codec failure is an error even when the
                    // process happens to exit with status 0.
                    let reason = if codec_failed {
                        SessionEndReason::Error
                    } else {
                        SessionEndReason::Exited { success }
                    };
                    emit_window_pos_and_stop(
                        &window_mon,
                        &device_mon,
                        requested_pos,
                        runtime.first_window_pos,
                        runtime.last_window_pos,
                        reason,
                    );
                    break;
                }
            }
        }

        // Remove only this generation's entry: a newer session may already own
        // the key after a replacement.
        {
            let state_lock = app_handle_mon.state::<ScrcpyState>();
            let mut sessions = state_lock
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if sessions.get(&device_mon).map(|s| s.generation) == Some(session_generation) {
                sessions.remove(&device_mon);
            }
        }
        // Release this session's sink reservation even when a replacement
        // already bumped the generation: the replacement has its own
        // reservation, so the count decides when the sink can be unloaded.
        #[cfg(unix)]
        if uses_mic_sink {
            let state_lock = app_handle_mon.state::<ScrcpyState>();
            release_mic_sink(state_lock.inner(), &window_mon).await;
        }
        #[cfg(target_os = "linux")]
        if track_position {
            crate::kwin::stop_tracking(runtime.pid.load(Ordering::Relaxed));
        }
        runtime.pid.store(0, Ordering::Relaxed);
        session_done.notify_one();
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_end_payload_distinguishes_stop_exit_and_error() {
        let stopped = session_end_payload("device1", SessionEndReason::Stopped);
        assert_eq!(stopped["device"], "device1");
        assert_eq!(stopped["running"], false);
        assert_eq!(stopped["reason"], "stopped");
        assert!(stopped.get("success").is_none());

        let crashed = session_end_payload("device1", SessionEndReason::Exited { success: false });
        assert_eq!(crashed["reason"], "exited");
        assert_eq!(crashed["success"], false);

        let closed = session_end_payload("device1", SessionEndReason::Exited { success: true });
        assert_eq!(closed["success"], true);

        let error = session_end_payload("device1", SessionEndReason::Error);
        assert_eq!(error["reason"], "error");
        assert!(error.get("success").is_none());
    }

    #[test]
    fn reconnect_backoff_grows_and_caps() {
        assert_eq!(reconnect_delay(0), Duration::from_secs(2));
        assert_eq!(reconnect_delay(1), Duration::from_secs(4));
        assert_eq!(reconnect_delay(2), Duration::from_secs(8));
        assert_eq!(reconnect_delay(3), Duration::from_secs(16));
        assert_eq!(reconnect_delay(4), Duration::from_secs(30));
        assert_eq!(reconnect_delay(99), Duration::from_secs(30));
    }

    #[test]
    fn auto_reconnect_only_for_unexpected_plain_exits() {
        assert!(should_auto_reconnect(true, true, false, false, false));
        assert!(!should_auto_reconnect(false, true, false, false, false));
        assert!(!should_auto_reconnect(true, false, false, false, false));
        assert!(!should_auto_reconnect(true, true, true, false, false));
        assert!(!should_auto_reconnect(true, true, false, true, false));
        assert!(!should_auto_reconnect(true, true, false, false, true));
    }

    #[test]
    fn adb_devices_parser_requires_a_ready_device() {
        let output = "List of devices attached\n192.168.1.5:5555\tdevice product:a05m model:SM_A055M\nusb123\tunauthorized\nemulator-5554\toffline\nadb-ZY22MGW35T-Z3uXXq (2)._adb-tls-connect._tcp\tdevice\n";
        assert!(adb_device_is_online(output, "192.168.1.5:5555"));
        assert!(!adb_device_is_online(output, "usb123"));
        assert!(!adb_device_is_online(output, "emulator-5554"));
        assert!(!adb_device_is_online(output, "missing"));
        assert!(adb_device_is_online(
            output,
            "adb-ZY22MGW35T-Z3uXXq (2)._adb-tls-connect._tcp"
        ));
    }

    #[test]
    fn session_generation_bumps_per_run_and_rejects_stale_sidecars() {
        let mut generations = HashMap::new();
        let first = bump_session_generation(&mut generations, "device1");
        assert_eq!(first, 1);
        assert!(session_generation_is_current(
            &generations,
            "device1",
            first
        ));

        // A replacement bumps the generation: the previous session's sidecars
        // are stale even though the registry key gets reused right away.
        let second = bump_session_generation(&mut generations, "device1");
        assert_eq!(second, 2);
        assert!(!session_generation_is_current(
            &generations,
            "device1",
            first
        ));
        assert!(session_generation_is_current(
            &generations,
            "device1",
            second
        ));

        // Generations are per device.
        assert_eq!(bump_session_generation(&mut generations, "device2"), 1);
        assert!(session_generation_is_current(
            &generations,
            "device1",
            second
        ));
    }

    #[cfg(unix)]
    #[test]
    fn sink_input_filter_requires_the_scrcpy_pid() {
        // A custom binary can call itself "scrcpy-custom"; the PID is still
        // the authoritative link to the session.
        let ours = "Sink Input #12\n  Properties:\n    application.name = \"scrcpy-custom\"\n    application.process.id = \"4242\"\n";
        let other = "Sink Input #13\n  Properties:\n    application.name = \"scrcpy\"\n    application.process.id = \"9999\"\n";
        assert!(sink_input_is_scrcpy_pid(ours, 4242));
        assert!(!sink_input_is_scrcpy_pid(other, 4242));
        assert!(!sink_input_is_scrcpy_pid(ours, 0));
    }

    #[test]
    fn session_generation_rejects_unknown_devices() {
        let generations = HashMap::new();
        assert!(!session_generation_is_current(&generations, "device1", 1));
    }

    #[cfg(unix)]
    async fn pid_alive(pid: u32) -> bool {
        StdCommand::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminate_child_escalates_when_sigterm_is_ignored() {
        // `exec` keeps the ignored SIGTERM disposition, so only the forced
        // kill can end this process. The readiness line proves the trap is
        // installed before the test signals: killing too early can beat the
        // shell's startup and make the test pass or fail by luck.
        let mut child = TokioCommand::new("sh")
            .args(["-c", "trap '' TERM; echo ready; exec sleep 30"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        let pid = child.id().expect("pid");
        let stdout = child.stdout.take().expect("stdout");
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .await
            .expect("readiness line");
        assert_eq!(line.trim(), "ready");

        let graceful = terminate_child(&mut child, Duration::from_millis(300)).await;

        assert!(!graceful);
        assert!(!pid_alive(pid).await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn force_kill_pid_terminates_the_process() {
        let mut child = TokioCommand::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        let pid = child.id().expect("pid");

        force_kill_pid(pid);

        let status = child.wait().await.expect("wait");
        assert!(!status.success());
        assert!(!pid_alive(pid).await);
    }

    fn base_config(session_mode: &str) -> ScrcpyConfig {
        ScrcpyConfig {
            device: "device1".to_string(),
            session_mode: session_mode.to_string(),
            bitrate: None,
            fps: None,
            stay_awake: None,
            turn_off: None,
            audio_enabled: None,
            audio_codec: None,
            mirror_audio_source: None,
            audio_dup: None,
            require_audio: None,
            always_on_top: None,
            fullscreen: None,
            borderless: None,
            record: None,
            record_path: None,
            scrcpy_path: None,
            otg_pure: None,
            camera_facing: None,
            camera_id: None,
            codec: None,
            camera_ar: None,
            camera_high_speed: None,
            vd_width: None,
            vd_height: None,
            vd_dpi: None,
            rotation: None,
            res: None,
            hid_keyboard: None,
            hid_mouse: None,
            render_driver: None,
            flex_display: None,
            camera_torch: None,
            camera_zoom: None,
            camera_mic_sink: None,
            background_color: None,
            keep_active: None,
            auto_reconnect: None,
            vsync: None,
            mic_source: None,
            mic_codec: None,
            mic_bitrate: None,
            mic_denoise: None,
            window_x: None,
            window_y: None,
            ignore_video_encoder_constraints: None,
        }
    }

    #[test]
    fn camera_can_route_its_microphone_into_the_virtual_sink() {
        let mut camera = base_config("camera");
        assert!(!wants_virtual_mic(&camera));

        camera.camera_mic_sink = Some(true);
        assert!(wants_virtual_mic(&camera));

        // Turning audio off removes the route even when the saved toggle is on.
        camera.audio_enabled = Some(false);
        assert!(!wants_virtual_mic(&camera));

        assert!(!wants_virtual_mic(&base_config("mirror")));
        assert!(wants_virtual_mic(&base_config("mic")));
    }

    #[cfg(unix)]
    #[test]
    fn virtual_sink_unloads_only_after_the_last_user_releases_it() {
        let users = AtomicUsize::new(0);
        // A stray release must not underflow or ask for an unload.
        assert!(!release_mic_sink_user(&users));
        assert_eq!(users.load(Ordering::Relaxed), 0);

        reserve_mic_sink_user(&users);
        reserve_mic_sink_user(&users);
        assert!(!release_mic_sink_user(&users));
        assert_eq!(users.load(Ordering::Relaxed), 1);
        assert!(release_mic_sink_user(&users));
        assert_eq!(users.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_validate_scrcpy_config_accepts_defaults_and_rejects_bad_values() {
        assert!(validate_scrcpy_config(&base_config("mirror")).is_ok());

        let mut config = base_config("mirror");
        config.codec = Some("vp9".to_string());
        assert!(validate_scrcpy_config(&config).is_err());

        let mut config = base_config("mirror");
        config.device = "-s".to_string();
        assert!(validate_scrcpy_config(&config).is_err());

        let mut config = base_config("mic");
        config.mic_codec = Some("flac".to_string());
        assert!(validate_scrcpy_config(&config).is_err());

        let mut config = base_config("mirror");
        config.fps = Some(0);
        assert!(validate_scrcpy_config(&config).is_err());

        let mut config = base_config("mirror");
        config.mirror_audio_source = Some("speaker".to_string());
        assert!(validate_scrcpy_config(&config).is_err());
    }

    #[test]
    fn test_build_scrcpy_args_mirror_defaults() {
        let config = base_config("mirror");
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"-s".to_string()));
        assert!(args.contains(&"device1".to_string()));
        assert!(args.contains(&"--video-codec=h264".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_turn_off_allows_wake_before_screen_off() {
        let mut config = base_config("mirror");
        config.turn_off = Some(true);

        let args = build_scrcpy_args(&config, None, None);

        assert!(args.contains(&"--turn-screen-off".to_string()));
        assert!(!args.contains(&"--no-power-on".to_string()));
    }

    #[test]
    fn test_wakes_before_screen_off_only_for_windowed_control_sessions() {
        let mut config = base_config("mirror");
        assert!(!wakes_before_screen_off(&config));

        config.turn_off = Some(true);
        assert!(wakes_before_screen_off(&config));

        // No serial means scrcpy picks the device itself; do not run a wake
        // command against an empty `-s`.
        config.device.clear();
        assert!(!wakes_before_screen_off(&config));

        config.device = "device1".to_string();
        for mode in ["camera", "mic"] {
            config.session_mode = mode.to_string();
            assert!(!wakes_before_screen_off(&config), "{mode}");
        }

        config.session_mode = "desktop".to_string();
        assert!(wakes_before_screen_off(&config));
    }

    #[test]
    fn test_display_power_is_on_parses_dumpsys_variants() {
        assert!(display_power_is_on("Display Power: state=ON\n"));
        assert!(!display_power_is_on("Display Power: state=OFF\n"));
        // A stale wakefulness line must not hide an explicit display-off state.
        assert!(!display_power_is_on(
            "mWakefulness=Awake\nDisplay Power: state=OFF\n"
        ));
        assert!(display_power_is_on("mScreenOn=true\nmWakefulness=Awake\n"));
        assert!(!display_power_is_on("mScreenOn=false\n"));
        assert!(display_power_is_on("mWakefulness=Awake\n"));
        assert!(!display_power_is_on("mWakefulness=Dozing\n"));
        assert!(!display_power_is_on("unrecognized dump"));
    }

    #[test]
    fn test_build_scrcpy_args_camera_mode() {
        let mut config = base_config("camera");
        config.fps = Some(30);
        config.camera_facing = Some("front".to_string());

        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--video-source=camera".to_string()));
        assert!(args.contains(&"--camera-facing=front".to_string()));
        assert!(args.contains(&"--camera-fps".to_string()));
        assert!(args.contains(&"30".to_string()));
    }

    #[test]
    fn test_camera_mode_forwards_the_phone_microphone() {
        let config = base_config("camera");
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=mic".to_string()));

        // Disabling audio must not leave an orphan --audio-source.
        let mut muted = base_config("camera");
        muted.audio_enabled = Some(false);
        let args = build_scrcpy_args(&muted, None, None);
        assert!(args.contains(&"--no-audio".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--audio-source")));

        // Mirror keeps the default device-output capture.
        let mirror = base_config("mirror");
        let args = build_scrcpy_args(&mirror, None, None);
        assert!(!args.iter().any(|a| a.starts_with("--audio-source")));
    }

    #[test]
    fn test_mirror_audio_source_and_dup_flags() {
        let mut config = base_config("mirror");
        config.mirror_audio_source = Some("playback".to_string());
        config.audio_dup = Some(true);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=playback".to_string()));
        assert!(args.contains(&"--audio-dup".to_string()));

        // --audio-dup is only valid with playback.
        config.mirror_audio_source = Some("mic".to_string());
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=mic".to_string()));
        assert!(!args.contains(&"--audio-dup".to_string()));

        // The default source is implicit, matching scrcpy's default.
        config.mirror_audio_source = Some("output".to_string());
        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a.starts_with("--audio-source")));
        assert!(!args.contains(&"--audio-dup".to_string()));

        config.mirror_audio_source = Some("voice-performance".to_string());
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=voice-performance".to_string()));

        // Disabling audio removes both flags.
        config.mirror_audio_source = Some("playback".to_string());
        config.audio_dup = Some(true);
        config.audio_enabled = Some(false);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--no-audio".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--audio-source")));
        assert!(!args.contains(&"--audio-dup".to_string()));
    }

    #[test]
    fn test_normalize_mirror_audio_falls_back_on_unsupported_values() {
        let mut config = base_config("mirror");
        config.mirror_audio_source = Some("voice-call".to_string());
        let caps = Capabilities::for_test(&["audio-source"], &["output", "playback", "mic"]);
        assert!(normalize_mirror_audio(&mut config, &caps).is_some());
        assert_eq!(config.mirror_audio_source.as_deref(), Some("output"));

        let mut config = base_config("mirror");
        config.mirror_audio_source = Some("playback".to_string());
        config.audio_dup = Some(true);
        let caps = Capabilities::for_test(&["audio-source"], &["output", "playback"]);
        assert!(normalize_mirror_audio(&mut config, &caps).is_some());
        assert_eq!(config.audio_dup, Some(false));

        let mut config = base_config("mirror");
        config.mirror_audio_source = Some("playback".to_string());
        config.audio_dup = Some(true);
        let caps = Capabilities::for_test(&["audio-source", "audio-dup"], &["output", "playback"]);
        assert!(normalize_mirror_audio(&mut config, &caps).is_none());
        assert_eq!(config.audio_dup, Some(true));
    }

    #[test]
    fn test_build_scrcpy_args_bitrate_and_fps() {
        let mut config = base_config("mirror");
        config.bitrate = Some(8);
        config.fps = Some(60);

        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--video-bit-rate".to_string()));
        assert!(args.contains(&"8M".to_string()));
        assert!(args.contains(&"--max-fps".to_string()));
        assert!(args.contains(&"60".to_string()));
    }

    #[test]
    fn test_audio_codec_auto_adds_no_flag() {
        let mut config = base_config("mirror");
        config.audio_codec = Some("auto".to_string());

        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a.starts_with("--audio-codec")));
    }

    #[test]
    fn test_audio_codec_manual_adds_flag() {
        let mut config = base_config("mirror");
        config.audio_codec = Some("aac".to_string());

        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-codec=aac".to_string()));
    }

    #[test]
    fn test_audio_codec_override_takes_precedence() {
        let mut config = base_config("mirror");
        config.audio_codec = Some("auto".to_string());

        let args = build_scrcpy_args(&config, None, Some("flac"));
        assert!(args.contains(&"--audio-codec=flac".to_string()));
    }

    #[test]
    fn test_audio_codec_skipped_when_audio_disabled() {
        let mut config = base_config("mirror");
        config.audio_enabled = Some(false);
        config.audio_codec = Some("aac".to_string());

        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--no-audio".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--audio-codec")));
    }

    #[test]
    fn test_require_audio_flag_is_opt_in_and_only_with_audio() {
        let config = base_config("mirror");
        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a == "--require-audio"));

        let mut config = base_config("mirror");
        config.require_audio = Some(true);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.iter().any(|a| a == "--require-audio"));

        // Contradictory combination must never reach scrcpy.
        let mut config = base_config("mirror");
        config.audio_enabled = Some(false);
        config.require_audio = Some(true);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--no-audio".to_string()));
        assert!(!args.iter().any(|a| a == "--require-audio"));

        // Mic mode has its own minimal arg set (audio is mandatory there).
        let mut config = base_config("mic");
        config.require_audio = Some(true);
        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a == "--require-audio"));
    }

    #[test]
    fn test_capability_guard_protects_modes_and_audio() {
        let caps = Capabilities::for_test(
            &[
                "video-codec",
                "audio-codec",
                "video-source",
                "camera-size",
                "audio-source",
                "no-window",
                "no-video",
                "new-display",
                "require-audio",
            ],
            &["mic", "mic-voice-communication"],
        );
        for mode in ["mirror", "camera", "mic", "desktop"] {
            assert!(
                ensure_mode_supported(&base_config(mode), &caps).is_ok(),
                "{mode}"
            );
        }

        let old = Capabilities::for_test(&["video-codec", "audio-codec", "audio-source"], &["mic"]);
        let camera = ensure_mode_supported(&base_config("camera"), &old).unwrap_err();
        assert!(camera.contains("Camera mode"));
        assert!(camera.contains("--video-source"));
        let mic = ensure_mode_supported(&base_config("mic"), &old).unwrap_err();
        assert!(mic.contains("--no-window"));
        let desktop = ensure_mode_supported(&base_config("desktop"), &old).unwrap_err();
        assert!(desktop.contains("--new-display"));
        assert!(ensure_mode_supported(&base_config("mirror"), &old).is_ok());

        let no_audio = Capabilities::for_test(&["video-codec"], &[]);
        let audio = ensure_mode_supported(&base_config("mirror"), &no_audio).unwrap_err();
        assert!(audio.contains("Audio forwarding"));

        let mut require = base_config("mirror");
        require.require_audio = Some(true);
        let audio_only = Capabilities::for_test(&["video-codec", "audio-codec"], &[]);
        let require_err = ensure_mode_supported(&require, &audio_only).unwrap_err();
        assert!(require_err.contains("Require audio"));

        let mut hid = base_config("mirror");
        hid.hid_keyboard = Some(true);
        let no_hid = Capabilities::for_test(&["video-codec", "audio-codec"], &[]);
        let hid_err = ensure_mode_supported(&hid, &no_hid).unwrap_err();
        assert!(hid_err.contains("--keyboard"));

        // The audio-only bridge ignores HID flags, so they must not block it.
        let mut mic_hid = base_config("mic");
        mic_hid.hid_keyboard = Some(true);
        assert!(ensure_mode_supported(&mic_hid, &caps).is_ok());
    }

    #[test]
    fn test_mic_source_falls_back_when_denoise_is_unsupported() {
        let mut config = base_config("mic");
        config.mic_source = Some("mic-voice-communication".to_string());
        let old = Capabilities::for_test(&["audio-source"], &["mic"]);
        assert!(normalize_mic_source(&mut config, &old).is_some());
        assert_eq!(config.mic_source.as_deref(), Some("mic"));

        let mut config = base_config("mic");
        config.mic_source = Some("mic-voice-communication".to_string());
        let new = Capabilities::for_test(&["audio-source"], &["mic", "mic-voice-communication"]);
        assert!(normalize_mic_source(&mut config, &new).is_none());
        assert_eq!(
            config.mic_source.as_deref(),
            Some("mic-voice-communication")
        );
    }

    #[test]
    fn test_build_scrcpy_args_mic_defaults() {
        let config = base_config("mic");
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--no-window".to_string()));
        assert!(args.contains(&"--no-video".to_string()));
        assert!(args.contains(&"--audio-source=mic".to_string()));
        assert!(args.contains(&"--audio-codec=opus".to_string()));
        assert!(args.contains(&"--audio-bit-rate=128k".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_mic_uses_voice_communication_source() {
        let mut config = base_config("mic");
        config.mic_source = Some("mic-voice-communication".to_string());
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=mic-voice-communication".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_mic_normalizes_legacy_source() {
        let mut config = base_config("mic");
        config.mic_source = Some("voice-communication".to_string());
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=mic-voice-communication".to_string()));
        assert!(!args.contains(&"--audio-source=voice-communication".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_mic_codec_override_wins() {
        let mut config = base_config("mic");
        config.mic_codec = Some("aac".to_string());
        let args = build_scrcpy_args(&config, None, Some("opus"));
        assert!(args.contains(&"--audio-codec=opus".to_string()));
        assert!(!args.contains(&"--audio-codec=aac".to_string()));
    }

    #[test]
    fn test_is_audio_codec_error_matches_known_patterns() {
        assert!(is_audio_codec_error(
            "ERROR: Could not create default audio encoder for opus"
        ));
        assert!(is_audio_codec_error("Failed to initialize audio/opus"));
        assert!(is_audio_codec_error("audio codec not supported on device"));
        assert!(is_audio_codec_error("audio encoder failed to start"));
    }

    #[test]
    fn test_is_audio_codec_error_ignores_unrelated() {
        assert!(!is_audio_codec_error("ADB disconnected"));
        assert!(!is_audio_codec_error("device offline"));
        assert!(!is_audio_codec_error("video encoder error"));
        assert!(!is_audio_codec_error("permission issue"));
        assert!(!is_audio_codec_error("INFO Audio codec selected: opus"));
    }

    #[test]
    fn test_build_scrcpy_args_window_position() {
        let mut config = base_config("mirror");
        config.window_x = Some(100);
        config.window_y = Some(200);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--window-x=100".to_string()));
        assert!(args.contains(&"--window-y=200".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_window_position_negative_coords() {
        // Multi-monitor: a display left of / above the primary yields negative
        // coordinates, which scrcpy accepts.
        let mut config = base_config("mirror");
        config.window_x = Some(-1920);
        config.window_y = Some(0);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--window-x=-1920".to_string()));
        assert!(args.contains(&"--window-y=0".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_window_position_skipped_in_fullscreen() {
        let mut config = base_config("mirror");
        config.window_x = Some(100);
        config.window_y = Some(200);
        config.fullscreen = Some(true);
        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a.starts_with("--window-x")));
        assert!(!args.iter().any(|a| a.starts_with("--window-y")));
    }

    #[test]
    fn test_build_scrcpy_args_window_position_only_mirror() {
        // Camera (and desktop) sessions must not receive the mirror window's
        // remembered position, it is scoped to mirror mode.
        let mut config = base_config("camera");
        config.window_x = Some(100);
        config.window_y = Some(200);
        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a.starts_with("--window-x")));
        assert!(!args.iter().any(|a| a.starts_with("--window-y")));
    }

    #[test]
    fn test_parse_shell_var_int() {
        let text = "WINDOW=12345\nX=959\nY=24\nWIDTH=428\nHEIGHT=928\n";
        assert_eq!(parse_shell_var_int(text, "X"), Some(959));
        assert_eq!(parse_shell_var_int(text, "Y"), Some(24));
        assert_eq!(parse_shell_var_int(text, "MISSING"), None);
    }

    #[test]
    fn test_resolve_window_pos_first_launch_uses_raw() {
        // First launch ever (nothing requested): persist the raw last sample.
        assert_eq!(
            resolve_window_pos_to_persist(None, Some((100, 72)), (300, 222)),
            (300, 222)
        );
        assert_eq!(
            resolve_window_pos_to_persist(None, None, (300, 222)),
            (300, 222)
        );
    }

    #[test]
    fn test_resolve_window_pos_zero_offset_is_noop() {
        // Windows/Linux: the tool already reports the client origin, so the
        // first sample matches what we requested -> offset 0 -> last unchanged.
        let requested = Some((100, 100));
        let first = Some((100, 100));
        assert_eq!(
            resolve_window_pos_to_persist(requested, first, (640, 480)),
            (640, 480)
        );
    }

    #[test]
    fn test_resolve_window_pos_cancels_titlebar_offset() {
        // macOS-like: the tool reports the frame origin (title-bar top), 28 px
        // above the client origin scrcpy positions. Requested (100,100) but
        // first observed (100,72); the -28 px offset must be cancelled so the
        // persisted value maps back to the client origin.
        let requested = Some((100, 100));
        let first = Some((100, 72));
        assert_eq!(
            resolve_window_pos_to_persist(requested, first, (300, 222)),
            (300, 250)
        );
    }

    #[test]
    fn test_resolve_window_pos_cancels_full_frame_offset() {
        // Frame offset on both axes (border + title bar).
        let requested = Some((100, 100));
        let first = Some((92, 69));
        assert_eq!(
            resolve_window_pos_to_persist(requested, first, (292, 219)),
            (300, 250)
        );
    }

    #[test]
    fn test_resolve_window_pos_ignores_implausible_offset() {
        // The window was moved before the first sample (or placed by the WM):
        // the offset dwarfs any window decoration, so fall back to the raw last.
        let requested = Some((100, 100));
        let first = Some((900, 700));
        assert_eq!(
            resolve_window_pos_to_persist(requested, first, (950, 760)),
            (950, 760)
        );
    }

    #[test]
    fn test_window_position_tracking_is_windowed_mirror_only() {
        assert!(tracks_window_position(&base_config("mirror")));

        let mut fullscreen = base_config("mirror");
        fullscreen.fullscreen = Some(true);
        assert!(!tracks_window_position(&fullscreen));

        for mode in ["camera", "desktop", "mic"] {
            assert!(!tracks_window_position(&base_config(mode)));
        }
    }

    #[test]
    fn test_cap_terminal_output_keeps_small_output() {
        assert_eq!(cap_terminal_output("ok".to_string()), "ok");
    }

    #[test]
    fn test_cap_terminal_output_truncates_on_char_boundary() {
        // Two-byte chars: a naive byte cut would slice one in half and panic
        // on the next String operation.
        let text = "é".repeat(MAX_TERMINAL_OUTPUT_BYTES / 2 + 1);
        let capped = cap_terminal_output(text);
        assert!(capped.ends_with("[output truncated]"));
        assert!(capped.len() <= MAX_TERMINAL_OUTPUT_BYTES + "\n[output truncated]".len());
    }

    #[test]
    fn test_parse_adb_device_line_usb_serial() {
        assert_eq!(
            parse_adb_device_line("ZY22MGW35T device"),
            Some(("ZY22MGW35T".to_string(), "device".to_string()))
        );
    }

    #[test]
    fn test_parse_adb_device_line_tcpip_serial() {
        assert_eq!(
            parse_adb_device_line("192.168.1.50:5555 device"),
            Some(("192.168.1.50:5555".to_string(), "device".to_string()))
        );
    }

    #[test]
    fn test_parse_adb_device_line_mdns_serial() {
        assert_eq!(
            parse_adb_device_line("adb-ZY22MGW35T-Z3uXXq._adb-tls-connect._tcp device"),
            Some((
                "adb-ZY22MGW35T-Z3uXXq._adb-tls-connect._tcp".to_string(),
                "device".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_adb_device_line_mdns_conflict_serial_with_space() {
        // mDNS conflict resolution appends " (2)" inside the serial itself, so
        // the line has a space that is NOT the serial/state separator.
        assert_eq!(
            parse_adb_device_line("adb-ZY22MGW35T-Z3uXXq (2)._adb-tls-connect._tcp device"),
            Some((
                "adb-ZY22MGW35T-Z3uXXq (2)._adb-tls-connect._tcp".to_string(),
                "device".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_adb_device_line_ignores_trailing_l_fields() {
        // `adb devices -l` appends product/model/transport_id fields after the
        // state; they must not affect the parsed serial or state.
        assert_eq!(
            parse_adb_device_line(
                "ZY22MGW35T device usb:1-1 product:foo model:bar device:baz transport_id:1"
            ),
            Some(("ZY22MGW35T".to_string(), "device".to_string()))
        );
    }

    #[test]
    fn test_extract_device_model_replaces_underscores_with_spaces() {
        assert_eq!(
            extract_device_model(
                "ZY22MGW35T device usb:1-1 product:foo model:SM_S911B device:baz transport_id:1"
            ),
            Some("SM S911B".to_string())
        );
    }

    #[test]
    fn test_extract_device_model_missing_field_returns_none() {
        assert_eq!(extract_device_model("ZY22MGW35T device"), None);
    }

    #[test]
    fn test_parse_adb_device_line_offline_and_unauthorized() {
        assert_eq!(
            parse_adb_device_line("ZY22MGW35T offline"),
            Some(("ZY22MGW35T".to_string(), "offline".to_string()))
        );
        assert_eq!(
            parse_adb_device_line("192.168.1.50:5555 unauthorized"),
            Some(("192.168.1.50:5555".to_string(), "unauthorized".to_string()))
        );
    }

    #[test]
    fn test_parse_adb_device_line_unparsable_returns_none() {
        assert_eq!(parse_adb_device_line(""), None);
        assert_eq!(parse_adb_device_line("List of devices attached"), None);
    }

    #[test]
    fn test_get_devices_filtering_keeps_only_device_state() {
        // Mirrors the filter chain in get_devices: only the exact "device"
        // state is kept, regardless of serial shape, including mDNS wireless
        // debugging serials (._tcp/._udp) that must no longer be excluded.
        let output = "List of devices attached\n\
             ZY22MGW35T device usb:1-1 product:foo model:bar transport_id:1\n\
             192.168.1.50:5555 device product:foo model:bar transport_id:2\n\
             adb-ZY22MGW35T-Z3uXXq._adb-tls-connect._tcp device product:foo transport_id:3\n\
             adb-ZY22MGW35T-Z3uXXq (2)._adb-tls-connect._tcp device product:foo transport_id:4\n\
             emulator-5554 offline\n\
             0123456789ABCDEF unauthorized\n";

        let devices: Vec<String> = output
            .lines()
            .skip(1)
            .filter_map(parse_adb_device_line)
            .filter(|(_, state)| state == "device")
            .map(|(serial, _)| serial)
            .collect();

        assert_eq!(
            devices,
            vec![
                "ZY22MGW35T".to_string(),
                "192.168.1.50:5555".to_string(),
                "adb-ZY22MGW35T-Z3uXXq._adb-tls-connect._tcp".to_string(),
                "adb-ZY22MGW35T-Z3uXXq (2)._adb-tls-connect._tcp".to_string(),
            ]
        );
    }

    #[test]
    fn test_split_args_supports_single_quotes() {
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
    fn test_split_args_supports_escaped_spaces() {
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
    fn test_split_args_keeps_windows_paths_inside_quotes() {
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
    fn test_split_args_keeps_unc_paths() {
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
    fn test_split_args_rejects_unclosed_single_quote() {
        assert!(split_args("push 'Minha Foto.jpg").is_err());
    }

    #[test]
    fn test_is_expected_asset_name_accepts_real_releases() {
        assert!(is_expected_asset_name("scrcpy-win64-v3.3.3.zip", ".zip"));
        assert!(is_expected_asset_name(
            "scrcpy-linux-x86_64-v2.7.tar.gz",
            ".tar.gz"
        ));
    }

    #[test]
    fn test_is_expected_asset_name_rejects_unexpected() {
        assert!(!is_expected_asset_name("scrcpy-win64-v3.3.3.exe", ".zip"));
        assert!(!is_expected_asset_name("../evil.zip", ".zip"));
        assert!(!is_expected_asset_name("/abs/scrcpy-win64-v1.zip", ".zip"));
        assert!(!is_expected_asset_name("scrcpy-win64-v1.zip ", ".zip"));
        assert!(!is_expected_asset_name("scrcpy-v1/../../evil.zip", ".zip"));
        let oversized = format!("scrcpy-{}.zip", "a".repeat(200));
        assert!(!is_expected_asset_name(&oversized, ".zip"));
    }

    #[test]
    fn render_drivers_handle_multibyte_help_and_whole_tokens() {
        // A multibyte character straddling the 1600-byte window used to panic.
        let help = format!("--render-driver \"opengl\" {}é", "a".repeat(1574));
        let drivers = detect_render_drivers_from_help(&help);
        assert!(drivers.contains(&"opengl".to_string()));

        // "opengles2" must not be reported as plain "opengl".
        let only_gles = detect_render_drivers_from_help("--render-driver=\"opengles2\"");
        assert_eq!(only_gles, vec!["opengles2".to_string()]);
    }

    #[test]
    fn test_find_wlan_ipv4_keeps_scanning_past_trailing_src() {
        assert_eq!(
            find_wlan_ipv4("inet 10.0.0.5/24 brd 10.0.0.255 scope global wlan0 src"),
            Some("10.0.0.5".to_string())
        );
    }

    #[test]
    fn test_find_wlan_ipv4_ignores_ipv6_and_junk() {
        assert_eq!(find_wlan_ipv4("inet fe80::1/64 src ::1"), None);
        assert_eq!(find_wlan_ipv4(""), None);
        assert_eq!(find_wlan_ipv4("src"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_get_local_scrcpy_version_times_out_on_hung_binary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("openscrcpy-version-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let fake = dir.join("scrcpy");
        std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").expect("fake scrcpy");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let started = std::time::Instant::now();
        let version = get_local_scrcpy_version(Some(dir.to_string_lossy().to_string())).await;
        let _ = std::fs::remove_dir_all(&dir);

        assert!(version.is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn test_safe_file_name_keeps_final_component() {
        assert_eq!(
            safe_file_name("report.json"),
            Some("report.json".to_string())
        );
        assert_eq!(
            safe_file_name("../../etc/passwd"),
            Some("passwd".to_string())
        );
        assert_eq!(
            safe_file_name("/tmp/report.json"),
            Some("report.json".to_string())
        );
        assert_eq!(safe_file_name(""), None);
    }

    #[test]
    fn test_sanitize_filename_component_allowlists() {
        assert_eq!(
            sanitize_filename_component("192.168.1.50:5555"),
            "192.168.1.50_5555"
        );
        assert_eq!(
            sanitize_filename_component("emulator-5554"),
            "emulator-5554"
        );
        assert_eq!(sanitize_filename_component("../../evil"), "_.._evil");
        assert_eq!(sanitize_filename_component("..."), "device");
        assert_eq!(sanitize_filename_component(""), "device");
    }
}

/// Applies the persisted background preference and (re)builds the tray menu
/// with the labels for the active locale. Keeping the flag in backend state
/// lets the window close handler decide without a round-trip to the webview.
#[tauri::command]
pub fn configure_background_mode(
    app: tauri::AppHandle,
    state: State<'_, ScrcpyState>,
    enabled: bool,
    show_label: String,
    quit_label: String,
) -> Result<(), String> {
    // Turning background mode off must never depend on labels: otherwise a
    // bad translation could leave the backend hiding a window without a tray.
    let labels = if enabled {
        crate::tray::TrayLabels {
            show: validate::tray_label(&show_label)?,
            quit: validate::tray_label(&quit_label)?,
        }
    } else {
        crate::tray::TrayLabels {
            show: String::new(),
            quit: String::new(),
        }
    };
    match crate::tray::configure(&app, enabled, &labels) {
        Ok(()) => {
            state.background_mode.store(enabled, Ordering::Relaxed);
            Ok(())
        }
        Err(error) if !enabled => {
            // Never keep hiding the window if the preference was turned off,
            // even if hiding the tray icon itself failed.
            state.background_mode.store(false, Ordering::Relaxed);
            Err(error)
        }
        Err(error) => Err(error),
    }
}

#[tauri::command]
pub async fn stop_scrcpy(state: State<'_, ScrcpyState>, device: String) -> Result<(), String> {
    let handle = {
        let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.remove(&device)
    };
    let Some(handle) = handle else {
        return Ok(());
    };

    let SessionHandle {
        pid,
        stop,
        done,
        track_position,
        ..
    } = handle;
    let pid_value = pid.load(Ordering::Relaxed);

    // Grab the window's position now, one last time, before it is killed:
    // this is the only moment it is guaranteed to still be exactly where the
    // user left it. The monitor task refreshes its own sample every ~2s,
    // but a move right before Stop could still beat it. Only windowed mirror
    // sessions persist a position; camera/desktop/fullscreen must not
    // overwrite the mirror slot.
    let position = if track_position && pid_value != 0 {
        try_capture_window_pos(pid_value)
    } else {
        None
    };

    let notified = done.notified();
    let _ = stop.send(StopRequest { position });

    match tokio::time::timeout(Duration::from_secs(3), notified).await {
        Ok(_) => Ok(()),
        Err(_) => {
            force_kill_pid(pid_value);
            Err("session did not stop within 3s".to_string())
        }
    }
}

// App exit teardown: kill every registered child (mirror, camera, mic
// bridge) and unload the mic sink, so closing the app cannot leave audio
// or a mirror running in the background. This handler is synchronous, so it
// uses SIGKILL/TerminateProcess instead of waiting for a graceful stop.
pub fn teardown_sessions(app_handle: &tauri::AppHandle) {
    let state = app_handle.state::<ScrcpyState>();
    let handles: Vec<SessionHandle> = {
        let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.drain().map(|(_, handle)| handle).collect()
    };
    for handle in handles {
        let pid = handle.pid.load(Ordering::Relaxed);
        #[cfg(target_os = "linux")]
        if handle.track_position {
            crate::kwin::stop_tracking(pid);
        }
        let _ = handle.stop.send(StopRequest { position: None });
        force_kill_pid(pid);
    }
    #[cfg(unix)]
    teardown_mic_sink();
}

// Synchronous sink teardown for process exit: event handlers run outside
// the async runtime, so this cannot reuse the window-based helper.
#[cfg(unix)]
fn teardown_mic_sink() {
    let Ok(modules) = StdCommand::new("pactl")
        .args(["list", "short", "modules"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    else {
        return;
    };
    let module_idx = modules
        .lines()
        .find(|l| l.contains(&format!("sink_name={}", MIC_SINK_NAME)))
        .and_then(|l| l.split('\t').next().map(str::to_string));
    if let Some(idx) = module_idx {
        let _ = StdCommand::new("pactl")
            .args(["unload-module", &idx])
            .output();
    }
}

#[tauri::command]
pub async fn download_scrcpy(window: Window, app_handle: tauri::AppHandle) -> Result<(), String> {
    use std::io::Write;

    let (arch_tag, extension) =
        download::resolve_asset(std::env::consts::OS, std::env::consts::ARCH)?;

    let _ = window.emit(
        "scrcpy-log",
        format!(
            "[SYSTEM] Detecting platform: {} ({})",
            std::env::consts::OS,
            arch_tag
        ),
    );
    let _ = window.emit("scrcpy-status", json!({ "type": "downloading", "success": true, "message": format!("Fetching latest {} release...", arch_tag) }));

    let client = http_client("OpenScrcpy-Downloader")?;

    // Attempt to get the latest release via API, but fallback to redirect scraping if rate limited
    let mut download_url = String::new();
    let mut filename = String::new();
    let mut release_tag: Option<String> = None;

    let api_url = "https://api.github.com/repos/Genymobile/scrcpy/releases/latest";
    let api_resp = client
        .get(api_url)
        .timeout(Duration::from_secs(15))
        .send()
        .await;

    let mut used_fallback = false;

    if let Ok(resp) = api_resp {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                release_tag = json["tag_name"].as_str().map(str::to_string);
                if let Some(assets) = json["assets"].as_array() {
                    for asset in assets {
                        let name = asset["name"].as_str().unwrap_or("");
                        if name.contains(arch_tag) && name.ends_with(extension) {
                            download_url = asset["browser_download_url"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            filename = name.to_string();
                            break;
                        }
                    }
                }
            }
        } else if resp.status() == reqwest::StatusCode::FORBIDDEN {
            let _ = window.emit(
                "scrcpy-log",
                "[SYSTEM] API rate limited, attempting fallback discovery...",
            );
            used_fallback = true;
        }
    } else {
        used_fallback = true;
    }

    if used_fallback || download_url.is_empty() {
        // Fallback: Follow redirect of /releases/latest to get the tag name
        let redirect_res = client
            .get("https://github.com/Genymobile/scrcpy/releases/latest")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("Fallback failed: {}", e))?;

        let final_url = redirect_res.url().as_str();
        // URL is like https://github.com/Genymobile/scrcpy/releases/tag/v2.7
        if let Some(tag) = final_url.split('/').next_back() {
            if tag.starts_with('v') {
                filename = format!("scrcpy-{}-{}{}", arch_tag, tag, extension);
                download_url = format!(
                    "https://github.com/Genymobile/scrcpy/releases/download/{}/{}",
                    tag, filename
                );
                release_tag = Some(tag.to_string());
                let _ = window.emit(
                    "scrcpy-log",
                    format!("[SYSTEM] Discovered latest tag via fallback: {}", tag),
                );
            }
        }
    }

    if download_url.is_empty() {
        return Err(format!(
            "Could not find {} binary. (API rate limit might be active)",
            arch_tag
        ));
    }

    // Only accept asset names matching the expected scrcpy release pattern.
    // The fallback path builds this name itself, but the API path echoes a
    // server-provided string, so validate it before downloading.
    if !is_expected_asset_name(&filename, extension) {
        return Err("Unexpected release asset name, aborting download".to_string());
    }

    let _ = window.emit("scrcpy-log", format!("[SYSTEM] Found asset: {}", filename));

    let release_tag = release_tag
        .ok_or_else(|| format!("Could not determine the release tag for {}", filename))?;

    // Checksums are published next to the archive; verify before extraction.
    let sums_url = format!(
        "https://github.com/Genymobile/scrcpy/releases/download/{}/SHA256SUMS.txt",
        release_tag
    );
    let sums_resp = client
        .get(&sums_url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch SHA256SUMS.txt: {}", e))?;
    if !sums_resp.status().is_success() {
        return Err(format!(
            "Release {} does not publish SHA256SUMS.txt; aborting download",
            release_tag
        ));
    }
    let sums_text = sums_resp
        .text()
        .await
        .map_err(|e| format!("Failed to read SHA256SUMS.txt: {}", e))?;
    let expected_hash = download::parse_sha256sums(&sums_text, &filename)?;

    let install_path = match download::install_dir() {
        Some(dir) => dir.to_path_buf(),
        None => download::init_install_dir(&app_handle),
    };
    let install_parent = install_path
        .parent()
        .ok_or_else(|| "Invalid install directory".to_string())?
        .to_path_buf();
    std::fs::create_dir_all(&install_parent)
        .map_err(|e| format!("Failed to create install directory: {}", e))?;

    let pid = std::process::id();
    let temp_archive_path = install_parent.join(format!("scrcpy-bin.tmp-{}{}", pid, extension));
    let temp_extract_dir = install_parent.join(format!("scrcpy-bin.extract-{}", pid));
    let new_dir = install_path.with_file_name("scrcpy-bin.new");
    let old_dir = install_path.with_file_name("scrcpy-bin.old");
    let mut cleanup = download::TempCleanup::new(vec![
        temp_archive_path.clone(),
        temp_extract_dir.clone(),
        new_dir.clone(),
    ]);

    {
        let mut file = std::fs::File::create(&temp_archive_path)
            .map_err(|e| format!("Failed to create archive file: {}", e))?;
        let mut download_resp = timeout(Duration::from_secs(30), client.get(&download_url).send())
            .await
            .map_err(|_| "Timed out connecting to the download URL".to_string())?
            .map_err(|e| format!("Failed to connect to download URL: {}", e))?;
        if !download_resp.status().is_success() {
            return Err(format!(
                "Download failed with HTTP {}",
                download_resp.status()
            ));
        }
        let total_size = download_resp.content_length().unwrap_or(0);

        let _ = window.emit(
            "scrcpy-log",
            format!("[SYSTEM] Downloading: {} MB", total_size / 1024 / 1024),
        );

        let mut downloaded: u64 = 0;
        let mut last_percent: Option<u64> = None;
        loop {
            // A stalled transfer must not keep the download flag on forever.
            // The timeout is per chunk, not for the whole file, so a slow but
            // healthy link still gets to finish.
            let chunk = timeout(Duration::from_secs(30), download_resp.chunk())
                .await
                .map_err(|_| "Download stalled for 30s".to_string())?
                .map_err(|e| e.to_string())?;
            let Some(chunk) = chunk else { break };
            file.write_all(&chunk)
                .map_err(|e| format!("Failed to write chunk: {}", e))?;
            downloaded += chunk.len() as u64;
            if let Some(percent) = (downloaded * 100).checked_div(total_size) {
                // Chunks are a few KB: emitting on every one floods the
                // webview with hundreds of identical percentages.
                if last_percent != Some(percent) {
                    last_percent = Some(percent);
                    let _ = window.emit(
                        "scrcpy-status",
                        json!({ "type": "download-progress", "percent": percent }),
                    );
                }
            }
        }
    }

    let actual_hash = download::sha256_file(&temp_archive_path)?;
    if actual_hash != expected_hash {
        return Err(format!(
            "Checksum mismatch for {}: expected {}, got {}",
            filename, expected_hash, actual_hash
        ));
    }

    let _ = window.emit(
        "scrcpy-log",
        "[SYSTEM] Download finished. Starting extraction...",
    );
    let _ = window.emit(
        "scrcpy-status",
        json!({ "type": "downloading", "success": true, "message": "Extracting binaries..." }),
    );

    let _ = window.emit(
        "scrcpy-log",
        format!(
            "[SYSTEM] Decompressing {} archive...",
            if extension == ".zip" { "ZIP" } else { "TAR.GZ" }
        ),
    );
    download::extract_archive(&temp_archive_path, &temp_extract_dir, extension)?;

    let source_root = download::single_root(&temp_extract_dir)?;
    let needs_adb = source_root.join(download::exe_name("adb")).is_file();

    if std::fs::rename(&source_root, &new_dir).is_err() {
        download::copy_dir_all(&source_root, &new_dir)
            .map_err(|e| format!("Failed to stage extracted files: {}", e))?;
    }
    download::validate_extracted(&new_dir, needs_adb)?;

    // A live session keeps the old binaries mapped, and on Windows can lock
    // the install directory so the next update fails to clear its backup.
    // Refuse instead of swapping underneath a running mirror. Holding
    // `binary_swap` until the swap finishes also stops a new session from
    // slipping between the check and the rename.
    let state = app_handle.state::<ScrcpyState>();
    let _swap_guard = state.binary_swap.lock().await;
    let sessions_active = !state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty();
    let start_pending = !state
        .starting_devices
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty();
    if sessions_active || start_pending {
        return Err("Stop all mirror sessions before updating the scrcpy core".to_string());
    }

    // The adb server outlives the download and may still be running the old
    // binary. Kill it so the next adb call starts from the new tree.
    let mut adb_stop = create_command(get_binary_path("adb", None));
    adb_stop.arg("kill-server");
    if let Err(e) = process::output_with_timeout(adb_stop, Duration::from_secs(5)).await {
        let _ = window.emit(
            "scrcpy-log",
            format!("[SYSTEM] Could not stop adb before installing: {}", e),
        );
    }

    // Swap the staged tree into place, restoring the previous install if the
    // final rename fails.
    if old_dir.exists() {
        std::fs::remove_dir_all(&old_dir)
            .map_err(|e| format!("Failed to clear previous backup: {}", e))?;
    }
    let backup = if install_path.exists() {
        std::fs::rename(&install_path, &old_dir)
            .map_err(|e| format!("Failed to move previous install aside: {}", e))?;
        true
    } else {
        false
    };
    if let Err(e) = std::fs::rename(&new_dir, &install_path) {
        if backup {
            let _ = std::fs::rename(&old_dir, &install_path);
        }
        return Err(format!("Failed to install scrcpy binaries: {}", e));
    }
    if backup {
        let _ = std::fs::remove_dir_all(&old_dir);
    }

    let _ = std::fs::remove_file(&temp_archive_path);
    let _ = std::fs::remove_dir_all(&temp_extract_dir);
    cleanup.disarm();

    let _ = window.emit("scrcpy-status", json!({ "type": "download-complete", "success": true, "message": install_path.to_string_lossy() }));
    Ok(())
}

#[tauri::command]
pub async fn get_videos_dir(app_handle: tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager;
    app_handle
        .path()
        .video_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_report(
    app_handle: tauri::AppHandle,
    content: String,
    name: String,
) -> Result<String, String> {
    use std::fs;
    use tauri::Manager;

    let downloads = app_handle
        .path()
        .download_dir()
        .map_err(|e| e.to_string())?;

    // Keep only the final path component so a crafted `name` cannot escape
    // the downloads directory (absolute paths or `..` segments).
    let safe_name = safe_file_name(&name).ok_or_else(|| "Invalid report name".to_string())?;
    let path = downloads.join(&safe_name);
    // create_new refuses to follow an existing symlink or overwrite a file.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    use std::io::Write;
    file.write_all(content.as_bytes())
        .map_err(|e| e.to_string())?;

    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn get_scrcpy_bin_dir() -> Result<String, String> {
    if let Some(dir) = download::install_dir() {
        if dir.exists() {
            return Ok(dir.to_string_lossy().to_string());
        }
    }

    let current_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "Could not determine application directory".to_string())?;

    let extract_path = current_dir.join("scrcpy-bin");

    if extract_path.exists() {
        Ok(extract_path.to_string_lossy().to_string())
    } else {
        Ok(current_dir.to_string_lossy().to_string())
    }
}

pub async fn get_local_scrcpy_version(custom_path: Option<String>) -> Option<String> {
    let exe_path = get_binary_path("scrcpy", custom_path);
    let mut cmd = create_command(&exe_path);
    cmd.arg("--version");
    let output = process::output_with_timeout(cmd, Duration::from_secs(3))
        .await
        .ok()?;
    if output.status.success() {
        let out_str = String::from_utf8_lossy(&output.stdout);
        for line in out_str.lines() {
            if line.contains("scrcpy") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(idx) = parts.iter().position(|&x| x == "scrcpy") {
                    if let Some(version) = parts.get(idx + 1) {
                        return Some(version.trim_start_matches('v').to_string());
                    }
                }
            }
        }
    }
    None
}

pub async fn get_latest_scrcpy_version() -> Result<String, String> {
    let client = http_client("OpenScrcpy-Updater")?;

    let api_url = "https://api.github.com/repos/Genymobile/scrcpy/releases/latest";
    let api_resp = client
        .get(api_url)
        .timeout(Duration::from_secs(15))
        .send()
        .await;

    let mut tag_name = String::new();
    let mut used_fallback = false;

    if let Ok(resp) = api_resp {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(tag) = json["tag_name"].as_str() {
                    tag_name = tag.trim_start_matches('v').to_string();
                }
            }
        } else if resp.status() == reqwest::StatusCode::FORBIDDEN {
            used_fallback = true;
        }
    } else {
        used_fallback = true;
    }

    if used_fallback || tag_name.is_empty() {
        let redirect_res = client
            .get("https://github.com/Genymobile/scrcpy/releases/latest")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("Fallback failed: {}", e))?;

        let final_url = redirect_res.url().as_str();
        if let Some(tag) = final_url.split('/').next_back() {
            if tag.starts_with('v') || tag.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                tag_name = tag.trim_start_matches('v').to_string();
            }
        }
    }

    if tag_name.is_empty() {
        return Err("Could not determine latest version".to_string());
    }

    Ok(tag_name)
}

fn compare_versions(local: &str, remote: &str) -> bool {
    let local_parts: Vec<u32> = local
        .split('.')
        .filter_map(|s| {
            s.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .collect();
    let remote_parts: Vec<u32> = remote
        .split('.')
        .filter_map(|s| {
            s.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .collect();

    for i in 0..std::cmp::max(local_parts.len(), remote_parts.len()) {
        let local_val = local_parts.get(i).cloned().unwrap_or(0);
        let remote_val = remote_parts.get(i).cloned().unwrap_or(0);
        if remote_val > local_val {
            return true;
        } else if local_val > remote_val {
            return false;
        }
    }
    false
}

#[tauri::command]
pub async fn check_scrcpy_update(custom_path: Option<String>) -> serde_json::Value {
    let local_version = match get_local_scrcpy_version(custom_path.clone()).await {
        Some(v) => v,
        None => {
            return json!({ "update_available": false, "local_version": null, "latest_version": null, "message": "Scrcpy not installed or not working" })
        }
    };

    let latest_version = match get_latest_scrcpy_version().await {
        Ok(v) => v,
        Err(e) => {
            return json!({ "update_available": false, "local_version": local_version, "latest_version": null, "message": format!("Could not fetch latest release: {}", e) })
        }
    };

    let update_available = compare_versions(&local_version, &latest_version);

    json!({
        "update_available": update_available,
        "local_version": local_version,
        "latest_version": latest_version
    })
}

// "View release" from the update modal: open the releases page in the
// user's browser. Rust-side open keeps it outside the webview popup rules;
// the HTTPS check keeps the shell handler from being abused with anything
// else even if a future caller passes this command a value.
#[tauri::command]
pub async fn open_app_release(app: tauri::AppHandle) -> Result<(), String> {
    tauri_plugin_opener::OpenerExt::opener(&app)
        .open_url(
            "https://github.com/yanhenrique-dev/OpenScrcpy/releases/latest",
            None::<&str>,
        )
        .map_err(|e| e.to_string())
}

// One-click USB -> wireless switch: read the phone's Wi-Fi IP from its own
// routing table, lift adbd into TCPIP mode, then point the daemon back at
// the phone over the network. Every step is logged to the console like
// the rest of the adb surface, so a failure is diagnosable in-app.
#[tauri::command]
pub async fn goto_wireless(
    window: Window,
    device: String,
    custom_path: Option<String>,
) -> Result<serde_json::Value, String> {
    validate::device(&device)?;
    let adb_path = get_binary_path("adb", custom_path);
    let _ = window.emit(
        "scrcpy-log",
        "[SYSTEM] Switching to wireless...".to_string(),
    );

    // 1) The phone's Wi-Fi IPv4. `addr show wlan0` reads the Wi-Fi interface
    //    directly, so a VPN tunnel on the phone cannot steal the answer the
    //    way it does with the default-route source address; `ip route get`
    //    stays as fallback (a few OEMs rename the interface).
    let mut ip = String::new();
    for probe in ["ip -f inet addr show wlan0", "ip route get 1.1.1.1"] {
        let mut cmd = create_command(&adb_path);
        cmd.args(["-s", &device, "shell", probe]);
        let output = process::output_with_timeout(cmd, Duration::from_secs(5)).await?;
        if let Some(found) = find_wlan_ipv4(&String::from_utf8_lossy(&output.stdout)) {
            ip = found;
            break;
        }
    }
    if ip.is_empty() {
        let _ = window.emit(
            "scrcpy-log",
            "[SYSTEM] Could not read the Wi-Fi IP — is the phone actually on a Wi-Fi network?"
                .to_string(),
        );
        return Ok(json!({ "success": false, "message": "could not read the device Wi-Fi IPv4" }));
    }

    let _ = window.emit(
        "scrcpy-log",
        format!("[SYSTEM] adbd restarting in TCP mode (Wi-Fi IP {}).", ip),
    );
    let mut cmd = create_command(&adb_path);
    cmd.args(["-s", &device, "tcpip", "5555"]);
    let output = process::output_with_timeout(cmd, Duration::from_secs(5)).await?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let _ = window.emit("scrcpy-log", format!("[ADB ERROR] {}", err));
        return Ok(json!({
            "success": false,
            "message": if err.is_empty() { "adb tcpip failed".to_string() } else { err }
        }));
    }

    // 2) adbd needs a beat to come back up before the connect can land.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let target = format!("{}:5555", ip);
    let mut cmd = create_command(&adb_path);
    cmd.args(["connect", &target]);
    let output = process::output_with_timeout(cmd, Duration::from_secs(7)).await?;
    let out_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !out_text.is_empty() {
        let _ = window.emit("scrcpy-log", format!("[ADB] {}", out_text));
    }
    let success = output.status.success()
        && !out_text.contains("cannot connect")
        && !out_text.contains("failed");
    if success {
        let _ = window.emit(
            "scrcpy-log",
            format!(
                "[OK] Wireless ready at {} — you can unplug the cable.",
                target
            ),
        );
    }
    Ok(json!({ "success": success, "ip": ip, "port": 5555, "message": out_text }))
}

// Pull the first dotted-quad after "src"/"inet" out of `ip` output.
// Skips IPv6 candidates ("inet6 fe80::.../64" yields nothing valid), so the
// caller always gets a wlan v4 address or None.
fn find_wlan_ipv4(text: &str) -> Option<String> {
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
