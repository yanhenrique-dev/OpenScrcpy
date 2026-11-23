use gpui_kit::component::ActiveTheme as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Bounds, TestAppContext, WindowBounds, WindowOptions, init as kit_init, px,
    size,
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
                    )
                })
            },
        )
        .expect("open the shell window")
    });
    handle
}

#[gpui_kit::test]
fn shell_switches_modes_theme_and_locale(cx: &mut TestAppContext) {
    let handle = open_shell(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("panel-title").label(), Some("Screen"));

        // Mode tabs re-render the panel heading.
        window.click("mode-camera", cx);
        window.render_frame(cx);
        assert_eq!(window.find("panel-title").label(), Some("Camera"));

        // The wireless transport is part of the sidebar contract.
        assert_eq!(window.find("wireless-connect").label(), Some("Connect"));
        assert!(window.find("pair-start").label().is_some());

        // Camera mode configures the lens, torch and zoom before starting.
        assert_eq!(window.find("start-mission").label(), Some("Start camera"));
        let summary = window
            .find("camera-summary")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(summary.contains("Back"), "unexpected summary: {summary}");
        assert!(summary.contains("1.0×"), "unexpected summary: {summary}");
        window.click("camera-torch", cx);
        window.render_frame(cx);
        let summary = window
            .find("camera-summary")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(summary.contains("Torch"), "unexpected summary: {summary}");

        // Desktop mode has its own panel and turns the same session bar into a
        // virtual-display start.
        window.click("mode-desktop", cx);
        window.render_frame(cx);
        assert_eq!(window.find("panel-title").label(), Some("Desktop"));
        assert_eq!(
            window.find("start-mission").label(),
            Some("Start virtual display")
        );
        let summary = window
            .find("desktop-summary")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(
            summary.contains("1920×1080 @ 420 DPI"),
            "unexpected summary: {summary}"
        );
        window.click("desktop-flex", cx);
        window.render_frame(cx);
        let summary = window
            .find("desktop-summary")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(summary.contains("Flex"), "unexpected summary: {summary}");

        // Mic mode starts the windowless bridge and shares the session bar.
        window.click("mode-microphone", cx);
        window.render_frame(cx);
        assert_eq!(window.find("panel-title").label(), Some("Microphone"));
        assert_eq!(
            window.find("start-mission").label(),
            Some("Start Mic Bridge")
        );
        let summary = window
            .find("mic-summary")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(
            summary.contains("128 kbps"),
            "unexpected summary: {summary}"
        );
        #[cfg(unix)]
        {
            window.click("mic-denoise", cx);
            window.render_frame(cx);
            let summary = window
                .find("mic-summary")
                .label()
                .unwrap_or_default()
                .to_string();
            assert!(summary.contains("RNNoise"), "unexpected summary: {summary}");
        }

        // Filter/settings controls write the persisted mirror settings and the
        // summary reflects them before the session starts.
        window.click("mode-screen", cx);
        window.render_frame(cx);
        let summary = window
            .find("mirror-summary")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(summary.contains("8 Mbps"), "unexpected summary: {summary}");
        assert!(summary.contains("H.264"), "unexpected summary: {summary}");
        assert!(window.find("mirror-record").label().is_some());
        assert!(window.find("mirror-hid-keyboard").label().is_some());

        window.click("mirror-audio", cx);
        window.render_frame(cx);
        let summary = window
            .find("mirror-summary")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(
            summary.contains("Audio off"),
            "unexpected summary: {summary}"
        );

        window.click("mirror-audio", cx);
        window.render_frame(cx);

        // Choosing a theme repaints the window with the new palette.
        let light_background = cx.theme().background;
        window.click("theme-dark", cx);
        window.render_frame(cx);
        assert_ne!(cx.theme().background, light_background);

        // Locale switching retranslates the whole window without a restart.
        // Keep it last: the switch is process-visible to later UI work.
        let label = window
            .find("locale-toggle")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(label.contains("English"), "unexpected label: {label}");

        window.click("locale-toggle", cx);
        window.render_frame(cx);
        let label = window
            .find("locale-toggle")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(label.contains("Français"), "unexpected label: {label}");
        assert_eq!(window.find("mode-screen").label(), Some("Écran"));
    })
    .expect("window must stay open");
}
