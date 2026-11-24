//! Applies the saved theme preference to the Kit theme.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Window, WindowAppearance};

use crate::settings::ThemePreference;

/// Resolves `System` from the platform appearance and refreshes every window.
pub fn apply(preference: ThemePreference, window: Option<&mut Window>, cx: &mut App) {
    let mode = match preference {
        ThemePreference::Light => ThemeMode::Light,
        ThemePreference::Dark => ThemeMode::Dark,
        ThemePreference::System => {
            let appearance = cx.window_appearance();
            match appearance {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
                WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeMode::Light,
            }
        }
    };
    Theme::change(mode, window, cx);
}
