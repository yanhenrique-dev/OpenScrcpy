//! Tauri adapter for `open_scrcpy_core::events`.
//!
//! Commands publish to the bus; this module is the only place that knows the
//! webview event names and payload shapes. Keep these names stable while the
//! frontend still consumes them.

use std::sync::OnceLock;

use open_scrcpy_core::events::{AppEvent, EventBus, EventReceiver};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

static BUS: OnceLock<EventBus> = OnceLock::new();

/// Start the bus and forward every event to the main window.
pub fn init(app: &AppHandle) {
    let (bus, receiver) = EventBus::channel();
    let _ = BUS.set(bus);
    spawn_forwarder(app.clone(), receiver);
}

/// Events emitted before `init` (unit tests, very early startup) are dropped,
/// never a panic: commands must not fail because the UI bus is absent.
/// The process-wide bus, when `init` has run.
pub fn bus() -> Option<&'static EventBus> {
    BUS.get()
}

fn with_bus(emit: impl FnOnce(&EventBus)) {
    if let Some(bus) = BUS.get() {
        emit(bus);
    }
}

pub fn log(line: impl Into<String>) {
    with_bus(|bus| bus.log(line));
}

/// Concrete-`String` adapter for core functions that accept `Fn(String)`.
pub fn log_line(line: String) {
    log(line);
}

pub fn status(payload: serde_json::Value) {
    with_bus(|bus| bus.status(payload));
}

pub fn audio_status(payload: serde_json::Value) {
    with_bus(|bus| bus.audio_status(payload));
}

pub fn mic_route(display: impl Into<String>, routed: bool) {
    with_bus(|bus| bus.mic_route(display, routed));
}

pub fn mic_level(db: f64) {
    with_bus(|bus| bus.mic_level(db));
}

fn spawn_forwarder(app: AppHandle, mut receiver: EventReceiver) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = receiver.recv().await {
            let Some(window) = app.get_webview_window("main") else {
                continue;
            };
            let (name, payload) = encode(event);
            let _ = window.emit(name, payload);
        }
    });
}

/// The webview contract: event name plus the JSON payload the frontend reads.
/// Kept pure so the payload shapes can be tested without a Tauri window.
fn encode(event: AppEvent) -> (&'static str, Value) {
    match event {
        AppEvent::Log(line) => ("scrcpy-log", Value::String(line)),
        AppEvent::Status(payload) => ("scrcpy-status", payload),
        AppEvent::WindowPos(payload) => ("scrcpy-window-pos", payload),
        AppEvent::AudioStatus(payload) => ("scrcpy-audio-status", payload),
        AppEvent::MicRoute { display, routed } => {
            ("mic-route", serde_json::json!([display, routed]))
        }
        AppEvent::MicLevel(db) => ("mic-level", serde_json::json!(db)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_the_existing_webview_event_contract() {
        assert_eq!(
            encode(AppEvent::Log("[SYSTEM] ready".into())),
            ("scrcpy-log", json!("[SYSTEM] ready"))
        );
        assert_eq!(
            encode(AppEvent::Status(json!({"device": "d", "running": true}))),
            ("scrcpy-status", json!({"device": "d", "running": true}))
        );
        assert_eq!(
            encode(AppEvent::WindowPos(json!({"device": "d", "x": 1, "y": 2}))),
            ("scrcpy-window-pos", json!({"device": "d", "x": 1, "y": 2}))
        );
        assert_eq!(
            encode(AppEvent::AudioStatus(
                json!({"device": "d", "status": "ok"})
            )),
            (
                "scrcpy-audio-status",
                json!({"device": "d", "status": "ok"})
            )
        );
        assert_eq!(
            encode(AppEvent::MicRoute {
                display: "OpenScrcpy Mic".into(),
                routed: true,
            }),
            ("mic-route", json!(["OpenScrcpy Mic", true]))
        );
        assert_eq!(
            encode(AppEvent::MicLevel(-12.5)),
            ("mic-level", json!(-12.5))
        );
    }
}
