//! Session management
//!
//! Handles creation, attachment, detachment, and lifecycle of sessions.

/// Represents a pmux session
#[derive(Debug, Clone)]
pub struct Session {
    /// Unique session identifier
    pub id: String,

    /// Session name (user-defined or auto-generated)
    pub name: String,

    /// Windows in this session
    pub windows: Vec<Window>,
}

/// Represents a window (containing one or more panes)
#[derive(Debug, Clone)]
pub struct Window {
    /// Window ID
    pub id: usize,

    /// Window name
    pub name: String,

    /// Panes in this window
    pub panes: Vec<Pane>,
}

/// Represents a pane (a single PTY)
#[derive(Debug, Clone)]
pub struct Pane {
    /// Pane ID
    pub id: usize,

    /// Current working directory
    pub cwd: String,

    /// Running command
    pub command: String,
}

impl Session {
    /// Create a new session
    pub fn new(name: String) -> Self {
        Session {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            windows: Vec::new(),
        }
    }

    /// Attach to this session
    pub fn attach(&mut self) {
        // TODO: implement attach logic
    }

    /// Detach from this session
    pub fn detach(&mut self) {
        // TODO: implement detach logic
    }
}
