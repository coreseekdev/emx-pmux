//! PTY error types.

use std::fmt;
use std::io;

/// PTY error type.
#[derive(Debug)]
pub enum PtyError {
    /// Failed to create PTY pair.
    Create(io::Error),
    /// Failed to spawn child process.
    Spawn(io::Error),
    /// I/O error during PTY operations.
    Io(io::Error),
    /// Failed to resize PTY.
    Resize(io::Error),
    /// PTY has been closed.
    Closed,
    /// Child process has exited.
    ProcessExited(i32),
    /// ConPTY not available (Windows).
    #[cfg(windows)]
    ConPtyNotAvailable,
}

impl fmt::Display for PtyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Create(e) => write!(f, "failed to create PTY: {}", e),
            Self::Spawn(e) => write!(f, "failed to spawn process: {}", e),
            Self::Io(e) => write!(f, "PTY I/O error: {}", e),
            Self::Resize(e) => write!(f, "failed to resize PTY: {}", e),
            Self::Closed => write!(f, "PTY has been closed"),
            Self::ProcessExited(code) => write!(f, "process exited with code {}", code),
            #[cfg(windows)]
            Self::ConPtyNotAvailable => write!(f, "ConPTY not available on this Windows version"),
        }
    }
}

impl std::error::Error for PtyError {}

impl From<io::Error> for PtyError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<PtyError> for io::Error {
    fn from(e: PtyError) -> Self {
        match e {
            PtyError::Io(e) => e,
            PtyError::Create(e) | PtyError::Spawn(e) | PtyError::Resize(e) => e,
            PtyError::Closed => io::Error::new(io::ErrorKind::BrokenPipe, "PTY closed"),
            PtyError::ProcessExited(code) => {
                io::Error::new(io::ErrorKind::Other, format!("process exited: {}", code))
            }
            #[cfg(windows)]
            PtyError::ConPtyNotAvailable => {
                io::Error::new(io::ErrorKind::Unsupported, "ConPTY not available")
            }
        }
    }
}

pub type PtyResult<T> = Result<T, PtyError>;
