# pmux

A terminal multiplexer for Windows, inspired by tmux and screen.

## Overview

pmux provides session management, window/pane splitting, and persistent terminal sessions on Windows using ConPTY.

## Project Structure

```
pmux/
├── Cargo.toml          # Single crate with bin + lib targets
├── src/
│   ├── main.rs         # Binary entry point
│   ├── lib.rs          # Library entry point
│   ├── cli.rs          # CLI argument parsing
│   ├── session.rs      # Session management
│   ├── pty.rs          # PTY handling
│   ├── server.rs       # Server component
│   ├── client.rs       # Client component
│   └── ipc.rs          # IPC protocol
└── README.md
```

## Goals

- [ ] Windows-native support using ConPTY
- [ ] tmux-compatible command interface
- [ ] Session create/attach/detach
- [ ] Window/pane management
- [ ] Persistent sessions
- [ ] Multiple client support

## Status

Framework created. Implementation pending.

## License

MIT
