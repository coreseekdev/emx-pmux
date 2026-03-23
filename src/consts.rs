//! Named constants for values used across the codebase.
//!
//! Centralises magic numbers and strings so they can be reviewed
//! and adjusted in one place.

// ── Terminal defaults ────────────────────────────────────────────────

/// Default terminal width (columns).
pub const DEFAULT_COLS: u16 = 80;

/// Default terminal height (rows).
pub const DEFAULT_ROWS: u16 = 24;

/// Tab stop interval (columns).
pub const TAB_WIDTH: u16 = 8;

// ── I/O buffer sizes ────────────────────────────────────────────────

/// PTY reader buffer size (matches typical OS pipe buffer on Windows).
pub const PTY_READ_BUF_SIZE: usize = 65536;

/// Stdin buffer size for the attach loop.
pub const STDIN_BUF_SIZE: usize = 1024;

/// Pre-allocated capacity for the screen transcript buffer.
pub const TRANSCRIPT_BUF_CAPACITY: usize = 4096;

// ── IPC limits ───────────────────────────────────────────────────────

/// Maximum IPC message payload size (16 MiB).
pub const MAX_IPC_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

// ── Timing ───────────────────────────────────────────────────────────

/// Attach mode screen refresh interval (milliseconds).
/// ~30 Hz — balances responsiveness with CPU usage.
pub const ATTACH_REFRESH_MS: u64 = 33;

/// Default timeout for the `expect` command (milliseconds).
pub const DEFAULT_EXPECT_TIMEOUT_MS: u64 = 10_000;

/// Polling interval for the `expect` command (milliseconds).
pub const EXPECT_POLL_MS: u64 = 50;

/// Interval between dead-session reap cycles (seconds).
pub const REAP_INTERVAL_SECS: u64 = 2;

/// Maximum time to wait for the daemon to start (seconds).
pub const DAEMON_STARTUP_TIMEOUT_SECS: u64 = 5;

/// Delay between pipe-busy retries on Windows (milliseconds).
pub const PIPE_RETRY_DELAY_MS: u64 = 50;

/// Maximum number of pipe-busy retries on Windows.
pub const PIPE_RETRY_MAX_ATTEMPTS: u32 = 20;

// ── Environment variable names ───────────────────────────────────────

/// Environment variable to enable debug logging.
pub const ENV_LOG: &str = "EMX_PMUX_LOG";

/// Environment variable to set session log output directory.
pub const ENV_SESSION_LOG_PATH: &str = "EMX_PMUX_LOG_SESSION_PATH";

// ── Windows process creation ─────────────────────────────────────────

/// `CREATE_NO_WINDOW` flag for Windows `CreateProcess`.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
