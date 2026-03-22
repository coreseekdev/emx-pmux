# emx-pmux 数据流分析与问题诊断

> v0.5.0 — 交互式 attach 模式 (`-r`)，terminal raw mode 支持。

## 一、数据流全图

```
┌──────────────────────────────────────────────────────────────────┐
│                          Client (main.rs)                        │
│                                                                  │
│  pmux -S test -c "cmd"    pmux -X -S test stuff "dir\n"         │
│         │                          │                             │
│         ▼                          ▼                             │
│  run_create()              run_exec() -> "stuff"                 │
│         │                          │  unescape("dir\n")          │
│         ▼                          ▼                             │
│  Message::Create           Message::SendData { data: [..,0x0A] }│
│         │                          │                             │
└─────────┼──────────────────────────┼─────────────────────────────┘
          │   ipc::write_msg()       │   ipc::write_msg()
          │   (12-byte LE header     │   (revision + type + len
          │    + binary payload)     │    + binary payload)
          ▼                          ▼
    ┌─────────────────────────────────────────┐
    │        Local Socket (IPC)               │
    │   Win: Named Pipe (\\.\pipe\pmux_daemon)│
    │   Unix: Unix domain socket (.sock)      │
    └─────────────────────────────────────────┘
          │                          │
          ▼                          ▼
┌──────────────────────────────────────────────────────────────────┐
│                        Daemon (daemon.rs)                        │
│                                                                  │
│  tokio::select! { pipe.connect()/listener.accept() }             │
│         │                          │                             │
│         ▼                          ▼                             │
│  handle_one() -> dispatch()  handle_one() -> dispatch()          │
│         │                          │                             │
│         ▼                          ▼                             │
│  mgr.create(name, cmd)     mgr.send(name, data)                 │
│         │                          │                             │
│         ▼                          ▼                             │
│  ┌──────────────────────────────────────────────────────┐       │
│  │              Session (session.rs)                     │       │
│  │                                                       │       │
│  │  ┌─────────┐   write_all()   ┌─────────────────┐    │       │
│  │  │ master  │◄────────────────│ write_data(data) │    │       │
│  │  │(PtyMaster)                └─────────────────┘    │       │
│  │  └────┬────┘                                         │       │
│  │       │ Write端                                      │       │
│  │       ▼                                              │       │
│  │  ┌──────────────────────────────┐                    │       │
│  │  │         PTY Layer            │                    │       │
│  │  │  Win: ConPTY (hpc)           │                    │       │
│  │  │       input_write ──► ConHost ──► cmd.exe stdin   │       │
│  │  │       output_read ◄── ConHost ◄── cmd.exe stdout  │       │
│  │  │  Unix: Master FD ◄──► Slave FD (child stdio)     │       │
│  │  └──────────────────────────────┘                    │       │
│  │       │ Read端 (PtyReader — no HPCON ownership)      │       │
│  │       ▼                                              │       │
│  │  ┌──────────────┐       ┌──────────────┐            │       │
│  │  │ Reader Thread │──────►│ Screen Buffer │            │       │
│  │  │  (background) │ feed()│  (VTE parser) │            │       │
│  │  │  reader.read() │      │  80x24 cells  │            │       │
│  │  └──────────────┘       └──────┬───────┘            │       │
│  │                                │ screen_text()       │       │
│  │                                ▼                     │       │
│  │                       Message::ScreenData { content }│       │
│  └──────────────────────────────────────────────────────┘       │
└──────────────────────────────────────────────────────────────────┘
```

## 二、关键数据流路径

### 2.1 写入路径 (Client → PTY Child)

```
stuff "dir\n" → unescape() → [0x64,0x69,0x72,0x0A]
  → Message::SendData{data} → ipc::write_msg() → 12-byte header + binary payload
  → session.write_data() → master.write_all()
  → [Windows] WriteFile(input_write) → ConPTY → cmd.exe stdin
  → [Unix]    rustix::io::write(fd)  → kernel PTY → child stdin
```

### 2.2 读取路径 (PTY Child → Screen Buffer)

```
cmd.exe stdout → ConPTY → output_read → ReadFile() → reader thread (PtyReader)
  → screen.lock().feed(bytes) → VTE parser → Cell grid update
```

### 2.3 查看路径 (Client → Screen Text)

```
Message::ViewScreen{name} → mgr.view() → session.screen_text()
  → screen.lock().text() → Message::ScreenData{content}
```

### 2.4 命令路径 (Client -X → Daemon dispatch)

```
pmux -S name -X stuff "text\n"
  → [stuff special case] → unescape() → Message::SendData → mgr.send()
pmux -S name -X resize 132 43
  → Message::Command{args:["resize","132","43"]} → dispatch_command() → mgr.resize()
pmux -S name -X hardcopy file.txt
  → Message::Command{args:["view"]} → mgr.view() → client writes file
```

---

## 三、已修复 Bug 历史记录

### ~~BUG-1: `stuff` 不处理转义字符~~ ✅ 已修复 (v0.3.0)

`main.rs` 中添加了 `unescape()` 函数，支持 `\n`, `\r`, `\t`, `\\`, `\0`, `\a`, `\e`, `\xHH`, `\uXXXX`。
`stuff` 通过 MSG_SEND_DATA 发送预处理后的二进制数据。

### ~~BUG-2: Windows ConPTY 双重关闭~~ ✅ 已修复 (v0.3.0)

引入独立的 `PtyReader` 类型，只持有 read pipe handle，不持有 HPCON 所有权。
`PtyMaster::try_clone()` 返回 `PtyReader` 而非 `PtyMaster`。

### ~~BUG-3: Unix 非阻塞模式导致 Reader 线程退出~~ ✅ 已修复 (v0.3.0)

移除了 `spawn()` 中的非阻塞设置。Reader 线程是专用线程，阻塞 read 是正确做法。

### ~~BUG-4: `-c` 命令不做 shell wrap~~ ✅ 已修复 (v0.3.0)

`session.rs create()` 中用 shell 包装命令：
- Unix: `config.args = vec!["-c".into(), cmd]`（使用默认 $SHELL）
- Windows: `config.program = "cmd.exe"; config.args = vec!["/c".into(), cmd]`

### ~~BUG-5: `run_status` 直接调用 `process::exit(1)`~~ ✅ 已修复 (v0.3.0)

改为 `Err("daemon is not running".into())`，让 `main()` 统一处理。

### ~~BUG-6: Daemon 自动退出未实现~~ ✅ 已修复 (v0.3.0)

在 reap 后检查计数：`if had_sessions && mgr.count() == 0 { break; }`

### ~~BUG-7: `spawn_daemon` 未传递 `-D` 给新进程~~ ✅ 已修复 (v0.2.0)

`.arg("daemon")` → `.arg("--daemon")`

---

## 四、v0.4.0 新发现并修复的 Bug

### ~~BUG-8: [P0/Critical][Windows] Named Pipe 竞态 → ERROR_PIPE_BUSY (os error 231)~~ ✅ 已修复

**现象**: `pmux -S test` 报 `cannot connect to daemon: All pipe instances are busy. (os error 231)`

**根因**: 三重缺陷叠加导致的竞态条件：

1. **`is_daemon_running()` 消耗单一 pipe 实例 (幽灵连接)**:
   `OpenOptions::new().read(true).write(true).open(pipe_path)` 实际上连接到命名管道，
   消耗了 daemon 唯一的 pipe server 实例，然后立即 drop handle（幽灵连接）。
   Daemon 必须处理这个无效连接后才能服务真正的客户端。

2. **`is_daemon_running()` 在 PIPE_BUSY 时返回 false**:
   当 daemon 忙时返回 false → `ensure_daemon()` 错误地尝试启动第二个 daemon。

3. **Daemon 仅维护单个 pipe 实例，处理连接期间没有备用 pipe**:
   在 `handle_one()` 处理期间（包括处理幽灵连接），没有新的 pipe 实例等待客户端。

**修复 (三部分)**:

```rust
// 1. platform.rs: is_daemon_running() — PIPE_BUSY 也表示 daemon 在运行
match OpenOptions::new().read(true).write(true).open(socket_path()) {
    Ok(_) => true,
    Err(e) if e.raw_os_error() == Some(231) => true, // ERROR_PIPE_BUSY
    Err(_) => false,
}

// 2. platform.rs: connect_daemon() — 在 PIPE_BUSY 时重试 (最多 20 次, 1s)
loop {
    match ClientOptions::new().open(&path) {
        Ok(client) => return Ok(client),
        Err(e) if e.raw_os_error() == Some(231) && attempts < 20 => {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(e) => return Err(e),
    }
}

// 3. daemon.rs: run_windows() — 在 handle_one 之前创建下一个 pipe
let next = platform::create_next_pipe_instance()?;
let (mut rd, mut wr) = tokio::io::split(pipe);
let kill = handle_one(&mut rd, &mut wr, mgr).await;
pipe = next;
```

### ~~BUG-9: [P1] Screen CSI L/M (Insert/Delete Lines) 使用 scroll_top 而非 cursor_y~~ ✅ 已修复

**现象**: 当光标不在滚动区域顶部时，Insert Lines (CSI L) 和 Delete Lines (CSI M) 操作错误。

**根因**: CSI L 调用 `scroll_down(n)`，CSI M 调用 `scroll_up(n)`。这两个函数使用
`scroll_top` 作为操作起点。但 VT100 规范要求 CSI L/M 从 **cursor_y** 开始操作。

**修复**: 重构 `scroll_up()`/`scroll_down()` 为带区域参数的 `scroll_up_region(top, bottom, n)` /
`scroll_down_region(top, bottom, n)`。CSI L/M 传入 `cursor_y` 作为 top。

### ~~BUG-10: [P1] Screen resize 不重置 scroll_top~~ ✅ 已修复

**现象**: 如果之前通过 DECSTBM 设置了滚动区域（scroll_top ≠ 0），resize 后 scroll_top
仍保留旧值，导致后续滚动行为异常。

**修复**: `resize()` 中增加 `self.scroll_top = 0`。

### ~~BUG-11: [P1] DECSTBM 设置后光标移动到 scroll_top 而非 home~~ ✅ 已修复

**现象**: CSI r (DECSTBM) 设置滚动区域后，光标被移到 `scroll_top` 而非 (0,0)。

**根因**: DEC VT100 规范要求 DECSTBM 后光标移到 home 位置 (row=0, col=0)。
代码中 `*self.cursor_y = *self.scroll_top` 应为 `*self.cursor_y = 0`。

**修复**: `'r' => { ... *self.cursor_y = 0; }`

### ~~BUG-12: [P2] SessionInfo 上的 serde derives 和 serde 依赖已无用~~ ✅ 已修复

IPC 已完全迁移到二进制编码，`serde::Serialize/Deserialize` derive 和 `serde` crate
依赖已移除。

### ~~BUG-13: [P2] Dead code: write_handshake/read_handshake no-op shims~~ ✅ 已修复

v0.4.0 的 IPC 协议在每条消息中嵌入 revision magic，无需独立握手。
遗留的 no-op shim 函数已移除。
---

## 五、当前仍存在的问题

### ISSUE-1: [P1] Windows Named Pipe 单实例序列化

当前 daemon 同一时刻只有一个 pipe server 实例。虽然 v0.4.0 在 handle_one 之前创建下一个实例，
但在高并发场景下仍可能出现短暂的 PIPE_BUSY 窗口（创建实例到 select 循环顶部调用
`pipe.connect()` 之间）。

**影响**: 高并发 attach 场景下可能有短暂延迟（当前通过 client 侧 retry 缓解）。

**优化方向**: 使用 `tokio::spawn` 并发处理连接，或者预创建多个 pipe 实例。

### ISSUE-2: [P1] `is_daemon_running()` 仍会产生幽灵连接

虽然 v0.4.0 修复了 PIPE_BUSY 返回 false 的问题和 client 侧重试，
但 `is_daemon_running()` 的 `Ok(_)` 分支仍会连接并立即断开 —— 产生幽灵连接
让 daemon 浪费一个 accept/handle cycle。

**优化方向**: 改用 PID 文件 + `OpenProcess()` 检查进程是否存活，避免打开 pipe。

### ISSUE-3: [P2] PTY resize 竞态

`resize()` 修改 master 端 window size 和 Screen buffer。
Reader 线程可能在 resize 过程中向旧尺寸的 Screen 写入数据。
Mutex 保护了 Screen，但 PTY 端的 resize 与子进程的 SIGWINCH 处理之间存在短暂窗口。

### ISSUE-4: [P2] `ensure_daemon()` 存在 TOCTOU 竞态

两个客户端同时运行时，都可能看到 `is_daemon_running() == false` 并尝试 spawn daemon。
其中一个的 daemon 会因为 `first_pipe_instance(true)` 失败。
当前无文件锁保护，但实际影响较小（失败的那个 daemon 直接退出，客户端的 retry 会连到成功的 daemon）。

### ISSUE-5: [P2] Windows Named Pipe 路径无用户隔离

路径硬编码为 `\\.\pipe\pmux_daemon`，多用户场景下可能冲突。
Unix 侧使用 `$XDG_RUNTIME_DIR`/`$HOME` 已有用户隔离。

### ISSUE-6: [P2] Windows 环境变量块仅在 env 非空时构造

`build_env_block()` 在 `config.env.is_empty()` 时返回空 Vec（NULL → 继承环境），
但在非空时构造完整环境块（已合并父进程环境 + 自定义变量），行为正确。
不过 `HashMap` 的无序性可能导致环境变量顺序不稳定（通常无影响）。

---

## 六、当前修复优先级总结

| 状态 | Bug ID | 描述 | 影响 |
|------|--------|------|------|
| ✅ | BUG-1 | stuff 转义处理 | v0.3.0 修复 |
| ✅ | BUG-2 | ConPTY 双重关闭 | v0.3.0 修复 |
| ✅ | BUG-3 | Unix 非阻塞 reader | v0.3.0 修复 |
| ✅ | BUG-4 | -c 命令 shell wrap | v0.3.0 修复 |
| ✅ | BUG-5 | run_status exit | v0.3.0 修复 |
| ✅ | BUG-6 | Daemon 自动退出 | v0.3.0 修复 |
| ✅ | BUG-7 | spawn_daemon 参数 | v0.2.0 修复 |
| ✅ | BUG-8 | Win pipe ERROR_PIPE_BUSY | v0.4.0 修复 — 三部分修复（retry + PIPE_BUSY detection + eager next pipe） |
| ✅ | BUG-9 | CSI L/M scroll_top → cursor_y | v0.4.0 修复 |
| ✅ | BUG-10 | resize 不重置 scroll_top | v0.4.0 修复 |
| ✅ | BUG-11 | DECSTBM cursor → home | v0.4.0 修复 |
| ✅ | BUG-12 | serde 依赖清理 | v0.4.0 修复 |
| ✅ | BUG-13 | 死代码清理 | v0.4.0 修复 |
| ✅ | BUG-14 | run_resume 无交互模式 | v0.5.0 修复 — terminal.rs + raw mode + polling attach |
| ⏳ | BUG-15 | dispatch_command stuff 不 unescape | P1 — 客户端绕过了此路径 |
| ✅ | BUG-16 | handshake 空壳函数 | v0.4.0 已修复（确认） |
| ⏳ | ISSUE-1 | Win pipe 单实例瓶颈 | P1 — retry 缓解 |
| ⏳ | ISSUE-2 | is_daemon_running 幽灵连接 | P1 — 不影响正确性 |
| ⏳ | ISSUE-3 | PTY resize 竞态 | P2 — 短暂窗口 |
| ⏳ | ISSUE-4 | ensure_daemon TOCTOU | P2 — 低频竞态 |
| ⏳ | ISSUE-5 | Win pipe 路径隔离 | P2 — 多用户场景 |

---

## 七、v0.5.0 新发现 Bug

### ~~BUG-14: [P0/Critical] `run_resume()` 与 `run_view()` 完全相同 — 无交互式 attach 模式~~ ✅ 已修复 (v0.5.0)

**现象**: `pmux -r -S test` 显示一屏文本后立即退出，不进入交互式终端。
用户预期行为如同 `screen -r`：进入 PTY 子进程的交互环境。

**根因**: `run_resume()` 函数只发送 `Message::ViewScreen` 获取文本后 `println!`，
与 `run_view()` 逻辑完全一致。

**修复**:
1. 新建 `terminal.rs` 模块：跨平台终端 raw mode 支持
   - Unix: `libc::cfmakeraw()` + `tcsetattr()`
   - Windows: `SetConsoleMode()` 禁用 ECHO/LINE_INPUT，启用 VT_INPUT/VT_PROCESSING
2. 重写 `run_resume()` 为交互式 attach 循环：
   - 进入 raw mode → 获取初始屏幕 → `tokio::select!` 主循环
   - stdin → MSG_SEND_DATA 转发 | 33ms Timer → MSG_VIEW_SCREEN 刷新
   - Ctrl-A d 触发 detach（Screen 兼容）; Ctrl-A Ctrl-A 发送字面 Ctrl-A
   - ANSI `ESC[H` + 逐行渲染 + `ESC[K` + `ESC[J` 增量刷新
   - 退出时恢复终端状态 + detach 提示

### BUG-15: [P1] `dispatch_command("stuff")` 不处理转义序列

**现象**: MSG_COMMAND{stuff, "hello\n"} 通过 daemon 的 `dispatch_command()` 处理时，
`\n` 被作为字面字符串发送（两个字节 `\` 和 `n`），而非换行符 `0x0A`。

**根因**: `daemon.rs dispatch_command()` 的 stuff 分支直接调用
`mgr.send(name, text.as_bytes())`，未经过 `unescape()` 处理。
当前客户端的 `run_exec()` 绕过 MSG_COMMAND 直接使用 MSG_SEND_DATA + `unescape()`，
所以客户端走通了。但 MSG_COMMAND stuff 作为公共协议路径是有缺陷的。

**修复**: daemon 端的 `dispatch_command("stuff")` 应对文本做 `unescape()` 处理，
或者直接移除 MSG_COMMAND stuff 路径（客户端总是走 MSG_SEND_DATA）。

### BUG-16: [P2] ~~write_handshake/read_handshake 空壳函数仍存在于 ipc.rs~~ ✅ 已确认修复

**现象**: 原以为代码中仍存在 no-op shim，但经核实 BUG-13 确实已正确修复。
