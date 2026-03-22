//! CLI definitions (clap derive) - GNU screen compatible.

use clap::Parser;

/// pmux - cross-platform terminal multiplexer (screen-compatible)
#[derive(Parser, Debug)]
#[command(name = "pmux", version, about = "Cross-platform terminal multiplexer")]
pub struct Args {
    /// List sessions (screen -ls equivalent)
    #[arg(short = 'l', long = "list", conflicts_with = "exec")]
    pub list: bool,

    /// View/peek session screen buffer (non-interactive, unlike -r)
    #[arg(short = 'v', long = "view", conflicts_with = "exec")]
    pub view: bool,

    /// Resume/attach to a session (screen -r equivalent)
    #[arg(short = 'r', long = "resume", conflicts_with = "exec")]
    pub resume: bool,

    /// Detach a session elsewhere
    #[arg(short = 'd', long = "detach")]
    pub detach: bool,

    /// Send command to a running session (screen -X equivalent)
    #[arg(short = 'X', long = "exec")]
    pub exec: bool,

    /// Session name (screen -S equivalent)
    #[arg(short = 'S', long = "session")]
    pub session: Option<String>,

    /// Command to run (screen -s/-c equivalent)
    #[arg(short = 'c', long = "command")]
    pub command: Option<String>,

    /// Terminal width
    #[arg(short = 'x', long, default_value_t = 80)]
    pub width: u16,

    /// Terminal height
    #[arg(short = 'y', long, default_value_t = 24)]
    pub height: u16,

    /// Show daemon status
    #[arg(long = "status")]
    pub status: bool,

    /// Stop the daemon
    #[arg(long = "stop")]
    pub stop: bool,

    /// Ping the daemon
    #[arg(long = "ping")]
    pub ping: bool,

    /// (internal) Run as daemon
    #[arg(short = 'D', long = "daemon", hide = true)]
    pub daemon: bool,

    /// Command arguments for -X mode
    #[arg(trailing_var_arg = true)]
    pub command_args: Vec<String>,
}

impl Args {
    /// Determine the operation mode based on flags
    pub fn mode(&self) -> Mode {
        if self.daemon {
            return Mode::Daemon;
        }
        if self.status {
            return Mode::Status;
        }
        if self.stop {
            return Mode::Stop;
        }
        if self.ping {
            return Mode::Ping;
        }
        if self.list {
            return Mode::List;
        }
        if self.view {
            return Mode::View;
        }
        if self.exec {
            return Mode::Exec;
        }
        if self.resume {
            return Mode::Resume;
        }
        Mode::Create
    }
}

/// Operation mode derived from command-line flags
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// Create a new session
    Create,
    /// List sessions
    List,
    /// View session screen buffer (non-interactive)
    View,
    /// Resume/attach to a session
    Resume,
    /// Execute command on running session
    Exec,
    /// Show daemon status
    Status,
    /// Stop daemon
    Stop,
    /// Ping daemon
    Ping,
    /// Run as daemon
    Daemon,
}
