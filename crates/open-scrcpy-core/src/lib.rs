//! UI-agnostic domain for OpenScrcpy.
//!
//! This crate owns the scrcpy/adb helpers, argument validation, download and
//! extraction policy, and audio-failure classification. It must not depend on
//! Tauri or GPUI. Shells (the current Tauri app and the future GPUI app) call
//! these functions and own the windows, dialogs, notifications and events.

pub mod adb;
pub mod audio;
pub mod capabilities;
pub mod config;
pub mod download;
pub mod events;
pub mod mic;
pub mod monitor;
pub mod paths;
pub mod process;
pub mod report;
pub mod runtime;
pub mod scrcpy;
pub mod session;
pub mod state;
pub mod terminal;
pub mod update;
pub mod validate;
pub mod wireless;
