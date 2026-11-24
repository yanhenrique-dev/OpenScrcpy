//! Accessibility audit over the real AccessKit tree.
//!
//! The shell must not expose interactive controls without an accessible name.
//! This test walks every observed element in three states (main view, language
//! dialog, About dialog) so a new unlabeled control fails the suite.

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext as _, Bounds, Role, TestAppContext, WindowBounds, WindowOptions, init as kit_init,
    px, size,
};
use openscrcpy_gpui::{AppServices, AppSettings, AppShell, SettingsStore, ThemePreference};

fn open_shell(cx: &mut TestAppContext) -> gpui_kit::AnyWindowHandle {
    rust_i18n::set_locale("en");
    gpui_kit::component::set_locale("en");
    cx.update(kit_init);

    let settings = AppSettings {
        locale: "en".into(),
        theme: ThemePreference::Light,
        ..AppSettings::default()
    };
    let (services, events) = AppServices::for_test();
    let (_platform_tx, platform_commands) = tokio::sync::mpsc::unbounded_channel();
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1200.), px(890.)),
                ))),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                cx.new(|cx| {
                    AppShell::new(
                        window,
                        cx,
                        settings.clone(),
                        SettingsStore::without_persistence(),
                        services,
                        events,
                        platform_commands,
                    )
                })
            },
        )
        .expect("open the shell window")
    });
    handle
}

fn is_interactive(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::DefaultButton
            | Role::CheckBox
            | Role::RadioButton
            | Role::Switch
            | Role::TextInput
            | Role::MultilineTextInput
            | Role::ComboBox
            | Role::Link
            | Role::MenuItem
            | Role::Slider
    )
}

fn assert_interactive_elements_are_named(window: &gpui_kit::Window, state: &str) {
    let elements = window.elements();
    let interactive: Vec<_> = elements
        .iter()
        .filter(|element| element.role().is_some_and(is_interactive))
        .collect();
    assert!(
        !interactive.is_empty(),
        "{state}: no interactive elements were observed"
    );
    for element in interactive {
        let label = element.label().unwrap_or_default();
        assert!(
            !label.trim().is_empty(),
            "{state}: unlabeled {:?} at {:?}",
            element.role(),
            element.path()
        );
    }
}

#[gpui_kit::test]
fn every_interactive_control_has_an_accessible_name(cx: &mut TestAppContext) {
    let handle = open_shell(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_interactive_elements_are_named(window, "main");
        assert!(
            !window.find_all_by_role(Role::Heading).is_empty(),
            "the panel title must stay a heading"
        );

        window.click("locale-toggle", cx);
        window.render_frame(cx);
        assert_interactive_elements_are_named(window, "language dialog");
        window.close_dialog(cx);
        window.render_frame(cx);

        window.click("app-settings", cx);
        window.render_frame(cx);
        assert_interactive_elements_are_named(window, "settings sheet");
        window.close_sheet(cx);
        window.render_frame(cx);

        window.click("app-settings", cx);
        window.render_frame(cx);
        window.click("settings-about", cx);
        window.render_frame(cx);
        assert_interactive_elements_are_named(window, "about dialog");
    })
    .expect("window must stay open");
}
