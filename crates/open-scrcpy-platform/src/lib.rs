//! Native platform services shared by the UI shells.
//!
//! Today this owns the KDE/KWin Wayland bridge and the geometry of the
//! external `scrcpy` mirror window; the Windows drag helper and the
//! tray/updater adapters move here as Phase 1 progresses.

#[cfg(target_os = "linux")]
pub mod kwin;
pub mod window;
