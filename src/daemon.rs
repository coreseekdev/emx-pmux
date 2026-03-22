//! Daemon: listens for IPC requests and manages sessions.
//!
//! Auto-exits when all sessions have ended.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use interprocess::local_socket::traits::Listener;

use crate::ipc::{self, Request, Response};
use crate::platform;
use crate::session::SessionManager;

/// Run the daemon main loop. This function does not return until the
/// daemon is asked to shut down or all sessions exit.
pub fn run() -> io::Result<()> {
    platform::write_pid_file()?;

    let listener = platform::create_listener()?;
    // Use a non-blocking accept with a short timeout so we can
    // periodically reap dead sessions.
    listener.set_nonblocking(interprocess::local_socket::ListenerNonblockingMode::Accept)?;

    let mut mgr = SessionManager::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut had_sessions = false;

    // Ctrl-C handler (best-effort)
    {
        let shutdown = Arc::clone(&shutdown);
        let _ = ctrlc_set(move || shutdown.store(true, Ordering::SeqCst));
    }

    while !shutdown.load(Ordering::SeqCst) {
        // Accept one connection (non-blocking).
        match listener.accept() {
            Ok(mut stream) => {
                if let Some(true) = handle_one(&mut stream, &mut mgr, &shutdown) {
                    // KillServer was requested.
                    break;
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock
                || e.kind() == io::ErrorKind::TimedOut => {
                // No incoming connection this tick.
            }
            Err(_) => {
                // Transient accept error – ignore.
            }
        }

        // Reap dead sessions.
        mgr.reap_dead();

        // Auto-exit when all sessions have ended (and we had at least one).
        if had_sessions && mgr.count() == 0 {
            break;
        }
        if mgr.count() > 0 {
            had_sessions = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    platform::remove_pid_file();
    Ok(())
}

/// Handle a single client connection. Returns `Some(true)` if KillServer.
fn handle_one(
    stream: &mut interprocess::local_socket::Stream,
    mgr: &mut SessionManager,
    _shutdown: &Arc<AtomicBool>,
) -> Option<bool> {
    let req: Request = match ipc::recv_msg(stream) {
        Ok(r) => r,
        Err(_) => return None,
    };

    let (resp, kill) = dispatch(req, mgr);
    let _ = ipc::send_msg(stream, &resp);
    Some(kill)
}

/// Dispatch a request, returning (response, should_kill_server).
fn dispatch(req: Request, mgr: &mut SessionManager) -> (Response, bool) {
    match req {
        Request::NewSession {
            name,
            command,
            cols,
            rows,
        } => match mgr.create(name, command, cols, rows) {
            Ok(n) => (Response::Created { name: n }, false),
            Err(e) => (Response::Error { message: e }, false),
        },

        Request::KillSession { name } => match mgr.kill(&name) {
            Ok(()) => (Response::Ok, false),
            Err(e) => (Response::Error { message: e }, false),
        },

        Request::ListSessions => {
            let sessions = mgr.list();
            (Response::SessionList { sessions }, false)
        }

        Request::SendData { name, data } => match mgr.send(&name, &data) {
            Ok(()) => (Response::Ok, false),
            Err(e) => (Response::Error { message: e }, false),
        },

        Request::ViewScreen { name } => match mgr.view(&name) {
            Ok(content) => (Response::Screen { content }, false),
            Err(e) => (Response::Error { message: e }, false),
        },

        Request::ResizePty { name, cols, rows } => match mgr.resize(&name, cols, rows) {
            Ok(()) => (Response::Ok, false),
            Err(e) => (Response::Error { message: e }, false),
        },

        Request::KillServer => (Response::Ok, true),

        Request::Ping => (Response::Pong, false),
    }
}

// ── Ctrl-C helper ────────────────────────────────────────────────────

#[cfg(unix)]
fn ctrlc_set(f: impl Fn() + Send + 'static) -> io::Result<()> {
    // Use a simple signal handler via libc.
    // For simplicity, we just ignore SIGINT in the daemon –
    // the daemon exits when sessions are gone or KillServer is sent.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
    let _ = f; // unused on this path but keeps the signature consistent
    Ok(())
}

#[cfg(windows)]
fn ctrlc_set(f: impl Fn() + Send + Sync + 'static) -> io::Result<()> {
    use std::sync::OnceLock;
    static HANDLER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
    HANDLER.get_or_init(|| {
        let f = f;
        Box::new(move || f())
    });

    unsafe extern "system" fn handler(_: u32) -> i32 {
        if let Some(f) = HANDLER.get() {
            f();
        }
        1 // TRUE – handled
    }

    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    // SAFETY: handler follows the required signature.
    unsafe {
        if SetConsoleCtrlHandler(Some(handler), 1) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
