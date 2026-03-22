//! pmux - A terminal multiplexer for Windows
//!
//! Inspired by tmux and screen, pmux provides session management,
//! window/pane splitting, and persistent terminal sessions.

pub mod cli;
pub mod session;
pub mod pty;
pub mod server;
pub mod client;
pub mod ipc;
pub mod daemon;
pub mod platform;

/// Version information
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
