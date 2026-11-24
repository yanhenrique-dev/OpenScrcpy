//! UI-agnostic application events.
//!
//! Commands publish domain events here; each shell owns the adapter that turns
//! them into its own event mechanism. The Tauri shell forwards them with the
//! existing `scrcpy-*`/`mic-*` names, so the webview contract does not change
//! while the commands move out of the Tauri crate.

use serde_json::Value;
use tokio::sync::mpsc;

/// One event destined for the UI. Payloads that the frontend already receives
/// as JSON stay as [`Value`] so the wire format is unchanged.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    Log(String),
    Status(Value),
    WindowPos(Value),
    AudioStatus(Value),
    MicRoute { display: String, routed: bool },
    MicLevel(f64),
}

/// Cloneable handle used by commands. `send` is synchronous and unbounded:
/// the adapter consumes the receiver on the shell's runtime, preserving the
/// order and volume the old `window.emit` calls had.
#[derive(Clone)]
pub struct EventBus {
    sender: mpsc::UnboundedSender<AppEvent>,
}

pub type EventReceiver = mpsc::UnboundedReceiver<AppEvent>;

impl EventBus {
    pub fn channel() -> (Self, EventReceiver) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }

    pub fn send(&self, event: AppEvent) {
        // A closed receiver means the shell is shutting down; dropping the
        // event is the same outcome as emitting to a destroyed window.
        let _ = self.sender.send(event);
    }

    pub fn log(&self, line: impl Into<String>) {
        self.send(AppEvent::Log(line.into()));
    }

    pub fn status(&self, payload: Value) {
        self.send(AppEvent::Status(payload));
    }

    pub fn window_pos(&self, payload: Value) {
        self.send(AppEvent::WindowPos(payload));
    }

    pub fn audio_status(&self, payload: Value) {
        self.send(AppEvent::AudioStatus(payload));
    }

    pub fn mic_route(&self, display: impl Into<String>, routed: bool) {
        self.send(AppEvent::MicRoute {
            display: display.into(),
            routed,
        });
    }

    pub fn mic_level(&self, db: f64) {
        self.send(AppEvent::MicLevel(db));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn preserves_order_and_payloads() {
        let (bus, mut receiver) = EventBus::channel();
        bus.log("first");
        bus.status(json!({"device": "serial", "running": true}));
        bus.window_pos(json!({"device": "serial", "x": 10, "y": 20}));
        bus.audio_status(json!({"device": "serial", "status": "ok"}));
        bus.mic_route("OpenScrcpy Mic", true);
        bus.mic_level(-12.5);

        assert_eq!(receiver.recv().await, Some(AppEvent::Log("first".into())));
        assert_eq!(
            receiver.recv().await,
            Some(AppEvent::Status(
                json!({"device": "serial", "running": true})
            ))
        );
        assert_eq!(
            receiver.recv().await,
            Some(AppEvent::WindowPos(
                json!({"device": "serial", "x": 10, "y": 20})
            ))
        );
        assert_eq!(
            receiver.recv().await,
            Some(AppEvent::AudioStatus(
                json!({"device": "serial", "status": "ok"})
            ))
        );
        assert_eq!(
            receiver.recv().await,
            Some(AppEvent::MicRoute {
                display: "OpenScrcpy Mic".into(),
                routed: true,
            })
        );
        assert_eq!(receiver.recv().await, Some(AppEvent::MicLevel(-12.5)));
    }

    #[test]
    fn sends_after_receiver_is_dropped_do_not_panic() {
        let (bus, receiver) = EventBus::channel();
        drop(receiver);
        bus.log("dropped");
    }
}
