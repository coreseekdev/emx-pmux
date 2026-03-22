//! Inter-Process Communication (IPC)
//!
//! Handles communication between server and client components.

/// IPC protocol messages
#[derive(Debug, Clone)]
pub enum IpcMessage {
    /// Authentication: AUTH {key}
    Auth { key: String },

    /// Persistent mode flag
    Persistent,

    /// Target specification
    Target { target: String },

    /// Command to execute
    Command { cmd: String },

    /// Response from server
    Response { data: String },
}

/// IPC transport layer
pub struct IpcTransport {
    // TODO: add transport-specific fields
    _private: (),
}

impl IpcTransport {
    /// Create a new IPC transport
    pub fn new() -> Self {
        IpcTransport { _private: () }
    }

    /// Connect to a server
    pub fn connect(&mut self, port: u16) -> Result<(), String> {
        // TODO: implement TCP connection
        Ok(())
    }

    /// Send a message
    pub fn send(&mut self, msg: &IpcMessage) -> Result<(), String> {
        // TODO: implement message sending
        Ok(())
    }

    /// Receive a message
    pub fn receive(&mut self) -> Result<IpcMessage, String> {
        // TODO: implement message receiving
        Err("not implemented".to_string())
    }
}
