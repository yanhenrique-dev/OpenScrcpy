//! System tray and the "keep running in the background" window policy.
//!
//! The tray lives in Rust, not in the webview, so hiding the main window never
//! takes the restore/quit affordance with it. Labels are localized by the
//! frontend and validated before they reach a native menu.

use std::sync::atomic::Ordering;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Window, WindowEvent};

use crate::ScrcpyState;

const TRAY_ID: &str = "main";
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";

pub(crate) struct TrayLabels {
    pub show: String,
    pub quit: String,
}

/// Creates or updates the tray when `enabled`, and hides it otherwise.
pub(crate) fn configure(app: &AppHandle, enabled: bool, labels: &TrayLabels) -> Result<(), String> {
    if !enabled {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            // Hide instead of recreating: Tauri keeps global menu listeners
            // for the process lifetime, and a create/remove cycle would add
            // a new handler on every toggle.
            tray.set_visible(false).map_err(|error| error.to_string())?;
        }
        return Ok(());
    }

    // libappindicator-sys dlopens the Ayatana library and panics when it is
    // absent. Probe first so the frontend can revert the preference and show
    // an explanation instead of losing the whole app.
    #[cfg(target_os = "linux")]
    if !appindicator_available() {
        return Err(
            "system tray is unavailable: install libayatana-appindicator3 or libappindicator3"
                .to_string(),
        );
    }

    let menu = build_menu(app, labels)?;
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_menu(Some(menu))
            .map_err(|error| error.to_string())?;
        tray.set_visible(true).map_err(|error| error.to_string())?;
        return Ok(());
    }

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "application icon is unavailable".to_string())?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("OpenScrcpy")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            MENU_SHOW => show_main_window(app),
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .build(app)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Brings the main window back from the tray (and from a minimized state).
pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Hides instead of closing the main window while background mode is on.
/// Other windows keep their normal close behavior.
pub(crate) fn on_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != "main" {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        let state = window.state::<ScrcpyState>();
        if state.background_mode.load(Ordering::Relaxed) {
            api.prevent_close();
            let _ = window.hide();
        }
    }
}

fn build_menu(app: &AppHandle, labels: &TrayLabels) -> Result<Menu<tauri::Wry>, String> {
    let show = MenuItem::with_id(app, MENU_SHOW, &labels.show, true, None::<&str>)
        .map_err(|error| error.to_string())?;
    let quit = MenuItem::with_id(app, MENU_QUIT, &labels.quit, true, None::<&str>)
        .map_err(|error| error.to_string())?;
    Menu::with_items(app, &[&show, &quit]).map_err(|error| error.to_string())
}

/// The tray crate loads the indicator library dynamically: a missing library
/// only fails at runtime, not at build time.
#[cfg(target_os = "linux")]
fn appindicator_available() -> bool {
    ["libayatana-appindicator3.so.1", "libappindicator3.so.1"]
        .iter()
        .any(|name| {
            // SAFETY: dlopen only probes the library; the handle is dropped
            // immediately and no symbol from it is called here.
            unsafe { libloading::Library::new(name) }.is_ok()
        })
}
