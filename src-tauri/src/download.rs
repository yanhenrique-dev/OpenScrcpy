//! Tauri adapter for the extracted download module.
//!
//! The domain logic lives in `open_scrcpy_core::download`; this file only
//! resolves the shell-specific app-data directory and otherwise re-exports it.

pub use open_scrcpy_core::download::{download_latest, install_dir};
use std::path::PathBuf;

/// Resolve the install directory for the Tauri shell. Portable installs still
/// win inside the core; this only supplies the app-data fallback.
pub fn init_install_dir(app: &tauri::AppHandle) -> PathBuf {
    use tauri::Manager;

    open_scrcpy_core::download::init_install_dir(app.path().app_data_dir().ok())
}
