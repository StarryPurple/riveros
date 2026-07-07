# 无锁忙轮询通知环形缓冲区 — 实现文档

## 1. 设计目标与定位

第三步的核心是要**绕过 TCP/IP 网络栈**。传统 VM 间通信路径是：

```
用户态数据 → write() syscall → 内核 TCP 栈 → virtio-net → 宿主机 netdev
→ 宿主机内核 TCP 栈 → read() syscall → 对端用户态
```

每一条消息经过 **≥4 次 syscall + 2 次内核协议栈 + 序列化/反序列化**。

利用 CXL 共享内存后，路径变为：

```
用户态直接写 CXL 共享内存 → 原子更新 tail → 对端 busy-poll head → 读取
```

**零 syscall、零协议栈、零序列化。** 这是设计的核心目标。

### 1.1 与现有 CxlChannel 的关系

现有 `user/src/channel.rs` 中的 `CxlChannel` 是一个"被阉割的雏形"：

| 方面 | 当前 CxlChannel | 本设计目标 |
|------|----------------|------------|
| 原子语义 | `read_volatile`/`write_volatile`（非原子） | `AtomicU64` + `Acquire`/`Release` |
| 等待策略 | `yield_()` syscall（每次消息） | Busy-poll 零 syscall + hybrid fallback |
| 通知机制 | 无 | Doorbell flag + kernel wait queue |
| 作用域 | 仅同进程线程间 | 跨进程 + 跨 VM (Host↔Guest) |
| 绕过对象 | 无（只是共享内存 IPC） | TCP/IP 网络栈 |
| 内核支持 | 无 | 完整的 channel 子系统 |

---

## 2. 整体架构

```
宿主机 (Linux)                           QEMU                            Guest VM (rCore)
┌──────────────────┐                ┌─────────────┐                ┌──────────────────┐
│  host_bench      │                │ CXL Type-3  │                │  ring_bench      │
│  (producer)      │                │   Device     │                │  (consumer)      │
│                  │                │  ┌─────────┐ │                │                  │
│  LockFreeRing ───┼─── mmap ───────┼─►│ BAR0    │◄┼── shared ─────┼── LockFreeRing   │
│  .try_push()     │  /dev/shm      │  │ head    │ │  memory       │  .try_pop()      │
│  (zero syscall)  │                │  │ tail    │ │                │  (zero syscall)  │
│                  │                │  │ flags   │ │                │                  │
│                  │                │  │ data[N] │ │                │                  │
│                  │                │  └─────────┘ │                │                  │
│                  │                └──────┬──────┘                │                  │
│                  │                       │                       │                  │
│  sys_ring_notify │                       │ MSI-X 中断           │  sys_ring_wait   │
│  (仅 fallback)   │                       │ (硬件支持时)          │  (仅 fallback)   │
└──────────────────┘                       │                       └──────────────────┘
```

### 2.1 三层责任分离

| 层 | 位置 | 职责 | Syscall 是否参与 |
|----|------|------|:---:|
| **用户态（数据路径）** | CXL 共享内存 (mmap) | 无锁读写 (`try_push`/`try_pop`)、忙轮询 (`push_spin`/`pop_spin`) | **零 syscall** |
| **内核态（控制路径）** | rCore 内核 | 通道生命周期 (create/destroy)、等待/唤醒 (wait/notify)、页面钉住 (pinned)、与 fd_table 集成 | 仅控制路径 |
| **硬件/模拟层** | CXL BAR | 共享内存物理载体、MSI-X 中断 (可选) | — |

这与 **io_uring 的哲学完全一致**：

| io_uring | 本设计 |
|----------|--------|
| SQ/CQ 环在 mmap 共享内存中，用户态直接读写 | Ring Buffer 在 CXL 共享内存中，用户态直接读写 |
| 用户态填 SQ 无需 syscall | `try_push()` 直接写 ring buffer，零 syscall |
| 用户态读 CQ 无需 syscall | `try_pop()` 直接读 ring buffer，零 syscall |
| `io_uring_enter()` 做提交/等待 | `sys_ring_wait`/`sys_ring_notify` 做通知 |
| `IORING_SETUP_SQPOLL` 内核轮询 | `push_spin`/`pop_spin` 用户态轮询 |

---

## 3. 共享内存数据结构

### 3.1 内存布局

```
Offset  Size  Field               Type         Writer    Description
──────  ────  ──────────────────  ───────────  ────────  ────────────────────────────────
0       8     head                AtomicU64    Consumer  已消费字节总数 (单调递增，不回绕)
8       8     tail                AtomicU64    Producer  已生产字节总数 (单调递增，不回绕)
16      4     flags               AtomicU32    双方      状态标志 (见 §3.2)
20      4     capacity            u32          Init      数据区可用字节数 (创建后只读)
24      N     data[capacity]      [u8]         Producer  环形数据区
──────  ────  ──────────────────  ───────────  ────────  ────────────────────────────────
```

- `head` / `tail` 以 **64-bit 单调计数器** 实现，永不回绕。实际缓冲区偏移通过 `pos = counter % capacity` 计算。
- 数据区起始偏移固定为 24 字节，布局连续。
- 全部位于 CXL 分配的物理页内，页面被钉住 (pinned)，不会被 page migrator 迁移。

### 3.2 Flags 位定义

```rust
pub const RING_F_NEED_WAKE_C:  u32 = 1 << 0;  // consumer 请求通知（"有数据时唤醒我"）
pub const RING_F_NEED_WAKE_P:  u32 = 1 << 1;  // producer 请求通知（"有空间时唤醒我"）
pub const RING_F_PEER_READY:   u32 = 1 << 2;  // 对端已就绪映射
pub const RING_F_SHUTDOWN:     u32 = 1 << 3;  // 通道已关闭
```

### 3.3 Rust 结构体定义

```rust
// ────────── 共享头 (kernel 与 userspace 使用同一布局) ──────────

#[repr(C, align(8))]
pub struct RingHeader {
    pub head:     AtomicU64,   // offset 0
    pub tail:     AtomicU64,   // offset 8
    pub flags:    AtomicU32,   // offset 16
    pub capacity: u32,         // offset 20 (write-once at init)
    // data follows at offset 24
}

// ────────── 用户态包装 ──────────

pub struct LockFreeRing {
    base: *mut u8,               // mmap 返回的基地址 (header at base, data at base+24)
}

impl LockFreeRing {
    #[inline]
    fn hdr(&self) -> &RingHeader {
        unsafe { &*(self.base as *const RingHeader) }
    }

    #[inline]
    fn data_ptr(&self) -> *mut u8 {
        unsafe { self.base.add(24) }
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.hdr().capacity as usize
    }
}
```

### 3.4 宿主侧 C 结构体 (镜像布局)

```c
// host/ring_adapter.h
#include <stdint.h>
#include <stdatomic.h>

typedef struct {
    atomic_uint_least64_t head;
    atomic_uint_least64_t tail;
    atomic_uint_least32_t flags;
    uint32_t              capacity;
    // data follows at offset 24
} __attribute__((packed, aligned(8))) ring_header_t;
```

---

## 4. 无锁 SPSC 算法

### 4.1 为什么是 SPSC

SPSC (Single Producer, Single Consumer) 天然无锁，因为：

- `tail` **仅** producer 写入，consumer 只读
- `head` **仅** consumer 写入，producer 只读
- 没有多写者竞争，**不需要 CAS、LL/SC 或任何原子读-改-写指令**
- 64-bit 单调计数器消除 ABA 问题

### 4.2 生产者 `try_push`

```rust
/// 尝试发送数据。全部或无。不阻塞。
/// 返回值: Ok(()) 或 Err(PushError)
impl LockFreeRing {
    pub fn try_push(&self, data: &[u8]) -> Result<(), PushError> {
        let len = data.len();
        if len == 0 || len > self.capacity() {
            return Err(PushError::InvalidSize);
        }

        // (1) Acquire: 看到 consumer 最新 head (即已释放的空间)
        let head = self.hdr().head.load(Ordering::Acquire);
        // (2) Relaxed: tail 只有本线程写入
        let tail = self.hdr().tail.load(Ordering::Relaxed);

        // (3) 检查空间
        if tail.wrapping_sub(head) + len as u64 > self.capacity() as u64 {
            return Err(PushError::Full);
        }

        // (4) 循环拷贝数据 (发生在 Release tail 之前)
        let pos = (tail as usize) % self.capacity();
        let dst = self.data_ptr();
        let n = core::cmp::min(len, self.capacity() - pos);
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(),           dst.add(pos), n);
            core::ptr::copy_nonoverlapping(data.as_ptr().add(n),    dst,          len - n);
        }

        // (5) Release: 令 (4) 的写入对 consumer 可见
        self.hdr().tail.store(tail + len as u64, Ordering::Release);
        Ok(())
    }
}
```

### 4.3 消费者 `try_pop`

```rust
/// 尝试接收数据。不阻塞。
/// 返回值: Ok(实际字节数) 或 Err(PopError::Empty)
impl LockFreeRing {
    pub fn try_pop(&self, buf: &mut [u8]) -> Result<usize, PopError> {
        let max_len = buf.len();
        if max_len == 0 {
            return Ok(0);
        }

        // (1) Acquire: 看到 producer 最新 tail 及其之前的数据写入
        let tail = self.hdr().tail.load(Ordering::Acquire);
        // (2) Relaxed: head 只有本线程写入
        let head = self.hdr().head.load(Ordering::Relaxed);

        let available = tail.wrapping_sub(head) as usize;
        if available == 0 {
            return Err(PopError::Empty);
        }
        let read_len = core::cmp::min(available, max_len);

        // (3) 循环读取数据
        let pos = (head as usize) % self.capacity();
        let src = self.data_ptr();
        let n = core::cmp::min(read_len, self.capacity() - pos);
        unsafe {
            core::ptr::copy_nonoverlapping(src.add(pos),             buf.as_mut_ptr(),      n);
            core::ptr::copy_nonoverlapping(src,                     buf.as_mut_ptr().add(n), read_len - n);
        }

        // (4) Release: 令空间释放对 producer 可见
        self.hdr().head.store(head + read_len as u64, Ordering::Release);
        Ok(read_len)
    }
}
```

### 4.4 内存顺序证明

**Producer 写入 → Consumer 可见**：

```
P: data[tail%cap..] = payload       (普通写)
P: fence(Release)                   ─┐
P: tail.store(Release)               │ Release / Acquire 配对
C: tail.load(Acquire)               ─┤
C: fence(Acquire)                    │
C: read data[head%cap..]             → 读到 P 写入的值 ✓
P: fence(Release)  + tail.store(Release)
C: tail.load(Acquire) + fence(Acquire)
```

**Consumer 消费 → Producer 得知空间释放**：

```
C: head.store(Release)              ─┐
P: head.load(Acquire)               ─┤ Release / Acquire 配对
P: 计算 tail - head                    → 正确反映可用空间 ✓
```

**为什么 `head.load` 必须是 `Acquire`**？
- 如果 producer 用 `Relaxed` 读 `head`，可能读到过时的 `head` 值
- 过时的 `head` 使 producer 认为缓冲区比实际满，**拒绝写入本可写入的数据**
- 在 busy-poll 场景中，这种"虚假满"会导致死循环 → **活性问题 ≡ 正确性问题**

### 4.5 错误类型

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    Full,          // 缓冲区空间不足
    InvalidSize,   // data.len() > capacity || data.len() == 0
    Shutdown,      // 通道已关闭
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopError {
    Empty,         // 缓冲区为空
    Shutdown,      // 通道已关闭
}
```

---

## 5. 忙轮询策略

### 5.1 纯忙等模式 (Busy-Poll)

```rust
impl LockFreeRing {
    /// 忙等发送。零 syscall。CPU 100%。
    pub fn push_spin(&self, data: &[u8]) {
        loop {
            match self.try_push(data) {
                Ok(()) => return,
                Err(PushError::Full) => core::hint::spin_loop(),
                Err(PushError::Shutdown) => return,
                Err(_) => unreachable!(),
            }
        }
    }

    /// 忙等接收。零 syscall。CPU 100%。
    pub fn pop_spin(&self, buf: &mut [u8]) -> usize {
        loop {
            match self.try_pop(buf) {
                Ok(n) => return n,
                Err(PopError::Empty) => core::hint::spin_loop(),
                Err(PopError::Shutdown) => return 0,
            }
        }
    }
}
```

`core::hint::spin_loop()` 在 RISC-V 上编译为 `pause` 等效指令（实际为空操作 nop，但向 CPU 提示这是自旋循环，利于功耗优化）。

### 5.2 混合模式 (Hybrid)

```rust
impl LockFreeRing {
    /// 混合发送：先自旋 N 次，无结果则请求通知并阻塞。
    /// fd: 由 sys_ring_create 返回的文件描述符
    pub fn push_hybrid(&self, data: &[u8], max_spin: usize, fd: usize) {
        // 阶段 1：忙等
        for _ in 0..max_spin {
            if let Ok(()) = self.try_push(data) { return; }
            core::hint::spin_loop();
        }

        // 阶段 2：设置通知标志 (Release，使 producer 看到)
        self.hdr().flags.fetch_or(RING_F_NEED_WAKE_P, Ordering::Release);

        // 阶段 3：最后一次检查 (Missed Wakeup 避免, 见 §6.1)
        if let Ok(()) = self.try_push(data) {
            self.hdr().flags.fetch_and(!RING_F_NEED_WAKE_P, Ordering::Release);
            return;
        }

        // 阶段 4：阻塞 — 仅此步涉及 syscall
        sys_ring_wait(fd, 0);
        self.hdr().flags.fetch_and(!RING_F_NEED_WAKE_P, Ordering::Release);

        // 阶段 5：被唤醒后递归重试
        self.push_hybrid(data, max_spin, fd);
    }

    /// 混合接收
    pub fn pop_hybrid(&self, buf: &mut [u8], max_spin: usize, fd: usize) -> usize {
        for _ in 0..max_spin {
            if let Ok(n) = self.try_pop(buf) { return n; }
            core::hint::spin_loop();
        }

        self.hdr().flags.fetch_or(RING_F_NEED_WAKE_C, Ordering::Release);

        if let Ok(n) = self.try_pop(buf) {
            self.hdr().flags.fetch_and(!RING_F_NEED_WAKE_C, Ordering::Release);
            return n;
        }

        sys_ring_wait(fd, 0);
        self.hdr().flags.fetch_and(!RING_F_NEED_WAKE_C, Ordering::Release);

        self.pop_hybrid(buf, max_spin, fd)
    }
}
```

### 5.3 模式选择指南

| 模式 | CPU 占用 | 延迟 | 适用场景 |
|------|:-------:|:----:|----------|
| **纯 busy-poll** | 100% (按核) | 最低 | 延迟敏感；生产者/消费者独占核心 |
| **Hybrid (max_spin=高)** | 中高 | 低 | 通用场景 |
| **Hybrid (max_spin=低)** | 中低 | 中 | 一般场景 |
| **Hybrid (max_spin=0)** | 低 | 较高 | CPU 资源紧张的降级模式 |
| **纯阻塞 (无 busy-poll)** | 极低 | 最高 | 后台非实时传输，与传统 IPC 可比 |

---

## 6. 通知机制

### 6.1 Missed Wakeup 避免协议

这是两阶段通知协议，与 Linux `eventfd`、`futex` 中的经典模式一致。解决的核心竞态：**consumer 即将阻塞的时刻，producer 写入了数据**。

```
Consumer 侧                                Producer 侧
─────────────────                        ─────────────────

1. try_pop → 无数据
2. 自旋 N 次 → 仍无数据
3. flags |= NEED_WAKE_C          ────a───►   (consumer 表态：「我要睡了」)
4. try_pop 再次检查                               ...
   ├─ 有数据 → 清除 NEED_WAKE_C，返回             (生产者可能在此刻写入)
   └─ 无数据 → 继续                             1. try_push 成功
5. sys_ring_wait(fd) 阻塞            ◄───b────  2. flags & NEED_WAKE_C?
                                                    ├─ 是 │
                                                    │    │ sys_ring_notify(fd)
                                                    │    │ (若无则: consumer 看到 a 没看到 b, 睡了)
                                                    └─ 否 → 什么都不做

6. (被 sys_ring_notify 或数据可用唤醒)
7. flags &= ~NEED_WAKE_C
8. goto 1
```

**不变量**：在 `NEED_WAKE_C = 1` 的区间内：

- 如果 consumer 在步骤 2-3 之间检查 try_pop 无数据，然后设置 NEED_WAKE_C = 1
- 如果 producer 在此时写入数据并检查 NEED_WAKE_C，会观察到 NEED_WAKE_C = 1，从而调用 notify
- notify 将 consumer 唤醒，consumer 从 try_pop 拿到数据

**永远不会发生"consumer 睡了但 producer 不知道"的丢失唤醒**。

### 6.2 VM 内通知（同内核，通过 wait queue）

```
sys_ring_wait(fd) {
    call sys_ring_wait()
    → 内核:
      1. 找到 CHANNEL_TABLE[fd]
      2. 当前 task → Blocked
      3. 加入 channel.wait_queue
      4. schedule()
}

sys_ring_notify(fd) {
    call sys_ring_notify()
    → 内核:
      1. 找到 CHANNEL_TABLE[fd]
      2. 从 wait_queue 取出所有 task
      3. 每个 task → Ready, add_task()
}
```

### 6.3 跨 VM 通知（CXL.io MSI-X）

真实 CXL 硬件路径：

```
Producer (Host)                     QEMU                       Consumer (Guest)
────────────────                    ────                       ────────────────
try_push() 成功
flags & NEED_WAKE_C?
  是 → write CXL doorbell ──────►  捕获写入
                                   注入 MSI-X 中断 ──────────►  IRQ handler
                                                               sys_ring_notify()
                                                               唤醒 waiter (kernel task)
```

在 QEMU 纯软件模拟环境中，跨 VM 场景**优先使用纯 busy-poll**（phase 1 MVP）。混合模式在 guest 侧退化为 `max_spin` 后继续 spin 而非阻塞，因为跨 VM 阻塞需要 MSI-X 硬件支持。

---

## 7. 系统调用接口

新增 syscall ID 5000-5005。

### 7.1 `sys_ring_create` (5000)

```rust
/// 创建一个 ring buffer 通道
/// args:
///   capacity    — 数据区字节数（必须 > 0 且 <= 2^30）
///   vaddr_out   — 用户空间指针，写入映射到的虚地址
/// returns: fd (>= 0) 或 错误码 (< 0)
pub fn sys_ring_create(capacity: usize, vaddr_out: *mut usize) -> isize;
```

内核行为：

1. 校验 `capacity`：`capacity > 0 && capacity <= 0x4000_0000`
2. 计算所需页数：`pages = (32 + capacity + PAGE_SIZE - 1) / PAGE_SIZE`
3. 从 CXL slow tier 分配连续物理页
4. 将每页标记为 `pinned`（阻止 page migrator 迁移）
5. 将页面映射到调用进程的用户地址空间（`MapType::FramedSlow`, permission=RWU）
6. 初始化 `RingHeader`：head=0, tail=0, flags=0, capacity=capacity
7. 创建内核 `Channel` 对象，注册到全局 `CHANNEL_TABLE`
8. 在进程的 `fd_table` 中注册新 fd（Channel 实现 File trait）
9. 将用户态虚地址写入 `*vaddr_out`
10. 返回 fd

### 7.2 `sys_ring_mmap` (5001)

```rust
/// 获取指定 ring 的用户态映射地址（fork 后子进程用）
/// args: fd
/// returns: vaddr 或 错误码
pub fn sys_ring_mmap(fd: usize) -> isize;
```

### 7.3 `sys_ring_destroy` (5002)

```rust
/// 销毁 ring buffer，释放 CXL 页面
/// args: fd
/// returns: 0 或 错误码
pub fn sys_ring_destroy(fd: usize) -> isize;
```

也可以通过 `close(fd)` 触发（Channel 的 File trait 实现会处理 drop）。

### 7.4 `sys_ring_wait` (5003)

```rust
/// 阻塞等待通道通知
/// args:
///   fd          — 通道文件描述符
///   timeout_ms  — 超时 (0 = 无限等待)
/// returns: 0=通知唤醒, -1=超时, -2=通道关闭
pub fn sys_ring_wait(fd: usize, timeout_ms: usize) -> isize;
```

### 7.5 `sys_ring_notify` (5004)

```rust
/// 通知通道上的等待者
/// args: fd
/// returns: 0 或 错误码
pub fn sys_ring_notify(fd: usize) -> isize;
```

### 7.6 `sys_ring_create_cross` (5005)

```rust
/// 在 ivshmem 固定偏移处创建跨 VM 双工 ring buffer
/// ivshmem BAR 内偏移 0x3F00000 处分配 16KB，划分为两个 8KB 的 ring：
///   Ring 0 (Host→Guest): base + 0x0000
///   Ring 1 (Guest→Host): base + 0x2000
/// args: vaddr_out — 输出映射到的用户虚地址
/// returns: 0 = 成功
pub fn sys_ring_create_cross(vaddr_out: &mut usize) -> isize;
```

内核行为：

1. 使用 `MapType::Linear` 将 ivshmem 的 `0x3F00000` 偏移处的物理页映射到用户态
2. 初始化 Ring 0 和 Ring 1 的 `RingHeader` (head=0, tail=0, flags=0, capacity=8152)
3. 不注册到 CHANNEL_TABLE（跨 VM 通信不需要内核通知——使用纯 busy-poll）
4. 虚地址写入 `*vaddr_out`
5. 不支持销毁（ivshmem 内存由 QEMU 管理）

### 7.7 用户态 C 接口总览

```rust
// user/src/ring.rs — 所有 API

impl LockFreeRing {
    // ── 生命周期 (syscall) ──
    pub fn create(capacity: usize) -> Result<(usize, Self), RingError>;  // (fd, ring)
    pub fn attach(phys_addr: usize, size: usize) -> Result<(usize, Self), RingError>;
    pub fn destroy(fd: usize) -> isize;
    pub fn wait(fd: usize, timeout_ms: usize) -> isize;
    pub fn notify(fd: usize) -> isize;

    // ── 数据路径 (零 syscall) ──
    pub fn try_push(&self, data: &[u8]) -> Result<(), PushError>;
    pub fn try_pop(&self, buf: &mut [u8]) -> Result<usize, PopError>;

    // ── 忙轮询包装 (零 syscall) ──
    pub fn push_spin(&self, data: &[u8]);
    pub fn pop_spin(&self, buf: &mut [u8]) -> usize;

    // ── 混合模式 (最终 fallback 含 syscall) ──
    pub fn push_hybrid(&self, data: &[u8], max_spin: usize, fd: usize);
    pub fn pop_hybrid(&self, buf: &mut [u8], max_spin: usize, fd: usize) -> usize;

    // ── 状态查询 (零 syscall) ──
    pub fn is_empty(&self) -> bool;
    pub fn is_full(&self) -> bool;
    pub fn used(&self) -> usize;
    pub fn available(&self) -> usize;
}
```

---

## 8. 内核子系统设计

### 8.1 Channel 内核结构

```rust
// os/src/channel/mod.rs

use crate::mm::{FrameTracker, VirtAddr};
use crate::task::TaskControlBlock;
use alloc::collections::VecDeque;
use alloc::sync::Arc;

/// 内核侧 channel 表示
pub struct Channel {
    pub id:         usize,
    pub capacity:   usize,
    pub is_shutdown: bool,
    // 物理页面 (pinned, 不可迁移)
    pub header_frame: FrameTracker,
    pub data_frames:  Vec<FrameTracker>,
    // 用户态映射地址 (内核记录以便释放)
    pub user_vaddr: VirtAddr,
    // 通知等待队列
    pub wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

lazy_static! {
    pub static ref CHANNEL_TABLE: UPIntrFreeCell<Vec<Option<Arc<Channel>>>> =
        unsafe { UPIntrFreeCell::new(Vec::new()) };
}
```

### 8.2 File trait 实现

```rust
impl File for Channel {
    fn readable(&self) -> bool { true }
    fn writable(&self) -> bool { true }

    fn read(&self, buf: UserBuffer) -> usize {
        // 内核态阻塞读: 自旋 + 必要时 sleep on wait_queue
        // 这是兼容回退路径。高性能场景应用 LockFreeRing 用户态 API。
        loop {
            // 检查共享内存中的 tail-head 决定是否可读
            // 若无数据，调用 task::block_current_and_run_next()
            // 被唤醒后重试
        }
    }

    fn write(&self, buf: UserBuffer) -> usize {
        // 内核态阻塞写: 对称实现
    }

    fn close(&self) {
        // 释放 pinned 页面，从 CHANNEL_TABLE 移除
    }
}
```

### 8.3 页面钉住（与 Step 2 集成）

Ring buffer 的物理页面在整个生命周期内物理地址不可变。若 page migrator 将 ring 页从 fast tier 搬到 slow tier（或反之），对端（Host 或 Guest 中的另一个进程）会因物理地址变化而访问失效的数据。

**实现方案**：在 `TieredFrameAllocator` 中维护一个 `BTreeSet<PhysPageNum>` 记录所有 pinned 页面：

```rust
// os/src/mm/frame_allocator.rs
pub struct TieredFrameAllocator {
    // ...
    pub pinned_pages: BTreeSet<PhysPageNum>,
}

impl TieredFrameAllocator {
    pub fn mark_pinned(&mut self, ppn: PhysPageNum) {
        self.pinned_pages.insert(ppn);
    }
    pub fn is_pinned(&self, ppn: PhysPageNum) -> bool {
        self.pinned_pages.contains(&ppn)
    }
    // dealloc() 自动调用 self.pinned_pages.remove(&ppn)
}
```

选择 `BTreeSet` 而非 `FrameTracker` 内嵌字段，是因为 page migrator 的 `tick()` 通过 `collect_ptes()` 遍历 PTE 并解析 PPN，无法回溯到原始的 `FrameTracker` 实例。在帧分配器层面追踪 pinned 状态是最低侵入的方案。

**Page Migrator (`os/src/mm/page_migrator.rs`)** — `tick()` 循环中跳过 pinned 页面：

```rust
for (vpn, pte) in ptes {
    let ppn = pte.ppn();
    let alloc = FRAME_ALLOCATOR.exclusive_access();
    if alloc.is_pinned(ppn) { continue; }
    let tier = alloc.page_tier(ppn);
    // ... 热度追踪和迁移 (仅对非 pinned 页面执行)
}
```

**`ring_create` (`os/src/channel/mod.rs`)** — 映射完成后将所有 ring 页面标记为 pinned：

```rust
inner.memory_set.push(area, None);  // MapArea 分配并映射页面

// 遍历 ring 范围的 VPN，通过页表找到每个 PPN 并标记 pinned
let mut alloc = FRAME_ALLOCATOR.exclusive_access();
for vpn_i in start_vpn..start_vpn + page_count {
    let vpn = VirtPageNum::from(vpn_i);
    if let Some(pte) = inner.memory_set.translate(vpn) {
        alloc.mark_pinned(pte.ppn());
    }
}
```

`frame_dealloc` / `ring_destroy` 时，`TieredFrameAllocator::dealloc()` 自动从 `pinned_pages` 中移除对应 PPN，无需手动 unpin。

---

## 9. 跨 VM 通信方案

### 9.1 QEMU 配置

使用 `ivshmem-plain` 设备提供宿主机与 Guest 共享的 64MB 物理内存：

```makefile
# os/Makefile QEMU_ARGS
QEMU_ARGS += -object memory-backend-file,id=shm0,size=64M,\
               mem-path=../backend-file/cxl.mm,share=on
QEMU_ARGS += -device ivshmem-plain,memdev=shm0
```

- `memory-backend-file`：用本地文件 `/backend-file/cxl.mm` 后备 64MB 内存
- `share=on`：允许文件被多个进程（Host 进程 + QEMU）同时 mmap
- `ivshmem-plain`：QEMU PCI 设备，将后备文件映射为 Guest 的 PCI BAR

### 9.2 物理内存布局

ivshmem BAR (64 MB @ 0x4000_0000) 的分配：

```
Offset          Size    Content
──────          ────    ─────────────────────────────────────
0x0000_0000     128 KB  CXL allocator header (Step 5 共享页管理)
0x0002_0000     ~63 MB  CXL allocator data pages
0x03F0_0000     16 KB   ← 跨 VM ring buffer (不在 allocator 管理范围内)
  ├─ 0x00_0000   8 KB   Ring 0: Host → Guest (H 写, G 读)
  └─ 0x00_2000   8 KB   Ring 1: Guest → Host (G 写, H 读)
0x03F0_4000     ~1 MB  保留
```

两个 ring 使用固定的偏移 `0x3F0_0000`（63 MB 处），不在 CXL allocator 的 freelist 范围内，避免冲突。

### 9.3 Host 侧适配库

```c
// host/ring_adapter.h — 镜像 user/src/ring.rs 的 RingHeader 布局
typedef struct {
    atomic_uint_least64_t head;      // offset 0
    atomic_uint_least64_t tail;      // offset 8
    atomic_uint_least32_t flags;     // offset 16
    uint32_t               capacity; // offset 20
} __attribute__((packed, aligned(8))) ring_header_t;
static inline uint8_t *ring_data(ring_header_t *h) {
    return (uint8_t *)(h + 1);       // offset 24
}
```

核心操作与 Rust 端 `try_push`/`try_pop` 完全同构：

```c
int ring_try_push(ring_header_t *hdr, const uint8_t *data, size_t len) {
    if (len == 0 || len > hdr->capacity) return -1;
    uint64_t head = atomic_load_explicit(&hdr->head, memory_order_acquire);
    uint64_t tail = atomic_load_explicit(&hdr->tail, memory_order_relaxed);
    // ... 检查空间 → 循环拷贝 → Release 释放 tail
    ring_set_tail(hdr, tail + len);
    return 0;
}
```

Host 通过 `mmap("backend-file/cxl.mm")` 访问共享内存：

```c
int fd = open("../backend-file/cxl.mm", O_RDWR);
void *base = mmap(NULL, 64*1024*1024, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0);
ring_header_t *r0 = (ring_header_t *)((uint8_t*)base + 0x3F00000); // H→G
ring_header_t *r1 = (ring_header_t *)((uint8_t*)base + 0x3F02000); // G→H
```

### 9.4 Guest 工作流

```
1. 调用 sys_ring_create_cross(&vaddr)
2. 内核将 ivshmem BAR 的 [0x3F00000..0x3F04000) 映射到用户 VA
3. 初始化 Ring 0 和 Ring 1 的 RingHeader
4. 返回 vaddr
5. 从 vaddr 创建两个 LockFreeRing:
   let ring_h2g = LockFreeRing::new(vaddr);
   let ring_g2h = LockFreeRing::new(vaddr + 0x2000);
6. ring_h2g.pop_spin() 等待 Host 消息
7. ring_g2h.push_spin() 向 Host 发回声
```

### 9.5 运行方式

需要两个终端：

```bash
# 终端 1: 启动 QEMU + Guest
cd os && make run
# 在 rCore shell 中输入: ring_cross

# 终端 2: 启动 Host 测试
cd host && make && ./host_bench
```

### 9.6 实际测试结果

```
Host rings at: r0=0x70229d100000  r1=0x70229d102000
Guest ring capacities: 8168  8168

=== Cross-VM Ring Benchmark ===
Iterations: 1000, msg: 64 B
Total time: 1459 us
Avg one-way: 0.7 us
Throughput: 87.73 MB/s
```

对比 VM 内 Pipe 的单向延迟 ~288 μs（576ms/2000），跨 VM 延迟 **0.7 μs**，提升 **~400 倍**。

---

## 10. 与现有子系统的集成

### 10.1 与 Step 2 (页面分类与迁移)

| 集成点 | 方式 |
|--------|------|
| 页面分配 | `frame_alloc_slow()`, 指定 `pinned = true` |
| 页面迁移 | PageMigrator 跳过 pinned 页 |
| 页面释放 | `frame_dealloc()`, 与普通页相同 |

### 10.2 与 Step 4 (分布式后端)

第四步的一致性哈希分布可**感知通道位置**：
- `sys_ring_create` 按 key 哈希选择 CXL 池
- 通道创建后即绑定到特定池，不迁移

### 10.3 与 Step 5 (分布式共享与容错)

第五步的引用计数 GC 接口可分配通道内容纳的共享段：
- `cxl_shm_alloc()` 分配的共享内存通过 ring buffer 传输
- 传输结束后通过 GC 接口释放

### 10.4 与现有 CxlChannel

旧的 `CxlChannel` 可以重构为基于 `LockFreeRing` 的轻量包装（直接使用 `try_push`/`try_pop` + busy-poll）。或者保留用于基准对比中"旧方案"对照。

---

## 11. 项目文件布局

```
os/src/
  ├── channel/
  │   ├── mod.rs          [NEW]  CHANNEL_TABLE, Channel 结构体, ring_create/destroy/wait/notify
  │   ├── cross.rs        [NEW]  sys_ring_create_cross — 映射 ivshmem 固定偏移到用户 VA
  │   ├── layout.rs       [NEW]  RingHeader 定义 (head/tail/flags/capacity)
  │   └── syscall.rs      [NEW]  sys_ring_create/mmap/destroy/wait/notify 入口
  ├── syscall/mod.rs           [MOD] 注册 syscall 5000-5005

user/src/
  ├── ring.rs             [NEW]  LockFreeRing, RingHeader, 用户态 API
  │                            try_push/try_pop/push_spin/pop_spin
  │                            push_yield/pop_yield/push_hybrid/pop_hybrid
  └── bin/
       ├── ring_basic.rs       [NEW] 基本功能测试 (round-trip, 多线程, 边界)
       ├── ring_bench.rs       [NEW] VM 内基准测试 (yield/hybrid/busy-poll vs Pipe)
       └── ring_cross.rs       [NEW] 跨 VM 通信测试 (guest 侧回声程序)

host/
  ├── Makefile                 编译 host_bench
  ├── ring_adapter.h           与 layout.rs 布局一致的 C 头文件
  ├── ring_adapter.c           C11 atomic 实现 (try_push/try_pop/push_spin/pop_spin)
  └── host_bench.c             跨 VM benchmark (发 1000 条, 收回声, 测延迟/吞吐)
```

---

## 12. 基准测试结果

### 12.1 VM 内基准 (QEMU 单核, 2000 条 64B 消息)

```
=== Ring Buffer Benchmark (single-core QEMU) ===
Iterations: 2000, msg: 64 B

Method                 ms         B/s
─────────────────────────────────────────────
LockFreeRing (yield)      7   18285714
LockFreeRing (hybrid)    12   10666666
LockFreeRing (busy)     666     192192
Pipe                    576     222222
```

| 方法 | 耗时 (ms) | 吞吐量 (B/s) | vs Pipe |
|------|:--------:|:-----------:|:-------:|
| LockFreeRing (yield) | **7** | 18.3 MB/s | **82x** |
| LockFreeRing (hybrid) | 12 | 10.7 MB/s | 48x |
| Pipe | 576 | 0.22 MB/s | 1x |
| LockFreeRing (busy) | 666 | 0.19 MB/s | 0.9x |

**分析**：
- Busy-poll 在单核下最差（两个线程互相抢 CPU，每次时间片几乎全空转）
- Yield 模式与 Pipe 一样走协作调度，但数据路径零 syscall + 无内核拷贝 = **快 82 倍**
- Hybrid 多了一步 wait/notify 开销，但仍远优于 Pipe

### 12.2 跨 VM 基准 (Host→Guest, 1000 条 64B 消息)

```
=== Cross-VM Ring Benchmark ===
Iterations: 1000, msg: 64 B
Total time: 1459 us
Avg one-way: 0.7 us
Throughput: 87.73 MB/s
```

| 指标 | 值 | 对比 |
|------|:---:|:----:|
| 单向延迟 | **0.7 μs** | ~400x 优于 Pipe (288 μs) |
| 吞吐量 | **87.73 MB/s** | ~400x 优于 Pipe (0.22 MB/s) |
| 数据路径 syscall | **0** | 无系统调用 |
| 协议栈开销 | **0** | 无 TCP/IP 封装 |

**分析**：
- Host 和 Guest 在**不同 CPU** 上并行运行（Host 真核 + QEMU 模拟核）
- Busy-poll 在此场景下发挥全部优势：Guest pol 的同时 Host 可自由生产
- 延迟仅受 CXL 模拟内存访问延迟 + atomic 指令时间限制
- 这是"绕过 TCP/IP 网络栈"的核心成果

---

## 13. 实现状态

### ✅ Phase 1: 内核通道子系统 (已完成)
| 步骤 | 文件 | 行数 |
|------|------|:---:|
| 1.1 | `os/src/channel/layout.rs` | 41 |
| 1.2 | `os/src/channel/mod.rs` | 147 |
| 1.3 | `os/src/channel/syscall.rs` | 18 |
| 1.4 | `os/src/syscall/mod.rs` (注册) | — |
| 1.5 | `user/src/ring.rs` | 220 |

### ✅ Phase 2: 忙轮询 + 通知 (已完成)
| 步骤 | 文件 | 行数 |
|------|------|:---:|
| 2.1 | `sys_ring_wait/notify` (kernel) | 在 mod.rs 中 |
| 2.2 | syscall 5003-5004 | 注册完成 |
| 2.3 | `push_hybrid/pop_hybrid` + 双向 notify | 在 ring.rs 中 |
| 2.4 | `ring_basic.rs` | 67 |

### ✅ Phase 3: 页面钉住集成 (已完成)
| 步骤 | 文件 | 内容 |
|------|------|------|
| 3.1 | `os/src/mm/frame_allocator.rs` | `TieredFrameAllocator` 加 `pinned_pages: BTreeSet` + `mark_pinned/is_pinned` |
| 3.2 | `os/src/mm/page_migrator.rs` | `tick()` 循环头部检查 `alloc.is_pinned(ppn)` 并 `continue` |
| 3.3 | `os/src/channel/mod.rs:ring_create()` | 映射后遍历 VPN 调用 `alloc.mark_pinned(ppn)` |

### ✅ Phase 4: 基准测试 (已完成)
| 步骤 | 文件 | 内容 |
|------|------|------|
| 4.1 | `ring_bench.rs` | yield/hybrid/busy vs Pipe 对比 |
| 4.2 | 结果收集 | 单核 yield=82x Pipe, 跨 VM 0.7μs |

### ✅ Phase 5: 跨 VM 支持 (已完成)
| 步骤 | 文件 | 行数 |
|------|------|:---:|
| 5.1 | `host/ring_adapter.{h,c}` | 64 + 51 |
| 5.2 | `os/Makefile` | ivshmem 参数 |
| 5.3 | `os/src/channel/cross.rs` | `sys_ring_create_cross` |
| 5.4 | `user/src/bin/ring_cross.rs` | 71 |
| 5.5 | `host/host_bench.c` | 96 |

### 总计

| 位置 | 语言 | 行数 |
|------|:----:|:----:|
| 内核 `os/src/channel/` | Rust | ~220 |
| 用户 `user/src/ring.rs` + bins | Rust | ~360 |
| 宿主 `host/` | C | ~210 |

---

## 14. 附录：与 io_uring 的设计对比

| 概念 | io_uring | 本设计 |
|------|----------|--------|
| 共享队列 | SQ (Submission Queue) | Ring Buffer data area |
| 完成队列 | CQ (Completion Queue) | `head` / `tail` 计数器 |
| 共享内存 | mmap'd SQ/CQ rings | CXL BAR pages |
| 零 syscall 提交 | 用户态写 SQ, `smp_store_release` 更新 tail | `try_push`: Atomically store tail |
| 零 syscall 收割 | 用户态读 CQ, `smp_load_acquire` 读 head | `try_pop`: Atomically load tail |
| 提交/等待 | `io_uring_enter(IORING_ENTER_GETEVENTS, ...)` | `sys_ring_wait(fd, ...)` |
| 内核轮询 | `IORING_SETUP_SQPOLL`: 内核线程轮询 SQ | `push_spin`/`pop_spin`: 用户态线程轮询 |
| 唤醒对端 | 内核处理 SQ 后回填 CQ，设置 `cqe->flags` | Producer 检查 `NEED_WAKE_C` → `sys_ring_notify` |
| 通知延迟 | 从提交到完成：内核调度 + 处理 | 从 tail store 到 consumer 可见：内存序 + CXL 延迟 |
| 绕过对象 | 传统阻塞 I/O (read/write) | TCP/IP 网络栈 (数据封装 + 系统调用) |

### 核心区别

io_uring 解决的是 **I/O 提交路径**的 syscall 开销；本设计解决的是 **网络数据路径** 的 syscall 和协议栈开销。两者在"共享内存队列 + 环形缓冲区"的架构上同构；
io_uring 用这个模式绕过了 `read`/`write` syscall，本设计用这个模式绕过了 TCP/IP 协议栈。
