use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tar::Archive;

static INSTALL_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Asset tag and archive extension for the running platform. Linux arm64 has
/// no official scrcpy build, so it fails with a message instead of silently
/// downloading the x86_64 archive.
pub fn resolve_asset(os: &str, arch: &str) -> Result<(&'static str, &'static str), String> {
    match (os, arch) {
        ("linux", "x86_64") => Ok(("linux-x86_64", ".tar.gz")),
        ("linux", "aarch64") => Err(
            "No official scrcpy build for Linux arm64. Install scrcpy from your distribution package manager."
                .to_string(),
        ),
        ("windows", "x86_64") => Ok(("win64", ".zip")),
        ("windows", "aarch64") => Ok(("winarm64", ".zip")),
        ("windows", "x86") => Ok(("win32", ".zip")),
        ("macos", "x86_64") => Ok(("macos-x86_64", ".tar.gz")),
        ("macos", "aarch64") => Ok(("macos-aarch64", ".tar.gz")),
        _ => Err(format!(
            "Unsupported platform for auto-download: {} ({})",
            os, arch
        )),
    }
}

/// Find the checksum for `filename` in a SHA256SUMS.txt body. A matching line
/// with a malformed hash is an error, not a skip: the release file is the
/// only integrity signal we have.
pub fn parse_sha256sums(text: &str, filename: &str) -> Result<String, String> {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if name.trim_start_matches('*') != filename {
            continue;
        }
        if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(hash.to_ascii_lowercase());
        }
        return Err(format!("Malformed SHA256 entry for {}", filename));
    }
    Err(format!("SHA256SUMS.txt has no entry for {}", filename))
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open archive for checksum: {}", e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn exe_name(base: &str) -> String {
    let ext = std::env::consts::EXE_EXTENSION;
    if ext.is_empty() {
        base.to_string()
    } else {
        format!("{}.{}", base, ext)
    }
}

/// Resolved once in `setup`: the executable directory in portable mode when
/// it is writable, otherwise the per-user app data directory.
/// Resolve and remember the install directory. `app_data_dir` is supplied by
/// the shell (Tauri today, GPUI at cutover); the core never reads app handles.
pub fn init_install_dir(app_data_dir: Option<PathBuf>) -> PathBuf {
    let portable = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .filter(|dir| is_writable(dir))
        .map(|dir| dir.join("scrcpy-bin"));
    let chosen = portable
        .or_else(|| app_data_dir.map(|dir| dir.join("scrcpy-bin")))
        .unwrap_or_else(|| std::env::temp_dir().join("openscrcpy").join("scrcpy-bin"));
    let _ = INSTALL_DIR.set(chosen.clone());
    chosen
}

pub fn install_dir() -> Option<&'static Path> {
    INSTALL_DIR.get().map(PathBuf::as_path)
}

fn is_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn extract_archive(archive: &Path, dest: &Path, extension: &str) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    if extension == ".zip" {
        let file =
            std::fs::File::open(archive).map_err(|e| format!("Failed to open zip: {}", e))?;
        let mut zip =
            zip::ZipArchive::new(file).map_err(|e| format!("Failed to read zip archive: {}", e))?;
        zip.extract(dest)
            .map_err(|e| format!("Failed to extract: {}", e))?;
    } else {
        let file =
            std::fs::File::open(archive).map_err(|e| format!("Failed to open tar.gz: {}", e))?;
        let tar = GzDecoder::new(file);
        let mut archive = Archive::new(tar);
        archive
            .unpack(dest)
            .map_err(|e| format!("Failed to extract tar: {}", e))?;
    }
    Ok(())
}

/// Release archives usually contain a single root folder; Windows zips put
/// the files at the top level. Both are flattened to the same target.
pub fn single_root(extract_dir: &Path) -> Result<PathBuf, String> {
    let mut entries = std::fs::read_dir(extract_dir).map_err(|e| e.to_string())?;
    let first = entries.next();
    let has_second = entries.next().is_some();
    match first {
        Some(entry) => {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() && !has_second {
                Ok(path)
            } else {
                Ok(extract_dir.to_path_buf())
            }
        }
        None => Err("Downloaded archive was empty".to_string()),
    }
}

pub fn validate_extracted(dir: &Path, adb_expected: bool) -> Result<(), String> {
    if !dir.join(exe_name("scrcpy")).is_file() {
        return Err("Downloaded archive did not contain the expected scrcpy binary".to_string());
    }
    if adb_expected && !dir.join(exe_name("adb")).is_file() {
        return Err("Downloaded archive did not contain the expected adb binary".to_string());
    }
    Ok(())
}

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    let src_ref = src.as_ref();
    let dst_ref = dst.as_ref();

    let canonical_dst = if dst_ref.exists() {
        dst_ref.canonicalize()?
    } else {
        std::fs::create_dir_all(dst_ref)?;
        dst_ref.canonicalize()?
    };

    for entry in std::fs::read_dir(src_ref)? {
        let entry = entry?;
        let entry_path = entry.path();
        let file_name = entry.file_name();

        // Prevent directory traversal elements in entry name
        let file_name_str = file_name.to_string_lossy();
        if file_name_str.contains("..")
            || file_name_str.contains('/')
            || file_name_str.contains('\\')
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Directory entry contains directory traversal components",
            ));
        }

        let target_path = dst_ref.join(&file_name);

        // Ensure destination path is strictly within the target base directory
        if let Some(parent_dir) = target_path.parent() {
            if parent_dir.exists() {
                let canonical_parent = parent_dir.canonicalize()?;
                if !canonical_parent.starts_with(&canonical_dst)
                    && canonical_parent != canonical_dst
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "Path traversal attempt detected during directory copy",
                    ));
                }
            }
        }

        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry_path, &target_path)?;
        } else {
            std::fs::copy(&entry_path, &target_path)?;
        }
    }
    Ok(())
}

/// Removes every listed path unless disarmed. Used so a failed download or
/// extraction cannot leave `scrcpy-bin.new`, a partial archive or the extract
/// directory behind.
pub struct TempCleanup {
    paths: Vec<PathBuf>,
    armed: bool,
}

impl TempCleanup {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        for path in &paths {
            remove_path(path);
        }
        Self { paths, armed: true }
    }

    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for path in &self.paths {
            remove_path(path);
        }
    }
}

fn remove_path(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

// Scrcpy download/install pipeline: GitHub release discovery, checksum
// verification, staged extraction and atomic swap.

use std::time::Duration;

use serde_json::Value;

use crate::events::EventBus;
use crate::process::create_command;
use crate::state::ScrcpyState;

fn log(events: Option<&EventBus>, line: impl Into<String>) {
    if let Some(bus) = events {
        bus.log(line);
    }
}

fn status(events: Option<&EventBus>, payload: Value) {
    if let Some(bus) = events {
        bus.status(payload);
    }
}

/// Only accept asset names matching the expected scrcpy release pattern.
fn is_expected_asset_name(name: &str, extension: &str) -> bool {
    name.starts_with("scrcpy-")
        && name.ends_with(extension)
        && name.len() < 200
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

pub async fn download_latest(
    events: Option<&EventBus>,
    state: &ScrcpyState,
    install_path: &std::path::Path,
) -> Result<(), String> {
    let (arch_tag, extension) = resolve_asset(std::env::consts::OS, std::env::consts::ARCH)?;

    log(
        events,
        format!(
            "[SYSTEM] Detecting platform: {} ({})",
            std::env::consts::OS,
            arch_tag
        ),
    );
    status(
        events,
        serde_json::json!({ "type": "downloading", "success": true, "message": format!("Fetching latest {} release...", arch_tag) }),
    );

    let client = crate::update::http_client("OpenScrcpy-Downloader")?;

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
            log(
                events,
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
                log(
                    events,
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

    log(events, format!("[SYSTEM] Found asset: {}", filename));

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
    let expected_hash = parse_sha256sums(&sums_text, &filename)?;

    let install_path = install_path.to_path_buf();
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
    let mut cleanup = TempCleanup::new(vec![
        temp_archive_path.clone(),
        temp_extract_dir.clone(),
        new_dir.clone(),
    ]);

    {
        let mut file = std::fs::File::create(&temp_archive_path)
            .map_err(|e| format!("Failed to create archive file: {}", e))?;
        let mut download_resp =
            tokio::time::timeout(Duration::from_secs(30), client.get(&download_url).send())
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

        log(
            events,
            format!("[SYSTEM] Downloading: {} MB", total_size / 1024 / 1024),
        );

        let mut downloaded: u64 = 0;
        let mut last_percent: Option<u64> = None;
        loop {
            // A stalled transfer must not keep the download flag on forever.
            // The timeout is per chunk, not for the whole file, so a slow but
            // healthy link still gets to finish.
            let chunk = tokio::time::timeout(Duration::from_secs(30), download_resp.chunk())
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
                    status(
                        events,
                        serde_json::json!({ "type": "download-progress", "percent": percent }),
                    );
                }
            }
        }
    }

    let actual_hash = sha256_file(&temp_archive_path)?;
    if actual_hash != expected_hash {
        return Err(format!(
            "Checksum mismatch for {}: expected {}, got {}",
            filename, expected_hash, actual_hash
        ));
    }

    log(events, "[SYSTEM] Download finished. Starting extraction...");
    status(
        events,
        serde_json::json!({ "type": "downloading", "success": true, "message": "Extracting binaries..." }),
    );

    log(
        events,
        format!(
            "[SYSTEM] Decompressing {} archive...",
            if extension == ".zip" { "ZIP" } else { "TAR.GZ" }
        ),
    );
    extract_archive(&temp_archive_path, &temp_extract_dir, extension)?;

    let source_root = single_root(&temp_extract_dir)?;
    let needs_adb = source_root.join(exe_name("adb")).is_file();

    if std::fs::rename(&source_root, &new_dir).is_err() {
        copy_dir_all(&source_root, &new_dir)
            .map_err(|e| format!("Failed to stage extracted files: {}", e))?;
    }
    validate_extracted(&new_dir, needs_adb)?;

    // A live session keeps the old binaries mapped, and on Windows can lock
    // the install directory so the next update fails to clear its backup.
    // Refuse instead of swapping underneath a running mirror. Holding
    // `binary_swap` until the swap finishes also stops a new session from
    // slipping between the check and the rename.
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
    let mut adb_stop = create_command(crate::paths::binary_path("adb", None));
    adb_stop.arg("kill-server");
    if let Err(e) = crate::process::output_with_timeout(adb_stop, Duration::from_secs(5)).await {
        log(
            events,
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

    status(
        events,
        serde_json::json!({ "type": "download-complete", "success": true, "message": install_path.to_string_lossy() }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_real_release_asset_names() {
        assert!(is_expected_asset_name("scrcpy-win64-v3.3.3.zip", ".zip"));
        assert!(is_expected_asset_name(
            "scrcpy-linux-x86_64-v2.7.tar.gz",
            ".tar.gz"
        ));
    }

    #[test]
    fn rejects_unexpected_asset_names() {
        assert!(!is_expected_asset_name("scrcpy-win64-v3.3.3.exe", ".zip"));
        assert!(!is_expected_asset_name("../evil.zip", ".zip"));
        assert!(!is_expected_asset_name("/abs/scrcpy-win64-v1.zip", ".zip"));
        assert!(!is_expected_asset_name("scrcpy-win64-v1.zip ", ".zip"));
        assert!(!is_expected_asset_name("scrcpy-v1/../../evil.zip", ".zip"));
        let oversized = format!("scrcpy-{}.zip", "a".repeat(200));
        assert!(!is_expected_asset_name(&oversized, ".zip"));
    }

    #[test]
    fn resolve_asset_maps_every_supported_platform() {
        assert_eq!(
            resolve_asset("linux", "x86_64").unwrap(),
            ("linux-x86_64", ".tar.gz")
        );
        assert_eq!(
            resolve_asset("windows", "x86_64").unwrap(),
            ("win64", ".zip")
        );
        assert_eq!(
            resolve_asset("windows", "aarch64").unwrap(),
            ("winarm64", ".zip")
        );
        assert_eq!(resolve_asset("windows", "x86").unwrap(), ("win32", ".zip"));
        assert_eq!(
            resolve_asset("macos", "x86_64").unwrap(),
            ("macos-x86_64", ".tar.gz")
        );
        assert_eq!(
            resolve_asset("macos", "aarch64").unwrap(),
            ("macos-aarch64", ".tar.gz")
        );
        assert!(resolve_asset("linux", "aarch64").is_err());
        assert!(resolve_asset("freebsd", "x86_64").is_err());
    }

    #[test]
    fn parse_sha256sums_finds_the_asset_line() {
        let sums = "\
aaaabbbbccccddddeeeeffff0000111122223333444455556666777788889999  scrcpy-linux-x86_64-v3.3.3.tar.gz
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef *scrcpy-win64-v3.3.3.zip
";
        assert_eq!(
            parse_sha256sums(sums, "scrcpy-win64-v3.3.3.zip").unwrap(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert!(parse_sha256sums(sums, "scrcpy-win64-v9.9.9.zip").is_err());
    }

    #[test]
    fn parse_sha256sums_rejects_a_malformed_hash() {
        let sums = "not-a-hash  scrcpy-win64-v3.3.3.zip\n";
        assert!(parse_sha256sums(sums, "scrcpy-win64-v3.3.3.zip").is_err());
    }
}
