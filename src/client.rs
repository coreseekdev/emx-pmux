//! Client component
//!
//! Handles the client-side logic for connecting to and interacting with the server.

/// Client configuration
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Target session name
    pub session_name: String,

    /// Socket name for namespace isolation (-L flag)
    pub socket_name: Option<String>,
}

/// Client instance
pub struct Client {
    config: ClientConfig,
}

impl Client {
    /// Create a new client with the given configuration
    pub fn new(config: ClientConfig) -> Self {
        Client { config }
    }

    /// Attach to the target session
    pub fn attach(&mut self) -> Result<(), String> {
        // TODO: implement attach logic
        // 1. Read .port and .key files
        // 2. Connect to server via TCP
        // 3. Authenticate with session key
        // 4. Send PERSISTENT flag
        // 5. Send terminal size
        // 6. Enter render loop
        Ok(())
    }

    /// Send a command to the server
    pub fn send_command(&mut self, cmd: &str) -> Result<String, String> {
        // TODO: implement command sending
        Ok(String::new())
    }
}
