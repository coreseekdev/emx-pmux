//! pmux-lib - Core library for pmux terminal multiplexer
//!
//! This library contains the shared functionality used by both
//! the server and client components.

pub mod cli;
pub mod session;
pub mod pty;
pub mod server;
pub mod client;
pub mod ipc;

/// Version information
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
