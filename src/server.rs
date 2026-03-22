//! Server component
//!
//! Handles the server-side logic for session management and client connections.

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Session name
    pub session_name: String,

    /// Socket name for namespace isolation (-L flag)
    pub socket_name: Option<String>,

    /// Port for TCP communication
    pub port: Option<u16>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            session_name: "default".to_string(),
            socket_name: None,
            port: None,
        }
    }
}

/// Server instance
pub struct Server {
    config: ServerConfig,
}

impl Server {
    /// Create a new server with the given configuration
    pub fn new(config: ServerConfig) -> Self {
        Server { config }
    }

    /// Start the server event loop
    pub fn run(&mut self) -> Result<(), String> {
        // TODO: implement server main loop
        // 1. Bind TCP listener
        // 2. Write .port and .key files
        // 3. Start accept thread
        // 4. Load configuration
        // 5. Create initial window
        // 6. Enter event loop
        Ok(())
    }
}
