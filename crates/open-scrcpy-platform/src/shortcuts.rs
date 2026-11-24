//! Global OS shortcuts.
//!
//! The registration must stay alive for the whole process; the caller keeps the
//! returned guard. Events arrive on a background thread and are forwarded
//! through a Tokio channel, so a UI shell can consume them with its own
//! executor.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tokio::sync::mpsc::UnboundedSender;

/// Commands a shell exposes to OS-level shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformCommand {
    /// Bring the active mirror window back to the centre of the screen.
    Recenter,
}

/// Keeps the OS registration alive. Dropping it unregisters the shortcuts.
pub struct Shortcuts {
    _manager: GlobalHotKeyManager,
}

/// Registers Ctrl+Alt+Shift+C (recentre the active mirror). Fails when the
/// platform refuses the registration; the caller treats that as non-fatal.
pub fn register(commands: UnboundedSender<PlatformCommand>) -> Result<Shortcuts, String> {
    let manager = GlobalHotKeyManager::new().map_err(|error| error.to_string())?;
    let hotkey = HotKey::new(
        Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT),
        Code::KeyC,
    );
    manager
        .register(hotkey)
        .map_err(|error| error.to_string())?;

    std::thread::spawn(move || {
        while let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
            if event.state() == HotKeyState::Pressed {
                let _ = commands.send(PlatformCommand::Recenter);
            }
        }
    });

    Ok(Shortcuts { _manager: manager })
}
