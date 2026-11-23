//! Layout direction follows the active locale.
//!
//! GPUI Kit does not mirror layout on its own; `gpui-base` exposes a global
//! [`LayoutDirection`] plus logical styling helpers (`flex_row_dir`, `border_e`,
//! `ps`/`pe`, `text_start`). This module owns the global switch; the shell uses
//! the helpers on its structural rows.

use gpui_kit::App;
use gpui_kit::base::LayoutDirection;

/// Applies the direction of `locale` (`ar` → RTL) and refreshes the windows.
pub fn apply(locale: &str, cx: &mut App) {
    LayoutDirection::from_locale(locale).set_global(cx);
}
