//! Unix PTY implementation using rustix.

use std::ffi::CString;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, OwnedFd, RawFd};

use rustix::fs::OFlags;
use rustix::pty::{openpt, grantpt, unlockpt, ptsname, OpenptFlags};

use super::error::{PtyError, PtyResult};
use super::{PtyConfig, WindowSize};

/// Unix PTY master handle (synchronous).
pub struct PtyMaster {
    fd: OwnedFd,
    open: bool,
}

impl std::fmt::Debug for PtyMaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyMaster")
            .field("fd", &self.fd.as_raw_fd())
            .field("open", &self.open)
            .finish()
    }
}

impl PtyMaster {
    /// Resize the PTY window.
    pub fn resize(&self, size: WindowSize) -> PtyResult<()> {
        if !self.open {
            return Err(PtyError::Closed);
        }

        let ws = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // SAFETY: ioctl TIOCSWINSZ on a valid PTY fd
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
        if ret == -1 {
            return Err(PtyError::Resize(io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Set non-blocking mode.
    pub fn set_nonblocking(&self, nonblock: bool) -> PtyResult<()> {
        let flags = if nonblock {
            OFlags::NONBLOCK
        } else {
            OFlags::empty()
        };
        rustix::fs::fcntl_setfl(&self.fd, flags)
            .map_err(|e| PtyError::Io(io::Error::from_raw_os_error(e.raw_os_error())))?;
        Ok(())
    }

    /// Check if the PTY is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Close the PTY master.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Clone only the read handle for use in a reader thread.
    /// The returned `PtyReader` is a separate type that only supports reading.
    pub fn try_clone(&self) -> PtyResult<PtyReader> {
        let new_fd = self.fd.try_clone()
            .map_err(|e| PtyError::Io(e))?;
        Ok(PtyReader {
            fd: new_fd,
            open: self.open,
        })
    }

    /// Get the raw file descriptor.
    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl Read for PtyMaster {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.open {
            return Ok(0); // EOF
        }
        rustix::io::read(&self.fd, buf)
            .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))
    }
}

impl Write for PtyMaster {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.open {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "PTY closed"));
        }
        rustix::io::write(&self.fd, buf)
            .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Read-only handle for the PTY output.
pub struct PtyReader {
    fd: OwnedFd,
    open: bool,
}

impl Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.open {
            return Ok(0);
        }
        rustix::io::read(&self.fd, buf)
            .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))
    }
}

/// Unix PTY child process handle.
pub struct PtyChild {
    pid: u32,
    exited: bool,
    exit_code: Option<i32>,
}

impl std::fmt::Debug for PtyChild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyChild")
            .field("pid", &self.pid)
            .field("exited", &self.exited)
            .finish()
    }
}

impl PtyChild {
    /// Get the process ID.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Check if the child is still running (non-blocking, mutates cached state).
    pub fn is_running(&mut self) -> bool {
        if self.exited {
            return false;
        }
        match self.try_wait() {
            Ok(Some(_)) => false,
            _ => true,
        }
    }

    /// Check if process is still alive (non-mutating).
    ///
    /// Uses `kill(pid, 0)` which checks process existence without sending
    /// a signal.  This mirrors the Windows `is_process_alive(&self)` API
    /// so that `Session::is_alive` compiles on both platforms.
    pub fn is_process_alive(&self) -> bool {
        if self.exited {
            return false;
        }
        // SAFETY: kill with signal 0 is a standard POSIX existence check.
        unsafe { libc::kill(self.pid as libc::pid_t, 0) == 0 }
    }

    /// Non-blocking wait. Returns exit code if exited.
    pub fn try_wait(&mut self) -> PtyResult<Option<i32>> {
        if self.exited {
            return Ok(self.exit_code);
        }

        let mut status: libc::c_int = 0;
        // SAFETY: valid pid, WNOHANG
        let ret = unsafe { libc::waitpid(self.pid as libc::pid_t, &mut status, libc::WNOHANG) };
        if ret < 0 {
            return Err(PtyError::Io(io::Error::last_os_error()));
        }
        if ret == 0 {
            return Ok(None); // Still running
        }

        self.exited = true;
        if libc::WIFEXITED(status) {
            let code = libc::WEXITSTATUS(status);
            self.exit_code = Some(code);
            Ok(Some(code))
        } else if libc::WIFSIGNALED(status) {
            let sig = libc::WTERMSIG(status);
            self.exit_code = Some(-sig);
            Ok(Some(-sig))
        } else {
            self.exit_code = Some(-1);
            Ok(Some(-1))
        }
    }

    /// Blocking wait for child to exit.
    pub fn wait(&mut self) -> PtyResult<i32> {
        if self.exited {
            return Ok(self.exit_code.unwrap_or(-1));
        }

        let mut status: libc::c_int = 0;
        // SAFETY: valid pid
        let ret = unsafe { libc::waitpid(self.pid as libc::pid_t, &mut status, 0) };
        if ret < 0 {
            return Err(PtyError::Io(io::Error::last_os_error()));
        }

        self.exited = true;
        if libc::WIFEXITED(status) {
            let code = libc::WEXITSTATUS(status);
            self.exit_code = Some(code);
            Ok(code)
        } else if libc::WIFSIGNALED(status) {
            let sig = libc::WTERMSIG(status);
            self.exit_code = Some(-sig);
            Ok(-sig)
        } else {
            self.exit_code = Some(-1);
            Ok(-1)
        }
    }

    /// Send a signal to the child.
    pub fn signal(&self, sig: libc::c_int) -> PtyResult<()> {
        // SAFETY: valid pid and signal
        let ret = unsafe { libc::kill(self.pid as libc::pid_t, sig) };
        if ret == -1 {
            Err(PtyError::Io(io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }

    /// Kill the child (SIGKILL).
    pub fn kill(&self) -> PtyResult<()> {
        self.signal(libc::SIGKILL)
    }
}

/// Spawn a child process in a new PTY.
pub fn spawn(config: &PtyConfig) -> PtyResult<(PtyMaster, PtyChild)> {
    // Open master PTY
    let master_fd = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)
        .map_err(|e| PtyError::Create(io::Error::from_raw_os_error(e.raw_os_error())))?;

    grantpt(&master_fd)
        .map_err(|e| PtyError::Create(io::Error::from_raw_os_error(e.raw_os_error())))?;

    unlockpt(&master_fd)
        .map_err(|e| PtyError::Create(io::Error::from_raw_os_error(e.raw_os_error())))?;

    let slave_name = ptsname(&master_fd, Vec::new())
        .map_err(|e| PtyError::Create(io::Error::from_raw_os_error(e.raw_os_error())))?;

    let slave_path_str = slave_name
        .to_str()
        .map_err(|_| PtyError::Create(io::Error::new(io::ErrorKind::InvalidData, "invalid slave path")))?
        .to_string();

    let slave_c = CString::new(slave_path_str.as_bytes())
        .map_err(|_| PtyError::Create(io::Error::new(io::ErrorKind::InvalidData, "invalid slave path")))?;

    // Fork
    // SAFETY: fork is the standard way to create child process in Unix
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(PtyError::Spawn(io::Error::last_os_error()));
    }

    if pid == 0 {
        // ---- Child process ----
        // Create new session
        unsafe { libc::setsid() };

        // Open slave PTY
        let slave_fd = unsafe { libc::open(slave_c.as_ptr(), libc::O_RDWR) };
        if slave_fd < 0 {
            unsafe { libc::_exit(127) };
        }

        // Set controlling terminal
        unsafe { libc::ioctl(slave_fd, libc::TIOCSCTTY as libc::c_ulong, 0) };

        // Set window size on slave
        let ws = libc::winsize {
            ws_row: config.size.rows,
            ws_col: config.size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe { libc::ioctl(slave_fd, libc::TIOCSWINSZ, &ws) };

        // Redirect stdio
        unsafe {
            libc::dup2(slave_fd, 0);
            libc::dup2(slave_fd, 1);
            libc::dup2(slave_fd, 2);
            if slave_fd > 2 {
                libc::close(slave_fd);
            }
        }

        // Close master fd in child
        drop(master_fd);

        // Set environment variables
        for (key, value) in &config.env {
            std::env::set_var(key, value);
        }

        // Change working directory
        if let Some(ref dir) = config.working_directory {
            let _ = std::env::set_current_dir(dir);
        }

        // Set TERM if not specified
        if config.env.iter().all(|(k, _)| k != "TERM") {
            std::env::set_var("TERM", "xterm-256color");
        }

        // Build argv
        let program_c = CString::new(config.program.as_str()).unwrap_or_else(|_| CString::new("sh").unwrap());
        let mut argv_c: Vec<CString> = Vec::new();
        argv_c.push(program_c.clone());
        for arg in &config.args {
            if let Ok(a) = CString::new(arg.as_str()) {
                argv_c.push(a);
            }
        }
        let argv_ptrs: Vec<*const libc::c_char> = argv_c.iter().map(|s| s.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();

        unsafe { libc::execvp(program_c.as_ptr(), argv_ptrs.as_ptr()) };

        // If exec fails
        unsafe { libc::_exit(127) };
    }

    // ---- Parent process ----
    // Master fd stays blocking — the reader thread is dedicated and can block.

    // Set initial window size on master
    let master = PtyMaster {
        fd: master_fd,
        open: true,
    };
    master.resize(config.size)?;

    let child = PtyChild {
        pid: pid as u32,
        exited: false,
        exit_code: None,
    };

    Ok((master, child))
}
