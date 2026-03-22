# emx-pmux 当前限制与局限

修正 BUG-1 ~ BUG-7 及性能优化后，以下为仍然存在的已知限制。

---

## 一、架构限制

### 1. 单线程 Daemon 事件循环
- Daemon 使用 `sleep(50ms)` 轮询而非 epoll/IOCP 事件驱动
- 每轮只 accept 一个连接，高并发时存在排队延迟
- **影响**: 大量 session 或高频 RPC 调用时吞吐量受限
- **对比**: rust-expect 使用 tokio 异步运行时，expectrl 使用 async-io

### 2. 同步阻塞 IO 模型
- 每个 session 占用一个 reader 线程（阻塞 read）
- N 个 session = N 个系统线程
- **影响**: 100+ session 时线程开销显著
- **对比**: rust-expect 使用 tokio AsyncFd 多路复用，单线程管理多 session

### 3. 无 Scrollback / History Buffer
- Screen buffer 只保留当前可见区域 (cols × rows)
- 滚动出屏幕的内容永久丢失
- **影响**: `view` / `hardcopy` 只能看到最后一屏
- **对比**: rust-expect 使用 RingBuffer (默认 1MB，最大 100MB)

---

## 二、PTY 层限制

### 4. Unix: 无 SIGCHLD 处理
- 子进程退出后靠 reader 线程 EOF 检测 + daemon reap 轮询发现
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

### 8. 无协议版本化
- 请求/响应结构变更时旧客户端与新守护进程不兼容
- 无版本协商机制
- **影响**: 升级 pmux 时需要 `--stop` 后重启 daemon

### 9. 无身份验证
- 任何能连接到命名管道/socket 的进程都可控制 daemon
- Unix 靠文件权限保护，Windows Named Pipe 无额外 ACL
- **影响**: 同机多用户场景下存在安全风险

### 10. 单请求-单响应模型
- 每次 RPC 需建立新连接、完成请求、关闭连接
- 无连接复用、无流式传输
- **影响**: `stuff` + `view` 高频交互时连接开销可观

---

## 四、Screen 层限制

### 11. 不完整的 VT100/ANSI 支持
已实现的 CSI 序列:
- 光标移动 (CUU/CUD/CUF/CUB/CUP/CNL/CPL/CHA/VPA)
- 擦除 (ED/EL/ECH)
- 行操作 (IL/DL/ICH/DCH)
- 滚动 (SU/SD/DECSTBM)
- SGR (基本颜色 0-15、256 色索引)

**未实现**:
- 24-bit RGB 颜色 (SGR 38;2;r;g;b)
- 字符集切换 (G0/G1, SCS)
- DEC 私有模式 (DECCKM, DECOM, DECAWM, DECTCEM 等)
- 替代屏幕缓冲 (DECSET 1049)
- OSC 序列 (窗口标题、剪贴板等)
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

### 15. stuff 转义处理的局限
- 已实现: `\n`, `\r`, `\t`, `\\`, `\0`, `\a`, `\xHH`
- 未实现: `\e` (ESC, 0x1B)、`\uXXXX` (Unicode)、八进制 `\NNN`
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

### 18. 无优雅关闭
- `--stop` 发送 KillServer 后 daemon 立即退出
- 不等待子进程清理完成
- Unix daemon 不处理 SIGTERM（只忽略 SIGINT）

---

## 七、性能局限

### 19. Screen buffer 加锁粒度
- 整个 Screen 被一个 Mutex 保护
- reader 线程写入和 client view 读取互相阻塞
- **优化方向**: 可使用 RwLock 或 double-buffer 减少读写冲突

### 20. VTE 逐字节解析
- `vte::Parser::advance()` 每次只处理一个字节
- 这是 vte crate 的 API 限制，无法批量处理
- **影响**: 高吞吐场景（如 `cat large_file`）下 per-byte 函数调用开销

### 21. IPC JSON 序列化开销
- 每次 RPC 都做 JSON 序列化/反序列化
- SendData 将 `Vec<u8>` 序列化为 JSON 数组（每字节一个数字）
- **影响**: 发送二进制数据时 JSON 膨胀严重（1 byte → ~4 chars）
- **优化方向**: 可改用 bincode 或 MessagePack；或 SendData 单独走 raw 通道
