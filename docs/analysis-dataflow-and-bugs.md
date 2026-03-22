# emx-pmux 数据流分析与问题诊断

## 一、数据流全图

```
┌──────────────────────────────────────────────────────────────────┐
│                          Client (main.rs)                        │
│                                                                  │
│  pmux -S test -c "cmd"    pmux -X -S test stuff "dir"           │
│         │                          │                             │
│         ▼                          ▼                             │
│  run_create()              run_exec() -> "stuff"                 │
│         │                          │                             │
│         ▼                          ▼                             │
│  Request::NewSession       Request::SendData { data: b"dir" }   │
│         │                          │                             │
└─────────┼──────────────────────────┼─────────────────────────────┘
          │   ipc::send_msg()        │   ipc::send_msg()
          │   (4-byte len + JSON)    │   (4-byte len + JSON)
          ▼                          ▼
    ┌─────────────────────────────────────────┐
    │        Local Socket (IPC)               │
    │   Win: Named Pipe (@pmux_daemon)        │
    │   Unix: Abstract socket / .sock file    │
    └─────────────────────────────────────────┘
          │                          │
          ▼                          ▼
┌──────────────────────────────────────────────────────────────────┐
│                        Daemon (daemon.rs)                        │
│                                                                  │
│  listener.accept() -> handle_one() -> dispatch()                 │
│         │                          │                             │
│         ▼                          ▼                             │
│  mgr.create(name, cmd)     mgr.send(name, data)                 │
│         │                          │                             │
│         ▼                          ▼                             │
│  Session::spawn()          session.write_data(b"dir")            │
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
│  │       │ Read端 (try_clone)                           │       │
│  │       ▼                                              │       │
│  │  ┌──────────────┐       ┌──────────────┐            │       │
│  │  │ Reader Thread │──────►│ Screen Buffer │            │       │
│  │  │  (background) │ feed()│  (VTE parser) │            │       │
│  │  │  reader.read() │      │  80x24 cells  │            │       │
│  │  └──────────────┘       └──────┬───────┘            │       │
│  │                                │ screen_text()       │       │
│  │                                ▼                     │       │
│  │                       Response::Screen { content }   │       │
│  └──────────────────────────────────────────────────────┘       │
└──────────────────────────────────────────────────────────────────┘
```

## 二、关键数据流路径

### 2.1 写入路径 (Client → PTY Child)

```
stuff "dir" → SendData{data: b"dir"} → session.write_data() → master.write_all()
  → [Windows] WriteFile(input_write) → ConPTY → cmd.exe stdin
  → [Unix]    rustix::io::write(fd)  → kernel PTY → child stdin
```

### 2.2 读取路径 (PTY Child → Screen Buffer)

```
cmd.exe stdout → ConPTY → output_read → ReadFile() → reader thread
  → screen.lock().feed(bytes) → VTE parser → Cell grid update
```

### 2.3 查看路径 (Client → Screen Text)

```
ViewScreen{name} → mgr.view() → session.screen_text()
  → screen.lock().text() → Response::Screen{content}
```

---

## 三、cmd IO 不正确执行的根因分析

### BUG-1: ⚠️ [Critical] `stuff` 不处理转义字符

**现象**: 发送 `stuff "dir\n"` 时，cmd.exe 不执行命令。

**根因**: `run_exec()` 中 stuff 处理器直接将参数字符串转为字节发送，**不做任何转义处理**：

```rust
// main.rs run_exec() -> "stuff" 分支
let text = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
let resp = rpc(&Request::SendData {
    name: name.clone(),
    data: text.into_bytes(),  // ← 直接转字节，\n 是字面的 '\' 和 'n'
})?;
```

GNU screen 的 `stuff` 命令会解析 C 风格转义序列（`\n` → 0x0A, `\r` → 0x0D, `\x03` → Ctrl-C）。
当前 pmux 不做这个处理，所以：
- `stuff "dir\n"` → 发送字面 `dir\n`（5个字符），cmd.exe 收到 `d`, `i`, `r`, `\`, `n`
- cmd.exe 永远不会收到回车/换行，所以命令不执行

**修复方案**: 在 stuff 处理中添加 C 风格转义序列解析：
```rust
fn unescape(s: &str) -> Vec<u8> {
    let mut result = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push(b'\n'),
                Some('r') => result.push(b'\r'),
                Some('t') => result.push(b'\t'),
                Some('\\') => result.push(b'\\'),
                Some('x') => { /* 解析 hex */ }
                Some(other) => { result.push(b'\\'); /* push other */ }
                None => result.push(b'\\'),
            }
        } else {
            // push char bytes
        }
    }
    result
}
```

### BUG-2: ⚠️ [Critical][Windows] `try_clone()` 导致 ConPTY 双重关闭

**现象**: Session 结束后可能崩溃或产生未定义行为。

**根因**: `PtyMaster::try_clone()` 复制了 pipe 句柄但**共享同一个 HPCON**：

```rust
// pty/windows.rs
pub fn try_clone(&self) -> PtyResult<Self> {
    let output_read = dup(&self.output_read)?;
    let input_write = dup(&self.input_write)?;
    Ok(PtyMaster {
        hpc: self.hpc,    // ← 共享同一个 HPCON!
        input_write,
        output_read,
        open: self.open,  // ← true
        size: self.size,
    })
}

impl Drop for PtyMaster {
    fn drop(&mut self) {
        self.close();     // ← 两个 PtyMaster 都会调 ClosePseudoConsole(self.hpc)
    }
}
```

**结果**: Reader 线程结束 → 其 PtyMaster clone Drop → `ClosePseudoConsole(hpc)` → ConPTY 被关闭
→ 原始 master 的后续 write 失败 → Session 不再能通信

**修复方案**: 引入 `PtyReader` 类型，只包含 read 句柄，不持有 HPCON 所有权：
```rust
pub struct PtyReader {
    output_read: OwnedHandle,
}

impl PtyMaster {
    pub fn try_clone_reader(&self) -> PtyResult<PtyReader> {
        let output_read = dup(&self.output_read)?;
        Ok(PtyReader { output_read })
    }
}
```

### BUG-3: ⚠️ [Critical][Unix] 非阻塞模式导致 Reader 线程立即退出

**现象**: Unix 上 session 创建后无法读取任何输出。

**根因**: `spawn()` 在父进程中设置 master fd 为 `O_NONBLOCK`：

```rust
// pty/unix.rs spawn() 父进程部分
rustix::fs::fcntl_setfl(&master_fd, OFlags::NONBLOCK)?;
```

`try_clone()` 使用 `OwnedFd::try_clone()` (即 dup)，两个 fd 共享同一 file description，
所以 reader 线程的 fd 也是非阻塞的。

Reader 线程循环：
```rust
match reader.read(&mut buf) {
    Ok(0) => break,
    Ok(n) => { screen.feed(&buf[..n]); }
    Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
    Err(_) => break,  // ← WouldBlock 落到这里！立即退出！
}
```

当子进程还没来得及输出时，`read()` 返回 `EAGAIN (WouldBlock)`
→ 匹配 `Err(_) => break` → reader 线程退出 → `alive = false`

**修复方案**:
- 方案A: 移除非阻塞设置（reader 线程是专用线程，可以阻塞）
- 方案B: 在 reader 线程中处理 WouldBlock：
  ```rust
  Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
      std::thread::sleep(Duration::from_millis(10));
      continue;
  }
  ```

### BUG-4: ⚠️ [Critical] `-c` 命令不做 shell wrap

**现象**: `pmux -S test -c "tail -f /var/log/syslog"` 无法正确执行。

**根因**: `-c` 参数直接赋给 `config.program`，但 `execvp` / `CreateProcessW` 期望的是程序名：

```rust
// session.rs create()
let mut config = PtyConfig::default();
if let Some(cmd) = command {
    config.program = cmd;   // ← "tail -f /var/log/syslog" 作为程序名!
}
```

- **Unix**: `execvp("tail -f /var/log/syslog", [...])` → 找不到名为 `"tail -f /var/log/syslog"` 的可执行文件
- **Windows**: `CreateProcessW("tail -f /var/log/syslog")` → 同样失败

**修复方案**: 当指定 `-c` 时，用 shell 包装命令：
```rust
// Unix
config.program = "/bin/sh".to_string();
config.args = vec!["-c".to_string(), cmd];

// Windows
config.program = "cmd.exe".to_string();
config.args = vec!["/c".to_string(), cmd];
```

---

## 四、其他 Bug

### BUG-5: `run_status` 直接调用 `process::exit(1)`

```rust
fn run_status() -> Result<(), String> {
    if platform::is_daemon_running() {
        println!("daemon is running");
        Ok(())
    } else {
        println!("daemon is not running");
        process::exit(1);  // ← 绕过 main() 的错误处理
    }
}
```

**修复**: 改为 `Err("daemon is not running".into())`，让 main() 统一处理。

### BUG-6: Daemon 自动退出未实现

`daemon.rs` 注释中说 "Auto-exits when all sessions have ended"，但 main loop 只检查 `shutdown` 和 `KillServer`。当所有 session 都死掉后，daemon 不会自动退出，会永远空转。

**修复**: 在 reap 后检查计数：
```rust
mgr.reap_dead();
if had_sessions && mgr.count() == 0 {
    break;
}
```

### BUG-7: `spawn_daemon` 未传递 `-D` 给新进程

**已修复** (上一轮): `.arg("daemon")` → `.arg("--daemon")`

---

## 五、可重构的点

### REFACTOR-1: PTY 读写分离

当前 `PtyMaster` 同时负责读和写，`try_clone()` 克隆整个对象。应拆分为:
- `PtyMaster` — 只持有写句柄 + HPCON 所有权 + resize
- `PtyReader` — 只持有读句柄，实现 `Read`

这解决 BUG-2 (Windows ConPTY 双重关闭)，也让 API 更清晰。

### REFACTOR-2: Screen::feed() 中 ScreenPerformer 每字节创建

当前代码在 `feed()` 循环中每个字节都创建一个 `ScreenPerformer`：
```rust
pub fn feed(&mut self, data: &[u8]) {
    for &byte in data {
        let mut performer = ScreenPerformer { ... };  // 每字节创建
        self.parser.advance(&mut performer, byte);
    }
}
```

Rust 2021 的 NLL borrow checker 支持 disjoint field borrow，可将 performer 移到循环外：
```rust
pub fn feed(&mut self, data: &[u8]) {
    let mut performer = ScreenPerformer {
        cols: self.cols,
        rows: self.rows,
        cells: &mut self.cells,
        cursor_x: &mut self.cursor_x,
        // ...
    };
    for &byte in data {
        self.parser.advance(&mut performer, byte);
    }
}
```

这减少了大量冗余的指针复制（高频调用场景下有明显影响）。

### REFACTOR-3: Session 自动命名逻辑简化

当前命名逻辑:
```rust
let n = self.next_id.to_string();
self.next_id += 1;
if self.sessions.contains_key(&n) {
    loop { ... }
}
n
```

可简化为：
```rust
loop {
    let n = self.next_id.to_string();
    self.next_id += 1;
    if !self.sessions.contains_key(&n) {
        return n;
    }
}
```

### REFACTOR-4: 错误类型统一

`SessionManager` 使用 `Result<T, String>` 作为错误类型。建议改用专门的 error enum：
```rust
enum SessionError {
    NotFound(String),
    AlreadyExists(String),
    SpawnFailed(String),
    IoError(io::Error),
}
```

这使得错误处理更精确，也便于调用方按类型区分处理。

### REFACTOR-5: Windows 环境变量块

`build_env_block()` 在指定自定义环境变量时只传递了自定义变量，不继承父进程环境：
```rust
fn build_env_block(config: &PtyConfig) -> Vec<u16> {
    if config.env.is_empty() {
        return Vec::new();  // NULL → 继承环境
    }
    // 只有 config.env 中的变量，丢失了所有继承的环境！
}
```

应当先获取当前进程的环境变量，然后合并用户指定的变量。

### REFACTOR-6: Daemon 信号处理

Unix daemon 直接忽略 SIGINT (`SIG_IGN`)，不处理 SIGTERM。
应增加 SIGTERM 处理以支持系统 service 管理器 (systemd) 的优雅停止。

### REFACTOR-7: IPC 协议版本化

当前 IPC 是裸 JSON。未来如果修改 Request/Response 结构，旧 client 和新 daemon 不兼容。
建议在 wire protocol 中加入版本号字段。

---

## 六、修复优先级

| 优先级 | Bug ID | 描述 | 影响范围 |
|--------|--------|------|----------|
| P0 | BUG-1 | stuff 不处理转义字符 | 所有平台 — 命令无法执行 |
| P0 | BUG-3 | Unix 非阻塞导致 reader 退出 | Unix — 完全无输出 |
| P0 | BUG-4 | `-c` 命令不做 shell wrap | 所有平台 — 自定义命令失败 |
| P1 | BUG-2 | Windows ConPTY 双重关闭 | Windows — 潜在崩溃/UB |
| P2 | BUG-5 | run_status 直接 exit | 所有平台 — 不影响功能 |
| P2 | BUG-6 | Daemon 不自动退出 | 所有平台 — 资源泄漏 |
