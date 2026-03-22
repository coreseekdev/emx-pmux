# emx-pmux 当前限制与局限

v0.4.0 — IPC 协议重构为 Screen 兼容二进制分帧，MSG_* 常量与 Screen 一致，纯二进制编码（移除 JSON）。

---

## 一、架构限制

### ~~1. 单线程 Daemon 事件循环~~ ✅ 已解决 (v0.3.0)
- ~~Daemon 使用 `sleep(50ms)` 轮询而非 epoll/IOCP 事件驱动~~
- **现状**: Daemon 使用 `tokio::select!` 事件驱动，Unix 用 `UnixListener`，Windows 用 Named Pipe
- 每轮处理一个连接仍然是串行的（accept → handle → next accept），可考虑 `tokio::spawn` 并发处理

### ~~2. 同步阻塞 IO 模型~~ ⚠️ 部分解决 (v0.3.0)
- IPC 层已完全异步 (`AsyncRead`/`AsyncWrite`)
- 每个 session 的 PTY reader 线程仍使用 `std::thread` + 阻塞 `read()`
- **原因**: PTY 文件描述符 / HANDLE 不支持原生 async，阻塞 read 是正确做法
- **影响**: N 个 session = N 个 OS 线程（但 overhead 远小于 N 个同步 IPC 连接）
- **对比**: rust-expect 在 Unix 上使用 `AsyncFd` 包装 PTY fd（仅限 Linux epoll）

### 3. 无 Scrollback / History Buffer
- Screen buffer 只保留当前可见区域 (cols × rows)
- 滚动出屏幕的内容永久丢失
- **影响**: `view` / `hardcopy` 只能看到最后一屏
- **对比**: rust-expect 使用 RingBuffer (默认 1MB，最大 100MB)

---

## 二、PTY 层限制

### 4. Unix: 无 SIGCHLD 处理
- 子进程退出后靠 reader 线程 EOF 检测 + daemon reap interval (2s) 发现
- 不处理 SIGCHLD 信号，可能短暂产生僵尸进程（直到下一轮 reap）
- **对比**: rust-expect 使用 signal-hook-tokio 即时响应

### 5. Unix: fork() 安全性
- `fork()` 在多线程程序中存在已知风险（子进程继承锁状态）
- 当前 spawn 发生在 daemon 已有 reader 线程运行时
- **实际风险**: 低（子进程立即 exec），但理论上不安全
- **对比**: 可改用 posix_spawn 或在早期 fork

### 6. Windows: ConPTY 版本要求
- 需要 Windows 10 1809+ (October 2018 Update)
- 旧版 Windows 无法使用
- 运行时通过 `GetProcAddress` 检测，但无降级方案

### 7. PTY resize 竞态
- `resize()` 修改 master 端 window size 和 Screen buffer
- reader 线程可能在 resize 过程中向旧尺寸的 Screen 写入数据
- Mutex 保护了 Screen，但 PTY 端的 resize 与子进程的 SIGWINCH 处理之间存在短暂窗口

---

## 三、IPC 层限制

### ~~8. 无协议版本化~~ ✅ 已解决 (v0.3.0→v0.4.0 增强)
- ~~请求/响应结构变更时旧客户端与新守护进程不兼容~~
- **v0.3.0**: IPC 连接开头发送 magic `PMUX` + 2-byte 版本号
- **v0.4.0**: 每条消息内嵌 Screen 风格 revision magic (`0x706d7801`)，无需独立握手
- 版本不匹配时返回明确错误信息（含期望值与实际值的十六进制）

### 9. 无身份验证
- 任何能连接到命名管道/socket 的进程都可控制 daemon
- Unix 靠文件权限保护，Windows Named Pipe 无额外 ACL
- **影响**: 同机多用户场景下存在安全风险

### ~~10. 单请求-单响应模型~~ ⚠️ 部分改善 (v0.3.0)
- 每次 RPC 仍需建立新连接
- 但连接建立/收发已是 async，开销显著降低（无线程创建、无阻塞等待）
- **剩余问题**: 无流式传输（attach 模式需要持久双向连接）

---

## 四、Screen 层限制

### 11. 不完整的 VT100/ANSI 支持 ⚠️ 大幅改善 (v0.3.0)
已实现的 CSI 序列:
- 光标移动 (CUU/CUD/CUF/CUB/CUP/CNL/CPL/CHA/VPA)
- 擦除 (ED/EL/ECH)
- 行操作 (IL/DL/ICH/DCH)
- 滚动 (SU/SD/DECSTBM)
- SGR (基本颜色 0-15、256 色索引)
- ✅ 24-bit RGB 颜色 (SGR 38;2;r;g;b / 48;2;r;g;b)
- ✅ DEC 私有模式 (DECAWM 自动换行, DECTCEM 光标可见性, DECSET 1049/47/1047 替代屏幕)
- ✅ 替代屏幕缓冲 (DECSET/DECRST 1049, 保存/恢复主屏幕)
- ✅ OSC 0/1/2 序列 (窗口标题设置)

**仍未实现**:
- 字符集切换 (G0/G1, SCS)
- DECCKM, DECOM (应用光标键、原点模式)
- OSC 52 (剪贴板)
- 鼠标追踪
- Unicode 宽字符 (CJK 双宽度字符占位)

### 12. Screen::text() 丢失属性
- `text()` 只返回纯文本，颜色和样式信息丢失
- `hardcopy` 输出也是纯文本
- **对比**: 可增加 ANSI 带颜色输出模式

---

## 五、CLI 层限制

### 13. `-r` (resume) 未实现真正的交互模式
- 当前 `-r` 和 `-v` 行为相同（显示一屏后退出）
- 无终端 raw mode、无实时输入转发、无实时输出刷新
- **这是使 pmux 成为真正 multiplexer 的最大缺失功能**

### 14. 无 detach/reattach 生命周期
- `-d` 标志被解析但未改变行为（session 创建后总是 detached）
- 无 "attached client" 概念
- **影响**: 不支持 `screen -d -r` 式的抢占 attach

### ~~15. stuff 转义处理的局限~~ ✅ 大幅改善 (v0.3.0)
- 已实现: `\n`, `\r`, `\t`, `\\`, `\0`, `\a`, `\xHH`, ✅ `\e`/`\E` (ESC 0x1B), ✅ `\uXXXX` (Unicode)
- 未实现: 八进制 `\NNN`
- Shell 的 `$'\x03'` 语法依赖 shell 展开，pmux 自身不处理 `$'...'`

---

## 六、可靠性限制

### 16. 无持久化
- Daemon 重启后所有 session 丢失
- 无 session 状态序列化/恢复机制

### 17. 错误恢复有限
- Reader 线程遇到非 Interrupted 错误直接退出循环
- PtyMaster write 错误直接透传，无重试
- Daemon accept 错误被静默忽略

### 18. 无优雅关闭 ⚠️ 部分解决 (v0.3.0)
- `--stop` 发送 KillServer 后 daemon 退出（Session Drop 触发 child.kill()）
- ✅ Unix daemon 现在处理 SIGTERM（通过 tokio signal 异步接收，优雅退出事件循环）
- ✅ Windows daemon 处理 Ctrl-C（通过 tokio::signal::ctrl_c）
- **剩余**: 退出时不等待子进程完全终止（kill 后立即返回）

---

## 七、性能局限

### ~~19. Screen buffer 加锁粒度~~ ⚠️ 部分解决 (v0.3.0)
- ~~整个 Screen 被一个 Mutex 保护~~
- 现状: 仍使用 `Mutex<Screen>`，但 alive 标志已改用 `AtomicBool`（无锁）
- reader 线程写入和 client view 读取互相阻塞
- **优化方向**: 可使用 RwLock 或 double-buffer 减少读写冲突

### 20. VTE 逐字节解析
- `vte::Parser::advance()` 每次只处理一个字节
- 这是 vte crate 的 API 限制，无法批量处理
- **影响**: 高吞吐场景（如 `cat large_file`）下 per-byte 函数调用开销

### ~~21. IPC JSON 序列化开销~~ ✅ 已解决 (v0.4.0)
- ~~每次 RPC 都做 JSON 序列化/反序列化~~
- ~~SendData 将 `Vec<u8>` 序列化为 JSON 数组（每字节一个数字）~~
- **现状**: IPC 重构为 Screen 兼容的纯二进制协议（`serde_json` 依赖已移除）
  - **12 字节 LE 头**: `protocol_revision` (i32) + `type` (i32) + `payload_len` (u32)
  - **Screen 兼容 magic**: `PROTOCOL_REVISION = ('p'<<24)|('m'<<16)|('x'<<8)|1` = `0x706d7801`
    （遵循 Screen 的 `('m'<<24)|('s'<<16)|('g'<<8)|4` 模式）
  - **MSG_* 常量 0–9 与 Screen 完全一致**: CREATE=0, ERROR=1, ATTACH=2, CONT=3, DETACH=4, POW_DETACH=5, WINCH=6, HANGUP=7, COMMAND=8, QUERY=9
  - **pmux 扩展 ≥100**: SEND_DATA=100, VIEW_SCREEN=101, RESIZE_PTY=102, KILL_SERVER=103, PING=104, LIST_SESSIONS=105
  - **响应类型 ≥200**: OK=200, CREATED=201, SESSION_LIST=202, SCREEN_DATA=203, PONG=204
  - **保留 ≥300**: 用于未来 expect-takeover API
  - 字符串用 u16-LE 前缀 + UTF-8，可选字符串用 0xFFFF 哨兵表示 None
  - 每条消息内嵌 revision magic，无需独立握手
  - MSG_COMMAND 支持 Screen 风格的 `-X` 命令分发
