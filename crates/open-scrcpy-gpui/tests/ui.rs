use gpui_kit::Role;
use gpui_kit::base::LayoutDirection;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext as _, Bounds, TestAppContext, WindowBounds, WindowOptions, init as kit_init, px,
    size,
};
use openscrcpy_gpui::{AppServices, AppSettings, AppShell, SettingsStore, ThemePreference};

fn open_shell(cx: &mut TestAppContext) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<AppShell>) {
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
    let (handle, view) = cx.update(|cx| {
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
    (handle, view)
}

fn switch_mode(
    view: &gpui_kit::Entity<AppShell>,
    index: usize,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
) {
    // TabBar tabs have no per-tab ElementId. Drive the same public selection
    // path a command uses, then render the resulting panel.
    view.update(cx, |shell, cx| shell.select_mode(index, cx));
    window.render_frame(cx);
}

#[gpui_kit::test]
fn shell_switches_modes_theme_and_locale(cx: &mut TestAppContext) {
    let (handle, view) = open_shell(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("panel-title").label(), Some("Screen"));
        let sidebar_x = window.find("push-files").bounds().origin.x;
        let panel_x = window.find("panel-title").bounds().origin.x;
        assert!(sidebar_x < panel_x, "LTR sidebar must lead");

        // Mode tabs re-render the panel heading.
        switch_mode(&view, 1, window, cx);
        assert_eq!(window.find("panel-title").label(), Some("Camera"));

        window.click("app-settings", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-notifications").label(),
            Some("Native notifications")
        );
        assert_eq!(
            window.find("settings-remember-position").label(),
            Some("Remember window position")
        );
        window.click("settings-notifications", cx);
        window.render_frame(cx);
        window.close_sheet(cx);
        window.render_frame(cx);

        // File transfer is available from the sidebar (disabled without a device).
        assert_eq!(
            window.find("push-files").label(),
            Some("Flash / Push Files")
        );

        assert_eq!(
            window.find("push-hint").label(),
            Some("Drag & drop ANY FILE OR APK")
        );

        assert_eq!(
            window.find("shortcuts-title").label(),
            Some("Shortcuts (Alt +)")
        );

        // The console exposes the parity actions.
        assert_eq!(window.find("console-clear").label(), Some("Clear"));
        assert_eq!(window.find("console-copy").label(), Some("Copy"));
        assert_eq!(window.find("console-report").label(), Some("Report"));
        assert_eq!(window.find("terminal-run").label(), Some("Run"));

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
        switch_mode(&view, 2, window, cx);
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
        assert_eq!(window.find_by_label("Width").role(), Some(Role::Slider));
        assert_eq!(window.find_by_label("Height").role(), Some(Role::Slider));
        window.click("desktop-flex", cx);
        window.render_frame(cx);
        let summary = window
            .find("desktop-summary")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(summary.contains("Flex"), "unexpected summary: {summary}");

        // Mic mode starts the windowless bridge and shares the session bar.
        switch_mode(&view, 3, window, cx);
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
        switch_mode(&view, 0, window, cx);
        let summary = window
            .find("mirror-summary")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(summary.contains("8 Mbps"), "unexpected summary: {summary}");
        assert!(summary.contains("H.264"), "unexpected summary: {summary}");
        assert!(window.find("mirror-record").label().is_some());
        assert_eq!(
            window.find("mirror-require-audio").label(),
            Some("Require audio")
        );
        assert_eq!(
            window.find("mirror-audio-dup").label(),
            Some("Keep playing on device")
        );
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
        window.click("app-settings", cx);
        window.render_frame(cx);
        window.click("settings-theme-dark", cx);
        window.render_frame(cx);
        assert_ne!(cx.theme().background, light_background);

        // Locale switching retranslates the whole window without a restart.
        // Keep it last: the switch is process-visible to later UI work.
        window.click("app-settings", cx);
        window.render_frame(cx);
        window.click("settings-language", cx);
        window.render_frame(cx);
        window.click("locale-fr", cx);
        window.render_frame(cx);
        assert_eq!(window.find("panel-title").label(), Some("Écran"));

        // Arabic flips the layout direction (and retranslates again).
        window.click("app-settings", cx);
        window.render_frame(cx);
        window.click("settings-language", cx);
        window.render_frame(cx);
        window.click("locale-ar", cx);
        window.render_frame(cx);
        assert!(LayoutDirection::global(cx).is_rtl());
        assert_ne!(window.find("panel-title").label(), Some("Écran"));
        let sidebar_x = window.find("push-files").bounds().origin.x;
        let panel_x = window.find("panel-title").bounds().origin.x;
        assert!(sidebar_x > panel_x, "RTL sidebar must follow the panel");

        // About lives inside the settings sheet now.
        window.click("app-settings", cx);
        window.render_frame(cx);
        window.click("settings-about", cx);
        window.render_frame(cx);
        assert!(window.find("about-repository").label().is_some());
    })
    .expect("window must stay open");
}
