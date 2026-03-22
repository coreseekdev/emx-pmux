//! Windows ConPTY implementation.

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, INVALID_HANDLE_VALUE, S_OK, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, InitializeProcThreadAttributeList,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
    PROCESS_INFORMATION, STARTUPINFOEXW,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
};

use super::error::{PtyError, PtyResult};
use super::{PtyConfig, WindowSize};

const FALSE: i32 = 0;

/// Windows PTY master handle (synchronous, ConPTY-based).
pub struct PtyMaster {
    hpc: HPCON,
    input_write: OwnedHandle,
    output_read: OwnedHandle,
    open: bool,
    size: WindowSize,
}

impl std::fmt::Debug for PtyMaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyMaster")
            .field("open", &self.open)
            .field("size", &self.size)
            .finish()
    }
}

// SAFETY: ConPTY handles can be used across threads
unsafe impl Send for PtyMaster {}

impl PtyMaster {
    /// Resize the PTY window.
    pub fn resize(&mut self, size: WindowSize) -> PtyResult<()> {
        if !self.open {
            return Err(PtyError::Closed);
        }
        let coord = COORD {
            X: size.cols as i16,
            Y: size.rows as i16,
        };
        let result = unsafe { ResizePseudoConsole(self.hpc, coord) };
        if result != S_OK {
            return Err(PtyError::Resize(io::Error::from_raw_os_error(result)));
        }
        self.size = size;
        Ok(())
    }

    /// Check if the PTY is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Close the PTY master.
    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            unsafe { ClosePseudoConsole(self.hpc) };
        }
    }

    /// Clone only the read handle for use in a reader thread.
    /// The returned `PtyReader` does NOT own the HPCON, so dropping it
    /// will not close the pseudo console.
    pub fn try_clone(&self) -> PtyResult<PtyReader> {
        use windows_sys::Win32::Foundation::DuplicateHandle;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let dup = |src: &OwnedHandle| -> io::Result<OwnedHandle> {
            let mut new_handle: HANDLE = ptr::null_mut();
            let ok = unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    src.as_raw_handle() as HANDLE,
                    GetCurrentProcess(),
                    &mut new_handle,
                    0,
                    0, // not inheritable
                    2, // DUPLICATE_SAME_ACCESS
                )
            };
            if ok == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(unsafe { OwnedHandle::from_raw_handle(new_handle as RawHandle) })
            }
        };

        let output_read = dup(&self.output_read).map_err(PtyError::Io)?;

        Ok(PtyReader { output_read })
    }

    /// Set non-blocking mode (no-op on Windows, pipes are always blocking).
    pub fn set_nonblocking(&self, _nonblock: bool) -> PtyResult<()> {
        // Windows named pipes don't have a simple non-blocking toggle like Unix.
        // We handle this at the read level with PeekNamedPipe instead.
        Ok(())
    }
}

impl Read for PtyMaster {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.open {
            return Ok(0); // EOF
        }
        let mut bytes_read: u32 = 0;
        let success = unsafe {
            ReadFile(
                self.output_read.as_raw_handle() as HANDLE,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut bytes_read,
                ptr::null_mut(),
            )
        };
        if success == FALSE {
            let err = io::Error::last_os_error();
            // ERROR_BROKEN_PIPE (109) means child closed
            if err.raw_os_error() == Some(109) {
                return Ok(0); // EOF
            }
            return Err(err);
        }
        Ok(bytes_read as usize)
    }
}

impl Write for PtyMaster {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.open {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "PTY closed"));
        }
        let mut bytes_written: u32 = 0;
        let success = unsafe {
            WriteFile(
                self.input_write.as_raw_handle() as HANDLE,
                buf.as_ptr(),
                buf.len() as u32,
                &mut bytes_written,
                ptr::null_mut(),
            )
        };
        if success == FALSE {
            return Err(io::Error::last_os_error());
        }
        Ok(bytes_written as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for PtyMaster {
    fn drop(&mut self) {
        self.close();
    }
}

/// Windows PTY child process handle.
pub struct PtyChild {
    process: OwnedHandle,
    pid: u32,
    job: Option<OwnedHandle>,
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
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn is_running(&mut self) -> bool {
        if self.exited {
            return false;
        }
        match self.try_wait() {
            Ok(Some(_)) => false,
            _ => true,
        }
    }

    pub fn try_wait(&mut self) -> PtyResult<Option<i32>> {
        if self.exited {
            return Ok(self.exit_code);
        }
        let result = unsafe { WaitForSingleObject(self.process.as_raw_handle() as HANDLE, 0) };
        if result == WAIT_OBJECT_0 {
            let mut code: u32 = 0;
            unsafe { GetExitCodeProcess(self.process.as_raw_handle() as HANDLE, &mut code) };
            self.exited = true;
            self.exit_code = Some(code as i32);
            Ok(Some(code as i32))
        } else {
            Ok(None)
        }
    }

    pub fn wait(&mut self) -> PtyResult<i32> {
        if self.exited {
            return Ok(self.exit_code.unwrap_or(-1));
        }
        let result = unsafe { WaitForSingleObject(self.process.as_raw_handle() as HANDLE, INFINITE) };
        if result != WAIT_OBJECT_0 {
            return Err(PtyError::Io(io::Error::last_os_error()));
        }
        let mut code: u32 = 0;
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle() as HANDLE, &mut code) } == FALSE {
            return Err(PtyError::Io(io::Error::last_os_error()));
        }
        self.exited = true;
        self.exit_code = Some(code as i32);
        Ok(code as i32)
    }

    pub fn kill(&self) -> PtyResult<()> {
        if let Some(ref job) = self.job {
            if unsafe { TerminateJobObject(job.as_raw_handle() as HANDLE, 1) } == FALSE {
                return Err(PtyError::Io(io::Error::last_os_error()));
            }
        } else {
            if unsafe { TerminateProcess(self.process.as_raw_handle() as HANDLE, 1) } == FALSE {
                return Err(PtyError::Io(io::Error::last_os_error()));
            }
        }
        Ok(())
    }
}

/// Read-only handle for the PTY output pipe.
/// Does NOT own the HPCON — dropping this will not close the pseudo console.
pub struct PtyReader {
    output_read: OwnedHandle,
}

unsafe impl Send for PtyReader {}

impl Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut bytes_read: u32 = 0;
        let success = unsafe {
            ReadFile(
                self.output_read.as_raw_handle() as HANDLE,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut bytes_read,
                ptr::null_mut(),
            )
        };
        if success == FALSE {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(109) {
                return Ok(0); // EOF — broken pipe
            }
            return Err(err);
        }
        Ok(bytes_read as usize)
    }
}

// ---- Helper functions ----

fn create_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read_h: HANDLE = INVALID_HANDLE_VALUE;
    let mut write_h: HANDLE = INVALID_HANDLE_VALUE;
    // 64 KB buffer — matches psmux; default (0) is ~4 KB which can
    // stall ConPTY output on fast-producing child processes.
    if unsafe { CreatePipe(&mut read_h, &mut write_h, ptr::null(), 64 * 1024) } == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read_h as RawHandle),
            OwnedHandle::from_raw_handle(write_h as RawHandle),
        )
    })
}

fn create_job_object() -> io::Result<Option<OwnedHandle>> {
    let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if job.is_null() {
        return Ok(None);
    }

    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

    let result = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };

    if result == FALSE {
        unsafe { CloseHandle(job) };
        return Ok(None);
    }

    Ok(Some(unsafe { OwnedHandle::from_raw_handle(job as RawHandle) }))
}

/// Check if ConPTY is available on this Windows version.
pub fn is_conpty_available() -> bool {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    let kernel32 = unsafe { GetModuleHandleW(windows_sys::w!("kernel32.dll")) };
    if kernel32.is_null() {
        return false;
    }
    let proc = unsafe { GetProcAddress(kernel32, windows_sys::s!("CreatePseudoConsole")) };
    proc.is_some()
}

/// Escape a Windows command-line argument.
fn escape_argument(arg: &str) -> Vec<u16> {
    let needs_quoting = arg.is_empty()
        || arg.contains(' ')
        || arg.contains('\t')
        || arg.contains('"')
        || arg.contains('\\');

    if !needs_quoting {
        return arg.encode_utf16().collect();
    }

    let mut result: Vec<u16> = Vec::new();
    result.push(b'"' as u16);

    let chars: Vec<char> = arg.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let mut num_bs = 0;
            while i < chars.len() && chars[i] == '\\' {
                num_bs += 1;
                i += 1;
            }
            if i < chars.len() && chars[i] == '"' {
                for _ in 0..(num_bs * 2) {
                    result.push(b'\\' as u16);
                }
                result.push(b'\\' as u16);
                result.push(b'"' as u16);
                i += 1;
            } else if i >= chars.len() {
                for _ in 0..(num_bs * 2) {
                    result.push(b'\\' as u16);
                }
            } else {
                for _ in 0..num_bs {
                    result.push(b'\\' as u16);
                }
            }
        } else if c == '"' {
            result.push(b'\\' as u16);
            result.push(b'"' as u16);
            i += 1;
        } else {
            for code_unit in c.encode_utf16(&mut [0u16; 2]) {
                result.push(*code_unit);
            }
            i += 1;
        }
    }

    result.push(b'"' as u16);
    result
}

/// Spawn a child process in a new ConPTY.
pub fn spawn(config: &PtyConfig) -> PtyResult<(PtyMaster, PtyChild)> {
    if !is_conpty_available() {
        #[cfg(windows)]
        return Err(PtyError::ConPtyNotAvailable);
    }

    // Create pipes: (read, write)
    let (pty_in_read, pty_in_write) = create_pipe().map_err(PtyError::Create)?;
    let (pty_out_read, pty_out_write) = create_pipe().map_err(PtyError::Create)?;

    // Create pseudo console
    let coord = COORD {
        X: config.size.cols as i16,
        Y: config.size.rows as i16,
    };
    let mut hpc: HPCON = 0;
    let result = unsafe {
        CreatePseudoConsole(
            coord,
            pty_in_read.as_raw_handle() as HANDLE,
            pty_out_write.as_raw_handle() as HANDLE,
            0,
            &mut hpc,
        )
    };
    if result != S_OK {
        return Err(PtyError::Create(io::Error::from_raw_os_error(result)));
    }

    // Close the PTY-side pipe handles (duped into ConHost)
    drop(pty_in_read);
    drop(pty_out_write);

    // Build command line
    let mut cmdline_wide: Vec<u16> = escape_argument(&config.program);
    for arg in &config.args {
        cmdline_wide.push(b' ' as u16);
        cmdline_wide.extend(escape_argument(arg));
    }
    cmdline_wide.push(0);

    // Build environment block
    let env_block = build_env_block(config);

    // Working directory
    let working_dir: Option<Vec<u16>> = config.working_directory.as_ref().map(|d| {
        let mut w: Vec<u16> = OsStr::new(d).encode_wide().collect();
        w.push(0);
        w
    });

    // Create job object
    let job = create_job_object().map_err(PtyError::Spawn)?;

    // Setup startup info with pseudo console attribute
    let mut attr_size: usize = 0;
    unsafe {
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut attr_size);
    }
    let mut attr_list = vec![0u8; attr_size];
    if unsafe {
        InitializeProcThreadAttributeList(attr_list.as_mut_ptr() as *mut _, 1, 0, &mut attr_size)
    } == FALSE
    {
        return Err(PtyError::Spawn(io::Error::last_os_error()));
    }
    if unsafe {
        UpdateProcThreadAttribute(
            attr_list.as_mut_ptr() as *mut _,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            hpc as *mut _,
            mem::size_of::<HPCON>(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    } == FALSE
    {
        return Err(PtyError::Spawn(io::Error::last_os_error()));
    }

    let mut startup_info: STARTUPINFOEXW = unsafe { mem::zeroed() };
    startup_info.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
    startup_info.lpAttributeList = attr_list.as_mut_ptr() as *mut _;

    let mut proc_info: PROCESS_INFORMATION = unsafe { mem::zeroed() };

    let result = unsafe {
        CreateProcessW(
            ptr::null(),
            cmdline_wide.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            FALSE,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            if env_block.is_empty() {
                ptr::null()
            } else {
                env_block.as_ptr() as *const _
            },
            working_dir
                .as_ref()
                .map_or(ptr::null(), |w| w.as_ptr()),
            &startup_info.StartupInfo,
            &mut proc_info,
        )
    };

    if result == FALSE {
        unsafe { ClosePseudoConsole(hpc) };
        return Err(PtyError::Spawn(io::Error::last_os_error()));
    }

    // Close thread handle
    unsafe { CloseHandle(proc_info.hThread) };

    let process = unsafe { OwnedHandle::from_raw_handle(proc_info.hProcess as RawHandle) };

    // Assign to job
    if let Some(ref job_h) = job {
        unsafe {
            AssignProcessToJobObject(
                job_h.as_raw_handle() as HANDLE,
                process.as_raw_handle() as HANDLE,
            );
        }
    }

    let master = PtyMaster {
        hpc,
        input_write: pty_in_write,
        output_read: pty_out_read,
        open: true,
        size: config.size,
    };

    let child = PtyChild {
        process,
        pid: proc_info.dwProcessId,
        job,
        exited: false,
        exit_code: None,
    };

    Ok((master, child))
}

fn build_env_block(config: &PtyConfig) -> Vec<u16> {
    if config.env.is_empty() {
        return Vec::new(); // NULL → inherit parent environment
    }

    // Start with the parent process environment, then overlay custom vars.
    let mut env_map: std::collections::HashMap<String, String> = std::env::vars().collect();
    for (key, value) in &config.env {
        env_map.insert(key.clone(), value.clone());
    }

    let mut block = Vec::new();
    for (key, value) in &env_map {
        let entry = format!("{}={}", key, value);
        block.extend(entry.encode_utf16());
        block.push(0);
    }
    block.push(0); // Double null terminator
    block
}
