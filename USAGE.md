# pmux CLI Usage Guide

**pmux** is a cross-platform terminal multiplexer with PTY management and screen buffering.
Designed to be compatible with GNU screen command-line syntax.

## Architecture

```
┌─────────────────┐     IPC      ┌─────────────────┐
│   pmux client   │◄────────────►│   pmux daemon   │
│  (CLI commands) │              │  (sessions)     │
└─────────────────┘              └─────────────────┘
                                          │
                                          ▼
                                   ┌─────────────┐
                                   │   Sessions  │
                                   │  (PTY+Screen)│
                                   └─────────────┘
```

- **Daemon**: Manages all sessions in the background
- **Client**: CLI commands that communicate with the daemon via IPC
- **Session**: A PTY (pseudo-terminal) with a screen buffer for output capture

## Command Syntax

**pmux** follows GNU screen command-line conventions:

```bash
pmux [options] [command_args]
```

### Options

| Option | Short | Description | screen equivalent |
|--------|-------|-------------|-------------------|
| `--session` | `-S` | Specify session name | `-S` |
| `--list` | `-l` | List sessions | `-ls` |
| `--view` | `-v` | View/peek session (non-interactive) | (pmux extension) |
| `--resume` | `-r` | Resume/attach to session (interactive) | `-r` |
| `--detach` | `-d` | Detach session elsewhere | `-d` |
| `--exec` | `-X` | Execute command on session | `-X` |
| `--command` | `-c` | Command to run | `-c`/`-s` |
| `--status` | - | Show daemon status | (custom) |
| `--stop` | - | Stop daemon | (custom) |
| `--ping` | - | Ping daemon (health check) | (custom) |
| `--daemon` | `-D` | Run as daemon (internal) | - |

## Basic Usage

### Creating Sessions

```bash
# Create session with auto-generated name
pmux

# Create named session
pmux -S mysession

# Create session running specific command
pmux -S logs -c "tail -f /var/log/syslog"

# Create session with custom terminal size
pmux -S editor -x 120 -y 40

# Detached mode (don't attach)
pmux -d -S background-task
```

### Listing Sessions

```bash
# List all sessions
pmux -l

# Or use long form
pmux --list
```

Output format:
```
session-name: command [status]
```

Status: `running` | `dead`

### Viewing Sessions (Non-Interactive)

```bash
# View/peek session screen buffer (non-interactive, exits after display)
pmux -v -S mysession

# Or use long form
pmux --view -S mysession
```

**Difference between `-v` and `-r`:**
- `-v` / `--view`: Non-interactive peek, displays screen content and exits
- `-r` / `--resume`: Interactive attach (for future implementation, currently same as `-v`)

### Resuming/Attaching Sessions (Interactive)

```bash
# Resume/attach to a session (interactive mode)
pmux -r -S mysession

# Or with -d flag
pmux -d -r -S mysession
```

### Sending Commands to Sessions

Use `-X` flag to execute commands on running sessions:

```bash
# Send text/data (like screen -X stuff)
pmux -X -S mysession stuff "hello world"

# Send command with newline
pmux -X -S mysession stuff "ls -la"
pmux -X -S mysession stuff $'\n'

# Send special characters
pmux -X -S mysession stuff $'\x03'  # Ctrl+C

# View screen buffer
pmux -X -S mysession view

# Hardcopy - write screen to file or stdout
pmux -X -S mysession hardcopy          # output to stdout
pmux -X -S mysession hardcopy -        # explicit stdout
pmux -X -S mysession hardcopy /tmp/screen.txt  # write to file

# Resize session
pmux -X -S mysession resize 120 40

# Expect - wait for pattern to appear on screen (client-side regex poll)
pmux -X -S mysession expect 'pattern'              # default 10s timeout
pmux -X -S mysession expect 'pattern' --timeout 5000  # custom timeout (ms)

# Quit/kill session
pmux -X -S mysession quit
```

### Daemon Control

```bash
# Check daemon status
pmux --status

# Stop daemon
pmux --stop

# Health check
pmux --ping
```

## Usage Examples

### Basic Session Workflow

```bash
# 1. Create a session (daemon auto-started)
pmux -S shell

# 2. List all sessions
pmux -l
# Output: shell: /bin/bash [running]

# 3. Send commands to the session
pmux -X -S shell stuff "echo 'Hello from pmux'"
pmux -X -S shell stuff $'\n'

# 4. View the output
pmux -r -S shell

# 5. Kill the session when done
pmux -X -S shell quit
```

### Monitoring Multiple Logs

```bash
# Create log monitoring sessions
pmux -S syslog -c "tail -f /var/log/syslog"
pmux -S authlog -c "tail -f /var/log/auth.log"
pmux -S applog -c "tail -f /var/log/app.log"

# Check status
pmux -l

# View specific log
pmux -r -S syslog

# Clean up
pmux -X -S syslog quit
pmux -X -S authlog quit
pmux -X -S applog quit
```

### Long-running Background Tasks

```bash
# Start a backup task in detached mode
pmux -d -S backup -c "rsync -av /src /dest"

# Check if still running
pmux -l

# View progress
pmux -r -S backup
```

### Scripted Automation (stuff + expect)

```bash
# Create a session
pmux -S auto -c bash

# Wait for shell prompt, then run a command
pmux -X -S auto expect '\$'              # wait for $ prompt
pmux -X -S auto stuff "ls -la\n"          # send command
pmux -X -S auto expect '\$'              # wait for command to finish

# Capture output
pmux -X -S auto hardcopy /tmp/output.txt

# Clean up
pmux -X -S auto quit
```

## Comparison with GNU screen

| screen command | pmux equivalent | Notes |
|----------------|-----------------|-------|
| `screen -S name` | `pmux -S name` | Create named session |
| `screen -ls` | `pmux -l` | List sessions |
| `screen -r name` | `pmux -r -S name` | Attach/restore session |
| (no equivalent) | `pmux -v -S name` | Peek at session (non-interactive) |
| `screen -X -S name stuff "text"` | `pmux -X -S name stuff "text"` | Send data |
| `screen -X -S name hardcopy file` | `pmux -X -S name hardcopy file` | Write screen to file |
| (file required) | `pmux -X -S name hardcopy` | Write to stdout (pmux extension) |
| (no equivalent) | `pmux -X -S name expect 'pat'` | Wait for regex match on screen |
| `screen -X -S name quit` | `pmux -X -S name quit` | Kill session |
| `screen -dmS name cmd` | `pmux -d -S name -c cmd` | Detached mode |

## Session Naming

- **Auto-generated**: `0`, `1`, `2`, ... (incremental IDs)
- **Custom names**: Any string without spaces
- **Uniqueness**: Custom names must be unique; creating a duplicate returns an error

## Environment Variables

| Variable | Description |
|----------|-------------|
| `EMX_PMUX_LOG_SESSION_PATH` | Directory for session transcript logs. When set to an existing directory, each session writes a `<name>.log` file containing the full interaction text (prompts, commands, output) with escape sequences stripped. |
| `EMX_TMUX_LOG` | Debug log file path (internal diagnostics). |

### Session Logging

Session logging records the full terminal interaction transcript — everything visible on screen, with ANSI escape sequences removed.

```bash
# Enable session logging
export EMX_PMUX_LOG_SESSION_PATH=/tmp/pmux-logs
mkdir -p $EMX_PMUX_LOG_SESSION_PATH

# Start a session (daemon inherits the env var)
pmux -S mysession -c bash

# Interact with the session
pmux -X -S mysession stuff "echo hello\n"

# Log file is written in real-time
cat /tmp/pmux-logs/mysession.log
```

Log file format:
- One file per session: `<session_name>.log`
- Contains printable characters, newlines, and tabs as they appear on screen
- Escape sequences (colors, cursor movement, etc.) are stripped
- File is opened in append mode and flushed after each PTY read
- The environment variable must be set **before** the daemon starts (the daemon process inherits it)

## Platform-Specific Behavior

| Feature | Unix | Windows |
|---------|------|---------|
| Default shell | `/bin/bash` or `$SHELL` | `cmd.exe` |
| PTY implementation | POSIX PTY (rustix) | ConPTY |
| Fork/spawn | `fork()` + `execve()` | `CreateProcess` |
| IPC | Unix domain socket | Named pipe |

## Error Handling

All commands return:
- **Exit code 0**: Success
- **Exit code 1**: Error (message printed to stderr)

Common errors:
- `cannot connect to daemon`: Daemon not running (auto-started by session creation)
- `Session 'X' not found`: No session with that name
- `Session 'X' already exists`: Duplicate session name
- `session name required (use -S)`: Missing session name argument

## IPC Protocol

The client-daemon communication uses a simple request-response protocol:

**Requests:** `NewSession`, `KillSession`, `ListSessions`, `SendData`, `ViewScreen`, `ResizePty`, `KillServer`, `Ping`

**Responses:** `Created`, `Ok`, `SessionList`, `Screen`, `Error`, `Pong`

See `src/ipc.rs` for protocol details.
