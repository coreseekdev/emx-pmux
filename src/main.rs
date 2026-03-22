//! pmux - A terminal multiplexer for Windows
//!
//! Inspired by tmux and screen, pmux provides session management,
//! window/pane splitting, and persistent terminal sessions.

use pmux::daemon::{DaemonManager, DaemonStatus};
use pmux::platform;
use std::env;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    let command = &args[1];
    let mut mgr = DaemonManager::new();

    // Parse -L namespace flag (global, before command)
    let mut cmd_idx = 1;
    for i in 1..args.len() {
        if args[i] == "-L" {
            if let Some(ns) = args.get(i + 1) {
                mgr = mgr.with_namespace(ns.clone());
                cmd_idx = i + 2;
                break;
            }
        }
    }

    // Skip -L and namespace for command matching
    let actual_cmd = if cmd_idx < args.len() {
        &args[cmd_idx]
    } else {
        print_usage();
        process::exit(1);
    };

    let result = match actual_cmd.as_str() {
        "start" => {
            // pmux start [-n name]
            let name = parse_flag(&args, "-n", cmd_idx)
                .unwrap_or_else(|| mgr.next_session_name());
            match mgr.start(&name) {
                Ok(handle) => {
                    println!("Started daemon '{}' (PID: {})", name, handle.pid);
                    // Wait for daemon to be ready
                    match pmux::daemon::wait_for_daemon(&mgr.full_session_name(&name), 5000) {
                        Ok(()) => println!("Daemon '{}' is ready", name),
                        Err(e) => eprintln!("Warning: {}", e),
                    }
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }

        "stop" => {
            // pmux stop [-n name]
            let name = parse_flag(&args, "-n", cmd_idx)
                .unwrap_or_else(|| "default".to_string());
            mgr.stop(&name)
        }

        "restart" => {
            // pmux restart [-n name]
            let name = parse_flag(&args, "-n", cmd_idx)
                .unwrap_or_else(|| "default".to_string());
            // Stop first, ignore if not running
            let _ = mgr.stop(&name);
            // Then start
            match mgr.start(&name) {
                Ok(handle) => {
                    println!("Restarted daemon '{}' (PID: {})", name, handle.pid);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }

        "status" => {
            // pmux status [-n name]
            let name = parse_flag(&args, "-n", cmd_idx)
                .unwrap_or_else(|| "default".to_string());
            let status = mgr.status(&name);
            match status {
                DaemonStatus::Running => {
                    println!("Daemon '{}' is running", name);
                    Ok(())
                }
                DaemonStatus::Stopped => {
                    println!("Daemon '{}' is not running", name);
                    process::exit(1);
                }
            }
        }

        "ls" | "list-sessions" => {
            // pmux ls
            let sessions = mgr.list();
            if sessions.is_empty() {
                println!("No running sessions");
            } else {
                println!("Running sessions:");
                for name in sessions {
                    println!("  {}", name);
                }
            }
            Ok(())
        }

        "new" | "new-session" => {
            // pmux new -s name
            // This will auto-spawn the daemon if needed
            let name = parse_flag(&args, "-s", cmd_idx)
                .unwrap_or_else(|| mgr.next_session_name());
            println!("Creating session '{}'...", name);

            // TODO: Actually create the session via IPC
            // For now, just ensure daemon is running
            match mgr.ensure(&name) {
                Ok(_) => {
                    println!("Session '{}' created", name);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }

        "attach" | "attach-session" => {
            // pmux attach -t name
            let name = parse_flag(&args, "-t", cmd_idx)
                .unwrap_or_else(|| {
                    // Try to find last session or default
                    mgr.list().last()
                        .cloned()
                        .unwrap_or_else(|| "default".to_string())
                });
            println!("Attaching to '{}'...", name);

            // TODO: Implement actual attach via IPC
            // For now, just ensure daemon is running
            match mgr.ensure(&name) {
                Ok(_) => {
                    println!("Attached to '{}'", name);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }

        "daemon" => {
            // Internal command: run as daemon
            // pmux daemon -s name [-x width] [-y height]
            let name = parse_flag(&args, "-s", cmd_idx)
                .unwrap_or_else(|| "default".to_string());

            // Parse initial size
            let width: Option<u16> = parse_flag(&args, "-x", cmd_idx)
                .and_then(|s| s.parse().ok());
            let height: Option<u16> = parse_flag(&args, "-y", cmd_idx)
                .and_then(|s| s.parse().ok());
            let init_size = match (width, height) {
                (Some(w), Some(h)) => Some((w, h)),
                (Some(w), None) => Some((w, 24)),
                (None, Some(h)) => Some((80, h)),
                _ => None,
            };

            // TODO: Implement actual daemon main loop
            // For now, just write marker files
            if cfg!(windows) {
                use std::fs;
                use std::time::SystemTime;

                // Create .pmux directory
                let dir = platform::pmux_dir();
                fs::create_dir_all(&dir).ok();

                // Write port file (mock)
                let port_path = platform::port_file_path(&name);
                let port = 50000 + (SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() % 10000) as u16;
                fs::write(&port_path, format!("{}\n", port)).ok();

                // Write key file (mock)
                let key_path = platform::key_file_path(&name);
                // Simple pseudo-random key based on timestamp and PID
                let timestamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_nanos()).unwrap_or(0);
                let pid = std::process::id();
                // Use u64 arithmetic only
                let timestamp_u64 = (timestamp & 0xFFFFFFFFFFFFFFFF) as u64;
                let key = format!("{:016x}", timestamp_u64 ^ (pid as u64 * 0x9e3779b97f4a7c15));
                fs::write(&key_path, &key).ok();

                eprintln!("Daemon '{}' started on port {} (mock)", name, port);
            }

            // Keep process alive
            eprintln!("Daemon '{}' running (press Ctrl+C to stop)", name);
            std::thread::park();  // Park forever
            Ok(())
        }

        _ => {
            eprintln!("Unknown command: {}", actual_cmd);
            print_usage();
            process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

/// Print usage information
fn print_usage() {
    println!("pmux - A terminal multiplexer for Windows");
    println!();
    println!("Usage:");
    println!("  pmux <command> [options]");
    println!();
    println!("Commands:");
    println!("  start [-n name]       Start a daemon (optional name)");
    println!("  stop [-n name]        Stop a daemon");
    println!("  restart [-n name]     Restart a daemon");
    println!("  status [-n name]      Show daemon status");
    println!("  ls                    List running sessions");
    println!("  new -s name           Create a new session");
    println!("  attach -t name        Attach to a session");
    println!();
    println!("Options:");
    println!("  -L namespace          Session namespace (isolation)");
    println!();
    println!("Examples:");
    println!("  pmux start -n mysession");
    println!("  pmux new -s mysession");
    println!("  pmux attach -t mysession");
    println!("  pmux stop -n mysession");
}

/// Parse a flag value from arguments
fn parse_flag(args: &[String], flag: &str, start_idx: usize) -> Option<String> {
    for i in start_idx..args.len() {
        if args[i] == flag {
            return args.get(i + 1).cloned();
        }
    }
    None
}
