//! ADB discovery, device-list parsing and one-shot commands.
//!
//! Nothing here needs a window: the Tauri shell keeps the thin command
//! wrappers and the event names, while this module owns the process work and
//! the parsing rules.

use std::time::Duration;

use serde_json::{json, Value};

use crate::{paths, process, terminal, validate};

/// ADB device states that can appear in the second column of `adb devices -l`
/// output. Only "device" means the device is connected and authorized; every
/// other state (offline, mid-pairing, unauthorized, ...) is not usable yet.
pub const KNOWN_ADB_STATES: &[&str] = &[
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
pub fn parse_adb_device_line(line: &str) -> Option<(String, String)> {
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
pub fn extract_device_model(line: &str) -> Option<String> {
    let model = line
        .split_whitespace()
        .find_map(|t| t.strip_prefix("model:"))?;
    if model.is_empty() {
        return None;
    }
    Some(model.replace('_', " "))
}

/// Runs `scrcpy --version` through the resolved binary and reports whether it
/// is usable. This is the early liveness check the header shows.
pub async fn check_scrcpy(custom_path: Option<&str>) -> Value {
    let exe_path = paths::binary_path("scrcpy", custom_path);
    let mut cmd = process::create_command(&exe_path);
    cmd.arg("--version");
    let output = process::output_with_timeout(cmd, Duration::from_secs(5)).await;

    match output {
        Ok(output) if output.status.success() => {
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

/// Reads the Android release version (e.g. `15`) from the device. Used by the
/// shell to badge the device row; a failure is silent and simply omits it.
pub async fn android_version(serial: &str) -> Option<String> {
    if validate::device(serial).is_err() {
        return None;
    }
    let adb_path = paths::binary_path("adb", None);
    let mut cmd = process::create_command(&adb_path);
    cmd.args(["-s", serial, "shell", "getprop", "ro.build.version.release"]);
    let output = process::output_with_timeout(cmd, Duration::from_secs(3))
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    clean_property(&String::from_utf8_lossy(&output.stdout))
}

fn clean_property(raw: &str) -> Option<String> {
    let value = raw.lines().next()?.trim();
    if value.is_empty() || value.len() > 32 || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_string())
}

/// Keeps only usable devices, their models and the unavailable rows. Pure so
/// the filter chain can be tested without spawning adb.
pub fn parse_device_list(
    output: &str,
) -> (Vec<String>, serde_json::Map<String, Value>, Vec<Value>) {
    let mut devices: Vec<String> = Vec::new();
    let mut device_models = serde_json::Map::new();
    let mut unavailable = Vec::new();
    for line in output.lines().skip(1) {
        let Some((serial, state)) = parse_adb_device_line(line) else {
            continue;
        };
        if state != "device" {
            // An unauthorized phone is physically connected and showing the
            // "Allow USB debugging?" prompt; dropping it here made a first
            // run look like no device at all.
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
    (devices, device_models, unavailable)
}

/// Lists authorized devices plus the unavailable ones (unauthorized/offline).
pub async fn list_devices(custom_path: Option<&str>) -> Value {
    let adb_path = paths::binary_path("adb", custom_path);
    let mut cmd = process::create_command(&adb_path);
    cmd.arg("devices").arg("-l");
    let output = process::output_with_timeout(cmd, Duration::from_secs(10)).await;

    match output {
        Ok(output) => {
            if output.status.success() {
                let out_str = String::from_utf8_lossy(&output.stdout);
                let (devices, device_models, unavailable) = parse_device_list(&out_str);
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
        Err(error) => json!({ "error": true, "message": error.to_string() }),
    }
}

/// Lists mDNS services advertised by wireless debugging, de-duplicated by the
/// full `name|service|address` triple.
pub async fn list_mdns_services(custom_path: Option<&str>) -> Value {
    let adb_path = paths::binary_path("adb", custom_path);
    let mut cmd = process::create_command(&adb_path);
    cmd.arg("mdns").arg("services");
    let output = process::output_with_timeout(cmd, Duration::from_secs(10)).await;

    match output {
        Ok(output) => {
            if output.status.success() {
                let out_str = String::from_utf8_lossy(&output.stdout);
                let mut services = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for line in out_str.lines().skip(1) {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 3 {
                        let name = parts[0].trim();
                        let service = parts[1].trim();
                        let address = parts[2].trim();
                        let key = format!("{}|{}|{}", name, service, address);
                        if seen.insert(key) {
                            services.push(json!({
                                "name": name,
                                "service": service,
                                "address": address
                            }));
                        }
                    }
                }
                json!({ "error": false, "services": services })
            } else {
                json!({ "error": true, "message": "ADB mdns returned error" })
            }
        }
        Err(error) => json!({ "error": true, "message": error.to_string() }),
    }
}

/// Connects ADB to a wireless address. `log` receives the same progress and
/// error lines the shell used to emit; `silent` suppresses the pre-connect
/// message and per-line output, but failures still return a result.
pub async fn connect(
    log: impl Fn(String),
    ip: &str,
    custom_path: Option<&str>,
    silent: bool,
) -> Result<Value, String> {
    validate::adb_address(ip)?;
    let adb_path = paths::binary_path("adb", custom_path);
    if !silent {
        log(format!(
            "[SYSTEM] Attempting wireless connection to {}...",
            ip
        ));
    }

    let mut cmd = process::create_command(&adb_path);
    cmd.arg("connect").arg(ip);
    let output_res = process::output_with_timeout(cmd, Duration::from_secs(5)).await;

    match output_res {
        Ok(output) => {
            let out_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let err_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if !silent {
                if !out_text.is_empty() {
                    log(format!("[ADB] {}", out_text));
                }
                if !err_text.is_empty() {
                    log(format!("[ADB ERROR] {}", err_text));
                }
            }
            let success = output.status.success()
                && !out_text.contains("cannot connect")
                && !out_text.contains("failed");
            Ok(json!({
                "success": success,
                "message": if out_text.is_empty() { err_text } else { out_text },
            }))
        }
        Err(error) => {
            if !silent {
                log(format!("[SYSTEM] Connection to {} failed: {}.", ip, error));
            }
            Ok(json!({ "success": false, "message": error }))
        }
    }
}

/// Pairs with Android 11+ wireless debugging. The code is passed straight to
/// `adb pair`; adb decides whether it is valid.
pub async fn pair(
    log: impl Fn(String),
    ip: &str,
    code: &str,
    custom_path: Option<&str>,
) -> Result<Value, String> {
    validate::adb_address(ip)?;
    let adb_path = paths::binary_path("adb", custom_path);
    log(format!("[SYSTEM] Pairing with {}...", ip));

    let mut cmd = process::create_command(&adb_path);
    cmd.arg("pair").arg(ip).arg(code);
    let output = process::output_with_timeout(cmd, Duration::from_secs(10)).await?;

    let out_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let err_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !out_text.is_empty() {
        log(format!("[ADB] {}", out_text));
    }
    if !err_text.is_empty() {
        log(format!("[ADB ERROR] {}", err_text));
    }

    let success = output.status.success()
        && (out_text.contains("Successfully paired") || err_text.contains("Successfully paired"));
    Ok(json!({
        "success": success,
        "message": if out_text.is_empty() { err_text } else { out_text },
    }))
}

/// Kills the shared adb server. The frontend uses this as a recovery action.
pub async fn kill_server(log: impl Fn(String), custom_path: Option<&str>) -> Value {
    let adb_path = paths::binary_path("adb", custom_path);
    log("[SYSTEM] Terminating ADB stack...".to_string());

    let mut cmd = process::create_command(&adb_path);
    cmd.arg("kill-server");
    match process::output_with_timeout(cmd, Duration::from_secs(5)).await {
        Ok(output) if output.status.success() => {
            log("[SYSTEM] ADB Stack Terminated.".to_string());
            json!({ "success": true, "message": "ADB Stack Terminated" })
        }
        Ok(output) => {
            let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if err.is_empty() {
                "adb kill-server failed".to_string()
            } else {
                err
            };
            log(format!("[ADB ERROR] {message}"));
            json!({ "success": false, "message": message })
        }
        Err(error) => json!({ "success": false, "message": error }),
    }
}

/// Runs one shell command on an authorized device. The output is capped by the
/// terminal policy; failures come back as JSON, not as a command error.
pub async fn shell(device: &str, command: &str, custom_path: Option<&str>) -> Value {
    if let Err(error) = validate::device(device) {
        return json!({ "success": false, "message": error });
    }
    let adb_path = paths::binary_path("adb", custom_path);

    let mut cmd = process::create_command(&adb_path);
    cmd.arg("-s").arg(device).arg("shell").arg(command);
    let output = process::output_with_timeout_capped(
        cmd,
        Duration::from_secs(10),
        terminal::MAX_TERMINAL_OUTPUT_BYTES,
    )
    .await;

    match output {
        Ok((output, stdout_truncated, _)) => json!({
            "success": output.status.success(),
            "output": terminal::terminal_output(&output.stdout, stdout_truncated)
        }),
        Err(error) => json!({ "success": false, "message": error.to_string() }),
    }
}

/// Pushes a local file into the device's Download directory.
pub async fn push_file(device: &str, file_path: &str, custom_path: Option<&str>) -> Value {
    if let Err(error) = validate::device(device) {
        return json!({ "success": false, "message": error });
    }
    let adb_path = paths::binary_path("adb", custom_path);

    let output = process::create_command(&adb_path)
        .arg("-s")
        .arg(device)
        .arg("push")
        .arg(file_path)
        .arg("/sdcard/Download/")
        .output()
        .await;

    match output {
        Ok(output) => {
            if output.status.success() {
                json!({ "success": true, "message": "File pushed to Downloads" })
            } else {
                // adb reports the real cause on stderr ("device offline",
                // "No space left"); collapsing every failure to one string
                // left the log unable to tell them apart.
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let detail = if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                };
                let message = if detail.is_empty() {
                    format!("Transfer failed (exit code {:?})", output.status.code())
                } else {
                    format!("Transfer failed: {}", detail)
                };
                json!({ "success": false, "message": message })
            }
        }
        Err(error) => json!({ "success": false, "message": error.to_string() }),
    }
}

/// Installs an APK through `adb install`; stdout/stderr are the user-facing
/// result exactly as before.
pub async fn install_apk(device: &str, file_path: &str, custom_path: Option<&str>) -> Value {
    if let Err(error) = validate::device(device) {
        return json!({ "success": false, "message": error });
    }
    let adb_path = paths::binary_path("adb", custom_path);

    let output = process::create_command(&adb_path)
        .arg("-s")
        .arg(device)
        .arg("install")
        .arg(file_path)
        .output()
        .await;

    match output {
        Ok(output) => {
            let out_text = String::from_utf8_lossy(&output.stdout);
            let err_text = String::from_utf8_lossy(&output.stderr);
            if output.status.success() {
                json!({ "success": true, "message": out_text.trim() })
            } else {
                json!({ "success": false, "message": err_text.trim() })
            }
        }
        Err(error) => json!({ "success": false, "message": error.to_string() }),
    }
}

/// Shell helper: true when `serial` is listed as an authorized device.
pub fn device_is_online(output: &str, serial: &str) -> bool {
    output.lines().skip(1).any(|line| {
        parse_adb_device_line(line)
            .is_some_and(|(parsed, state)| parsed == serial && state == "device")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_getprop_output() {
        assert_eq!(clean_property("15\n"), Some("15".to_string()));
        assert_eq!(clean_property("  14  \r\n"), Some("14".to_string()));
        assert_eq!(clean_property("\n"), None);
        assert_eq!(clean_property("15\u{0007}"), None);
        assert_eq!(clean_property(&"9".repeat(40)), None);
    }

    #[test]
    fn parses_usb_serial() {
        assert_eq!(
            parse_adb_device_line("ZY22MGW35T device"),
            Some(("ZY22MGW35T".to_string(), "device".to_string()))
        );
    }

    #[test]
    fn parses_tcpip_serial() {
        assert_eq!(
            parse_adb_device_line("192.168.1.50:5555 device"),
            Some(("192.168.1.50:5555".to_string(), "device".to_string()))
        );
    }

    #[test]
    fn parses_mdns_serial() {
        assert_eq!(
            parse_adb_device_line("adb-ZY22MGW35T-Z3uXXq._adb-tls-connect._tcp device"),
            Some((
                "adb-ZY22MGW35T-Z3uXXq._adb-tls-connect._tcp".to_string(),
                "device".to_string()
            ))
        );
    }

    #[test]
    fn parses_mdns_conflict_serial_with_space() {
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
    fn ignores_trailing_l_fields() {
        assert_eq!(
            parse_adb_device_line(
                "ZY22MGW35T device usb:1-1 product:foo model:bar device:baz transport_id:1"
            ),
            Some(("ZY22MGW35T".to_string(), "device".to_string()))
        );
    }

    #[test]
    fn parses_unavailable_states() {
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
    fn extracts_model_and_turns_underscores_into_spaces() {
        assert_eq!(
            extract_device_model(
                "ZY22MGW35T device usb:1-1 product:foo model:SM_S911B device:baz transport_id:1"
            ),
            Some("SM S911B".to_string())
        );
    }

    #[test]
    fn missing_model_field_returns_none() {
        assert_eq!(extract_device_model("ZY22MGW35T device"), None);
    }

    #[test]
    fn unparsable_lines_are_ignored() {
        assert_eq!(parse_adb_device_line(""), None);
        assert_eq!(parse_adb_device_line("List of devices attached"), None);
    }

    #[test]
    fn device_list_keeps_only_the_device_state() {
        let output = "List of devices attached\n\
             ZY22MGW35T device usb:1-1 product:foo model:bar transport_id:1\n\
             192.168.1.50:5555 device product:foo model:bar transport_id:2\n\
             adb-ZY22MGW35T-Z3uXXq._adb-tls-connect._tcp device product:foo transport_id:3\n\
             adb-ZY22MGW35T-Z3uXXq (2)._adb-tls-connect._tcp device product:foo transport_id:4\n\
             emulator-5554 offline\n\
             0123456789ABCDEF unauthorized\n";
        let (devices, models, unavailable) = parse_device_list(output);
        assert_eq!(
            devices,
            vec![
                "ZY22MGW35T".to_string(),
                "192.168.1.50:5555".to_string(),
                "adb-ZY22MGW35T-Z3uXXq._adb-tls-connect._tcp".to_string(),
                "adb-ZY22MGW35T-Z3uXXq (2)._adb-tls-connect._tcp".to_string(),
            ]
        );
        assert_eq!(models.get("ZY22MGW35T"), Some(&json!("bar")));
        assert_eq!(unavailable.len(), 2);
    }

    #[test]
    fn device_is_online_only_accepts_the_device_state() {
        let output =
            "List of devices attached\nZY22MGW35T device\n192.168.1.50:5555 unauthorized\n";
        assert!(device_is_online(output, "ZY22MGW35T"));
        assert!(!device_is_online(output, "192.168.1.50:5555"));
        assert!(!device_is_online(output, "missing"));
    }
}
