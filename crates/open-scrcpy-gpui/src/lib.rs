//! OpenScrcpy's native GPUI shell.
//!
//! Phase 2 foundation: window, theme, locale switching, persisted settings and
//! the shell layout. Session and device features arrive in Phase 3 and call the
//! UI-agnostic `open-scrcpy-core` crate.

rust_i18n::i18n!("locales", fallback = "en");

mod app;
mod i18n;
mod services;
mod settings;
mod theme;

pub use app::AppShell;
pub use i18n::{SUPPORTED_LOCALES, is_supported};
pub use services::AppServices;
pub use settings::{AppSettings, SettingsStore, ThemePreference};

use gpui_kit::{AppContext as _, WindowOptions, px, size};

pub fn run() {
    let store = SettingsStore::new();
    let mut settings = store.load();
    let locale = i18n::resolve(&settings.locale);
    settings.locale = locale.to_string();
    i18n::apply(locale);

    let (services, events) = AppServices::new();

    match open_scrcpy_platform::single_instance::acquire("openscrcpy-gpui") {
        Ok(Some(guard)) => std::mem::forget(guard),
        Ok(None) => {
            eprintln!("[single-instance] another OpenScrcpy window is already running");
            return;
        }
        Err(error) => eprintln!("[single-instance] unavailable: {error}"),
    }

    let (platform_tx, platform_commands) = tokio::sync::mpsc::unbounded_channel();
    match open_scrcpy_platform::shortcuts::register(platform_tx) {
        Ok(shortcuts) => std::mem::forget(shortcuts),
        Err(error) => eprintln!("[shortcuts] could not register Ctrl+Alt+Shift+C: {error}"),
    }

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_app_identity("dev.yanhenrique.openscrcpy", "OpenScrcpy");

            let options = WindowOptions {
                window_bounds: Some(settings.window_bounds(cx)),
                window_min_size: Some(size(px(settings::MIN_WIDTH), px(settings::MIN_HEIGHT))),
                app_id: Some("openscrcpy".to_string()),
                ..Default::default()
            };
            let settings = settings.clone();
            let store = store.clone();
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| {
                    AppShell::new(
                        window,
                        cx,
                        settings,
                        store,
                        services,
                        events,
                        platform_commands,
                    )
                })
            })
            .expect("failed to open the OpenScrcpy window");
        });
}
