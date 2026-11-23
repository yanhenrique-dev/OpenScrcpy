//! The application shell: header, device hub, mode tabs and the console.
//!
//! This is the Phase 2 foundation. It owns presentation state only; session and
//! device state will arrive from `open-scrcpy-core` through a service in Phase
//! 3. Visible copy comes from the shared locale catalogs.

use gpui_kit::base::Selectable as _;
use gpui_kit::component::{ActiveTheme as _, button::Button};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, Window, WindowBounds, div,
};
use rust_i18n::t;

use crate::i18n;
use crate::settings::{AppSettings, SettingsStore, ThemePreference};
use crate::theme;

fn theme_label(preference: ThemePreference) -> SharedString {
    match preference {
        ThemePreference::Light => t!("header.colorModes.light"),
        ThemePreference::Dark => t!("header.colorModes.dark"),
        ThemePreference::System => t!("header.colorModes.system"),
    }
    .to_string()
    .into()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    Screen,
    Camera,
    Desktop,
    Microphone,
}

impl CaptureMode {
    const ALL: [Self; 4] = [Self::Screen, Self::Camera, Self::Desktop, Self::Microphone];

    fn id(self) -> &'static str {
        match self {
            Self::Screen => "mode-screen",
            Self::Camera => "mode-camera",
            Self::Desktop => "mode-desktop",
            Self::Microphone => "mode-microphone",
        }
    }

    fn label(self) -> SharedString {
        match self {
            Self::Screen => t!("controlPanel.screen"),
            Self::Camera => t!("controlPanel.camera"),
            Self::Desktop => t!("controlPanel.desktop"),
            Self::Microphone => t!("controlPanel.microphone"),
        }
        .to_string()
        .into()
    }
}

pub struct AppShell {
    settings: AppSettings,
    store: SettingsStore,
    mode: CaptureMode,
}

impl AppShell {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        settings: AppSettings,
        store: SettingsStore,
    ) -> Self {
        theme::apply(settings.theme, Some(window), cx);

        // Persist the restore bounds on close. The command-less foundation has
        // no other place to observe the final geometry.
        let close_store = store.clone();
        window.on_window_should_close(cx, move |window, _cx| {
            let maximized = matches!(window.window_bounds(), WindowBounds::Maximized(_));
            close_store.save_window(window.bounds(), maximized);
            true
        });

        Self {
            settings,
            store,
            mode: CaptureMode::Screen,
        }
    }

    fn persist(&self) {
        if let Err(error) = self.store.save(&self.settings) {
            eprintln!("[settings] could not save: {error}");
        }
    }

    fn set_theme(&mut self, theme: ThemePreference, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.theme == theme {
            return;
        }
        self.settings.theme = theme;
        theme::apply(theme, Some(window), cx);
        self.persist();
        cx.notify();
    }

    fn cycle_locale(&mut self, cx: &mut Context<Self>) {
        let next = i18n::next(&self.settings.locale);
        self.settings.locale = next.to_string();
        i18n::apply(next);
        self.persist();
        cx.refresh_windows();
    }

    fn switch_mode(&mut self, mode: CaptureMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    fn locale_label(&self) -> SharedString {
        format!(
            "{}: {}",
            t!("header.languageLabel"),
            i18n::label(&self.settings.locale)
        )
        .into()
    }

    fn theme_modes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().items_center().gap_1().children(
            [
                ThemePreference::Light,
                ThemePreference::Dark,
                ThemePreference::System,
            ]
            .map(|preference| {
                Button::new(preference.id())
                    .label(theme_label(preference))
                    .selected(self.settings.theme == preference)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_theme(preference, window, cx)
                    }))
            }),
        )
    }

    fn header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale_label = self.locale_label();
        let version = env!("CARGO_PKG_VERSION");

        div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().id("app-title").text_lg().child("OpenScrcpy"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("v{version}")),
                    ),
            )
            .child(div().flex_1())
            .child(self.theme_modes(cx))
            .child(
                Button::new("locale-toggle")
                    .label(locale_label)
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_locale(cx))),
            )
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_64()
            .flex_shrink_0()
            .p_3()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("sidebar.deviceHub").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("sidebar.noDevicesDetected").to_string()),
            )
    }

    fn mode_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_1()
            .p_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .children(CaptureMode::ALL.map(|mode| {
                Button::new(mode.id())
                    .label(mode.label())
                    .selected(self.mode == mode)
                    .on_click(cx.listener(move |this, _, _, cx| this.switch_mode(mode, cx)))
            }))
    }

    fn panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_6()
            .child(
                div()
                    .id("panel-title")
                    .role(Role::Heading)
                    .aria_label(self.mode.label().to_string())
                    .text_xl()
                    .child(self.mode.label())
                    .test_support(),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("sidebar.noDevicesDetected").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("sidebar.usbSetupTip").to_string()),
                    ),
            )
    }

    fn console(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .h_40()
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .px_4()
                    .py_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("logPanel.systemConsole").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .px_4()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("logPanel.waitingForSequence").to_string()),
            )
    }
}

impl Render for AppShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(self.mode_tabs(cx))
                            .child(self.panel(cx)),
                    ),
            )
            .child(self.console(cx))
    }
}
