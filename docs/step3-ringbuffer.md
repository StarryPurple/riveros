# Step 3：无锁忙轮询环形缓冲区

## 一句话概括

这个模块实现了一个**超高速的进程间/跨机器通信通道**。它让两个程序通过一块共享内存直接读写数据，不需要经过操作系统内核，也不需要 TCP/IP 协议栈，因此比传统方法快 40～80 倍。

## 为什么要做这个

### 传统通信方式的问题

假设虚拟机 A 里的一个程序想给虚拟机 B 发数据：

```
传统 TCP/IP 路径：
用户程序 → write() 系统调用 → 内核 TCP 栈 → virtio-net 网卡 → 宿主机
→ 网络线路 → 另一个机器 → 宿主机 → virtio-net → 内核 TCP → read() 系统调用 → 用户程序

每发一条消息：至少 4 次系统调用 + 2 次协议栈处理
```

CXL 技术让两台机器可以共享同一块物理内存。利用这个，我们可以：

```
CXL 共享内存路径：
用户 A 直接往共享内存写数据 → 用户 B 直接从共享内存读数据

零系统调用、零协议栈
```

### 本模块解决了什么

1. **怎么写数据到共享内存？** → 环形缓冲区（ring buffer）
2. **怎么保证数据完整性？** → 无锁算法（lock-free）
3. **怎么知道有新数据？** → 忙轮询（busy-poll）
4. **怎么通知对方？** → 通知机制（notification）

## 核心概念

### 环形缓冲区（Ring Buffer）

一个固定大小的循环数组。生产者往里面写数据，消费者从里面读。就像食堂的取餐台——厨师做好菜放上去（写），服务员取走送出去（读）。

```
初始状态（空）：
[ ][ ][ ][ ][ ][ ][ ][ ]
 ↑head/tail

写入 2 条后：
[X][X][ ][ ][ ][ ][ ][ ]
 ↑tail  ↑head

读出 1 条后：
[ ][X][ ][ ][ ][ ][ ][ ]
 ↑head
    ↑tail
```

head = 已生产的字节数（producer 更新）
tail = 已消费的字节数（consumer 更新）

当 head - tail = capacity 时 → 缓冲区满了
当 head = tail 时 → 缓冲区空了

这两个计数器是 **64 位无符号整数**，单调递增，永不回绕。实际存储位置用 `counter % capacity` 计算。

### 无锁算法（Lock-Free）

为什么这个 ring buffer 不需要锁？

因为它是 **SPSC (Single Producer, Single Consumer)**：
- 只有生产者能改 tail → 不需要跟别人抢
- 只有消费者能改 head → 不需要跟别人抢
- 读和写的是不同变量 → 不会冲突

这避免了锁的开销（加锁/解锁需要系统调用，非常慢）。

**内存顺序**：

```rust
// 生产者：
data[pos] = payload;                       // 普通写
tail.store(new_tail, Ordering::Release);   // Release：让上面的写对消费者可见

// 消费者：
let t = tail.load(Ordering::Acquire);      // Acquire：看到生产者的所有写
let value = data[read_pos];                // 普通读
```

`Release` + `Acquire` 配对保证：消费者看到 tail 更新时，也一定看到了所有数据。

### 忙轮询（Busy-Poll）

消费者发现缓冲区为空时怎么办？

- **传统方式**：告诉操作系统"我要睡了，有数据时叫我" → 上下文切换很慢（几微秒）
- **忙轮询**：原地转圈检查 → 零上下文切换，但消耗 CPU

```
while try_pop(&mut buf).is_err() {
    core::hint::spin_loop();  // CPU 提示"我在等"
}
```

单纯的忙轮询在**单核**上效率很低（两个线程互相抢 CPU）。所以提供了三种模式：

| 模式 | 方法 | 原理 | 适用场景 |
|------|------|------|---------|
| 纯忙等 | `push_spin` / `pop_spin` | 一直转圈检查 | 多核、延迟敏感 |
| 协作 | `push_yield` / `pop_yield` | 没有数据就让出 CPU | 单核、公平 |
| 混合 | `push_hybrid` / `pop_hybrid` | 先转 N 圈，不行再通知 | 通用 |

### 通知机制

当消费者忙等了一段时间还没数据，可以设置一个"叫醒我"的标志，然后进入睡眠。生产者写完数据后检查这个标志，如果设置了就唤醒消费者。

```
消费者：
1. try_pop → 没数据
2. 自旋 N 圈
3. 设置 flags |= NEED_WAKE
4. try_pop 再检查一次（防止错过）
5. 真的没数据 → sys_ring_wait() → 睡眠

生产者：
1. try_push → 成功
2. 检查 flags & NEED_WAKE → 需要通知
3. sys_ring_notify() → 唤醒消费者
```

这个协议叫 **"Missed Wakeup 避免"**，是 Linux 内核 `futex` 和 `eventfd` 使用的标准方法。

## 代码结构

```
os/src/channel/           ← 内核侧通道子系统
├── layout.rs              RingHeader 定义（head / tail / flags / capacity）
├── mod.rs                 Channel 结构体 + CHANNEL_TABLE + wait_queue
└── syscall.rs             sys_ring_create / destroy / wait / notify

os/src/channel/cross.rs   ← 跨 VM 通信（CXL 共享内存）

user/src/
├── ring.rs               ← LockFreeRing 用户态库
│                            try_push / try_pop（零 syscall）
│                            push_spin / pop_spin（纯忙等）
│                            push_yield / pop_yield（协作）
│                            push_hybrid / pop_hybrid（混合）
└── bin/
    ├── ring_basic.rs     基础功能测试
    ├── ring_bench.rs     与 Pipe 的吞吐量对比
    └── ring_cross.rs     跨 VM 通信测试（Guest 侧）

host/                     ← 宿主机侧
├── ring_adapter.h        C 语言实现的 LockFreeRing
├── ring_adapter.c        C 实现（try_push / try_pop）
└── host_bench.c          宿主机 benchmark
```

## 系统调用接口

| 编号 | 函数 | 作用 |
|:---:|------|------|
| 5000 | `sys_ring_create(cap, &vaddr)` | 创建 ring buffer，返回 fd + 虚地址 |
| 5001 | `sys_ring_mmap(fd)` | 获取映射地址 |
| 5002 | `sys_ring_destroy(fd)` | 销毁 |
| 5003 | `sys_ring_wait(fd, timeout)` | 阻塞等待通知 |
| 5004 | `sys_ring_notify(fd)` | 唤醒等待者 |
| 5005 | `sys_ring_create_cross(&vaddr)` | 创建跨 VM ring buffer |

**关键设计**：数据路径（try_push / try_pop）**不走系统调用**，直接在用户态操作共享内存。只有创建/销毁/通知才走 syscall。

## 与 io_uring 的对比

| 概念 | io_uring | 本设计 |
|------|----------|--------|
| 共享队列 | SQ（提交队列） | Ring Buffer 数据区 |
| 完成队列 | CQ（完成队列） | head / tail 计数器 |
| 共享内存 | mmap 出来的 SQ/CQ | CXL 共享内存（ivshmem）|
| 零 syscall 提交 | 用户直接写 SQ | `try_push` 直接写 ring |
| 零 syscall 收割 | 用户直接读 CQ | `try_pop` 直接读 ring |
| 提交/等待 | `io_uring_enter()` | `sys_ring_wait()` |
| 内核轮询 | `IORING_SETUP_SQPOLL` | `push_spin` / `pop_spin` |

两者都用了"共享内存队列"这个思想来避免系统调用。io_uring 绕过的是文件 I/O 路径，本设计绕过的是网络通信路径。

## 如何运行和测试

```bash
cd os && make run
```

在 shell 里输入：

```bash
ring_basic    # 基础测试（round-trip / 多线程 / 边界检测）
ring_bench    # 性能对比（vs Pipe）
cxl_info      # 打印 CXL 统计
```

基准测试结果（单核 QEMU）：

```
LockFreeRing (yield):     7 ms  → 18.3 MB/s  (比 Pipe 快 82 倍)
Pipe:                   576 ms  →  0.22 MB/s
LockFreeRing (busy):    666 ms  →  0.19 MB/s  (单核下 busy-poll 最差)
```

### 跨 VM 通信

```bash
# 宿主侧编译
cd host && make

# 终端 1：Guest
cd os && make run
# shell: ring_cross

# 终端 2：Host
./host/host_bench
```

结果：
```
Iterations: 1000, msg: 64 B
Total time: 1459 us
Avg one-way: 0.7 us          ← 不到 1 微秒！
Throughput: 87.73 MB/s       ← 跨 VM 通信
```

## 快速入门：从代码角度看

```rust
// 1. 创建 ring buffer（系统调用）
let mut vaddr: usize = 0;
let fd = sys_ring_create(4096, &mut vaddr as *mut usize);
let ring = LockFreeRing::new(vaddr as *mut u8);

// 2. 写数据（零 syscall）
let msg = [0x42u8; 64];
ring.try_push(&msg).unwrap();

// 3. 读数据（零 syscall）
let mut buf = [0u8; 64];
ring.try_pop(&mut buf).unwrap();

// 4. 忙等读写（零 syscall）
ring.push_spin(&msg);     // 一直等到写入成功
let n = ring.pop_spin(&mut buf);  // 一直等到读到数据

// 5. 销毁
sys_ring_destroy(fd);
```
