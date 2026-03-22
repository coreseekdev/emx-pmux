//! Command-line interface parsing
//!
//! Handles parsing of tmux-compatible commands.

/// Parsed CLI arguments
#[derive(Debug, Clone)]
pub struct Args {
    /// Command to execute (e.g., "new-session", "attach", "ls")
    pub command: String,

    /// Arguments for the command
    pub args: Vec<String>,
}

impl Args {
    /// Parse command-line arguments
    pub fn parse() -> Self {
        // TODO: implement argument parsing
        Args {
            command: String::new(),
            args: Vec::new(),
        }
    }
}
