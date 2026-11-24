//! Binary lookup shared by the shells.
//!
//! Order is deliberate: an explicit custom folder wins, then a portable
//! `scrcpy-bin` beside the executable, then the install directory resolved at
//! startup, then the working directory, and finally the bare name so the OS
//! `PATH` can resolve it.

use std::path::Path;

use crate::download;

pub fn binary_path(binary_name: &str, custom_folder: Option<&str>) -> String {
    let exe_ext = std::env::consts::EXE_EXTENSION;
    let binary_filename = if exe_ext.is_empty() {
        binary_name.to_string()
    } else {
        format!("{}.{}", binary_name, exe_ext)
    };

    if let Some(folder) = custom_folder {
        if !folder.trim().is_empty() {
            let full_path = Path::new(folder).join(&binary_filename);
            if full_path.exists() && full_path.is_file() {
                return full_path.to_string_lossy().to_string();
            }
        }
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let local_bin = exe_dir.join("scrcpy-bin").join(&binary_filename);
            if local_bin.exists() && local_bin.is_file() {
                return local_bin.to_string_lossy().to_string();
            }
        }
    }

    if let Some(dir) = download::install_dir() {
        let installed = dir.join(&binary_filename);
        if installed.exists() && installed.is_file() {
            return installed.to_string_lossy().to_string();
        }
    }

    if let Ok(current_dir) = std::env::current_dir() {
        let local_bin = current_dir.join("scrcpy-bin").join(&binary_filename);
        if local_bin.exists() && local_bin.is_file() {
            return local_bin.to_string_lossy().to_string();
        }
    }

    binary_name.to_string()
}

/// Directory that holds the resolved scrcpy/adb binaries, falling back to the
/// executable directory when no `scrcpy-bin` folder exists.
pub fn scrcpy_bin_dir() -> Result<String, String> {
    if let Some(dir) = download::install_dir() {
        if dir.exists() {
            return Ok(dir.to_string_lossy().to_string());
        }
    }

    let current_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|path| path.to_path_buf()))
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "Could not determine application directory".to_string())?;
    let extract_path = current_dir.join("scrcpy-bin");
    if extract_path.exists() {
        Ok(extract_path.to_string_lossy().to_string())
    } else {
        Ok(current_dir.to_string_lossy().to_string())
    }
}
