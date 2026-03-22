//! CLI definitions (clap derive).

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "pmux", version, about = "Cross-platform terminal multiplexer")]
pub struct Args {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Create a new session (auto-starts daemon if needed).
    #[command(alias = "new-session")]
    New {
        /// Session name (auto-generated if omitted).
        #[arg(short = 't', long)]
        target: Option<String>,
        /// Command to run (default: platform shell).
        #[arg(short = 'c', long)]
        command: Option<String>,
        /// Terminal width.
        #[arg(short = 'x', long, default_value_t = 80)]
        width: u16,
        /// Terminal height.
        #[arg(short = 'y', long, default_value_t = 24)]
        height: u16,
    },

    /// Kill a session.
    #[command(alias = "kill-session")]
    Kill {
        /// Session name.
        #[arg(short = 't', long)]
        target: String,
    },

    /// List sessions.
    #[command(alias = "list-sessions")]
    Ls,

    /// Send data to a session's PTY.
    Send {
        /// Session name.
        #[arg(short = 't', long)]
        target: String,
        /// Text to send (joined with spaces).
        #[arg(required = true, trailing_var_arg = true)]
        text: Vec<String>,
    },

    /// View a session's screen buffer.
    View {
        /// Session name.
        #[arg(short = 't', long)]
        target: String,
    },

    /// Resize a session's PTY.
    Resize {
        /// Session name.
        #[arg(short = 't', long)]
        target: String,
        #[arg(short = 'x', long)]
        width: u16,
        #[arg(short = 'y', long)]
        height: u16,
    },

    /// Show daemon status.
    Status,

    /// Force-kill the daemon process.
    Stop,

    /// Ping the daemon (health check).
    Ping,

    /// (internal) Run as daemon – not for direct use.
    #[command(hide = true)]
    Daemon,
}
