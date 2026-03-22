//! Cross-platform terminal raw mode support for interactive attach.

use std::io;

// ── Unix ─────────────────────────────────────────────────────────────

#[cfg(unix)]
mod imp {
    use std::io;
    use std::os::unix::io::AsRawFd;

    /// Saved terminal state for restoration.
    pub struct SavedState {
        orig: libc::termios,
    }

    /// Enable raw mode on stdin. Returns saved state for restoration.
    pub fn enable_raw_mode() -> io::Result<SavedState> {
        let fd = std::io::stdin().as_raw_fd();
        let mut orig: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut orig) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = orig;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(SavedState { orig })
    }

    /// Restore terminal to saved state.
    pub fn disable_raw_mode(saved: &SavedState) -> io::Result<()> {
        let fd = std::io::stdin().as_raw_fd();
        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &saved.orig) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

// ── Windows ──────────────────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use std::io;
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode,
        ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;

    /// Saved terminal state for restoration.
    pub struct SavedState {
        input_mode: u32,
        output_mode: u32,
    }

    /// Enable raw mode on stdin/stdout. Returns saved state for restoration.
    pub fn enable_raw_mode() -> io::Result<SavedState> {
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            if input == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let mut input_mode: u32 = 0;
            if GetConsoleMode(input, &mut input_mode) == 0 {
                return Err(io::Error::last_os_error());
            }

            let raw_input = (input_mode
                & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
                | ENABLE_VIRTUAL_TERMINAL_INPUT;
            if SetConsoleMode(input, raw_input) == 0 {
                return Err(io::Error::last_os_error());
            }

            let output = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut output_mode: u32 = 0;
            if GetConsoleMode(output, &mut output_mode) == 0 {
                // Restore input on failure
                SetConsoleMode(input, input_mode);
                return Err(io::Error::last_os_error());
            }
            let vt_output = output_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
            if SetConsoleMode(output, vt_output) == 0 {
                SetConsoleMode(input, input_mode);
                return Err(io::Error::last_os_error());
            }

            Ok(SavedState {
                input_mode,
                output_mode,
            })
        }
    }

    /// Restore terminal to saved state.
    pub fn disable_raw_mode(saved: &SavedState) -> io::Result<()> {
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            SetConsoleMode(input, saved.input_mode);
            let output = GetStdHandle(STD_OUTPUT_HANDLE);
            SetConsoleMode(output, saved.output_mode);
        }
        Ok(())
    }
}

pub use imp::SavedState;

/// Enter raw mode. Returns a guard that restores on `restore()`.
pub fn enable_raw_mode() -> io::Result<SavedState> {
    imp::enable_raw_mode()
}

/// Restore terminal from raw mode.
pub fn disable_raw_mode(saved: &SavedState) -> io::Result<()> {
    imp::disable_raw_mode(saved)
}
