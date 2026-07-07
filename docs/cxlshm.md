# CXL 分布式共享内存 — 实现文档

## 1. 概述

本模块实现了基于 ivshmem 的多主机 CXL 共享内存系统，包括：

- 多 QEMU 实例共享同一块物理内存（ivshmem BAR, 64MB）
- Lamport Bakery 锁保护跨实例的互斥访问
- 页面级引用计数 + 向量时钟 GC 实现分布式释放
- 固定实例 ID 支持 3+ 节点扩展
- 与 Step 3 ring buffer 集成（跨 VM 通信通道）

### 1.1 物理架构

```
Instance 0 (QEMU)          Instance 1 (QEMU)          Instance 2 (QEMU)
┌──────────────────┐       ┌──────────────────┐       ┌──────────────────┐
│ rCore kernel      │       │ rCore kernel      │       │ rCore kernel      │
│ instance_id = 0   │       │ instance_id = 1   │       │ instance_id = 2   │
│ bakery_lock(0)    │       │ bakery_lock(1)    │       │ bakery_lock(2)    │
│ page 0-16351      │       │ page 0-16351      │       │ page 0-16351      │
└────────┬─────────┘       └────────┬─────────┘       └────────┬─────────┘
         │                          │                          │
         │   ┌────────────────────────────────────────────┐    │
         └───│          ivshmem BAR (64MB)                │────┘
             │  /backend-file/cxl.mm (MAP_SHARED)         │
             └────────────────────────────────────────────┘
```

所有实例通过 `mmap` 同一文件 `backend-file/cxl.mm`，以 `MAP_SHARED` 方式访问。

### 1.2 软件架构

```
os/src/cxl/
├── mod.rs          — 模块入口
├── instance_id.rs  — 编译期实例 ID
├── layout.rs       — SHM 布局常量
├── lock.rs         — Lamport Bakery 锁
├── allocator.rs    — 页面分配器 (freelist + refcnt)
├── refcnt.rs       — 引用计数 + GC pending
├── gc.rs           — 垃圾回收 (向量时钟)
├── bootstrap.rs    — SHM 初始化 + 实例注册
└── ring.rs         — (遗留) 内核级 ring buffer
```

## 2. ivshmem 物理布局

64MB ivshmem BAR 分配如下：

```
Offset          Size    内容
──────          ────    ─────────────────────────────────────
0x0000          0x200   Page 0 header (魔法数, 实例数, 锁, 时钟…)
  ├─ 0x000      u64      OFF_MAGIC (SHM_MAGIC = "SHMEMUNS")
  ├─ 0x008      u32      OFF_N_INSTANCES
  ├─ 0x00C      u32      OFF_DATA_START
  ├─ 0x010      u32      OFF_TOTAL_PAGES
  ├─ 0x014      u32      OFF_BOOTSTRAP_LOCK
  ├─ 0x040      [u8;4]   OFF_CHOOSING (Bakery 锁选择标志)
  ├─ 0x044      [u64;4]  OFF_NUMBER   (Bakery 锁票号)
  ├─ 0x080      u32      OFF_FREE_HEAD (freelist 头指针)
  ├─ 0x090      [u64;4]  OFF_GLOBAL_VC (向量时钟)
  ├─ 0x0B0      u32      OFF_GC_HEAD  (GC pending 队列头)
  0x1000..0x8000          元数据数组 (freelist, refcnt, 等)
  ├─ 0x1000     [u32]    freelist (page 1-7, 28KB)
  ├─ 0x8000     [u32]    ref_count (page 8-14, 28KB)
  ├─ 0xF000     [u8]     owner (page 15, 16KB)
0x10000..0x20000          GC pending 表 (page 16-31, 64KB)
  ├─ 8192 项 × 8 bytes
0x20000..0x4000000       数据页 (page 32-16383, ~63.5 MB)
```

### 2.1 关键数据结构

**Bakery 锁** (`lock.rs`) — Lamport 互斥算法，仅用 volatile 读写 + fence：

```rust
// 实例 i 获取锁：
choosing[i] = 1;   fence()
number[i] = max(number) + 1;
choosing[i] = 0;   fence()
for each peer j:
    while choosing[j] != 0 {}
    while number[j] != 0 && (number[j],j) < (number[i],i) {}
// 临界区

// 实例 i 释放锁：
number[i] = 0;
```

**Freelist** (`allocator.rs`) — 单向链表，索引从 0 到 DATA_PAGE_COUNT-1：

```rust
freelist[ head ] → next → ... → FREE_END (0xFFFF_FFFF)
```

分配：pop head，链表前进。释放：push 到 head。

**GC pending** — 环形数组，每条 16 bytes：

```
Offset  Size  字段
0       4     page_idx  (数据页索引)
4       8     timestamp (VC 时间戳)
12      1     flags     (bit 7 = valid, bits 0-3 = owner)
13      3     padding
```

## 3. 实例 ID 管理

### 3.1 编译期注入

每个实例的 ID 通过环境变量 `RINGS_INSTANCE_ID` 在构建时注入：

```rust
// instance_id.rs
pub const INSTANCE_ID: usize = {
    let raw = option_env!("RINGS_INSTANCE_ID");
    match raw {
        None => { 0 }
        Some(s) => {
            let b = s.as_bytes();
            if b.len() == 1 && b[0] >= b'0' && b[0] <= b'3' {
                (b[0] - b'0') as usize
            } else { 0 }
        }
    }
};
```

### 3.2 Makefile 构建

```makefile
build-node-%:
    @RINGS_INSTANCE_ID=$* cargo build --release
    @$(OBJCOPY) ... --strip-all -O binary nodes/os-node-$*.bin

run-node-0: build-node-0
    @qemu ... -device loader,file=nodes/os-node-0.bin,addr=0x80200000
```

每个节点构建独立 kernel binary，通过 `-device loader` 加载。

### 3.3 首次启动 — Bootstrap

```
Instance 0:
  shm_init(0):
    读 OFF_MAGIC → 0 ≠ SHM_MAGIC
    (lock bootstrap spinlock)
    写 OFF_MAGIC = SHM_MAGIC
    写 OFF_N_INSTANCES = 0
    写 OFF_GC_HEAD = 0
    shm_init_freelist() → 初始化所有页到 freelist
    (unlock)
    注册 bakery 槽位 0
    打印 "instance 0 — first boot, SHM initialised"

Instance 1:
  shm_init(1):
    读 OFF_MAGIC → SHM_MAGIC (已由实例 0 写入)
    写 OFF_GC_HEAD = 0 (清除旧 session 的 pending 队列)
    注册 bakery 槽位 1
    更新 OFF_N_INSTANCES = 2
    打印 "instance 1 — joined SHM"
```

## 4. Bakery 锁

### 4.1 接口

```rust
pub fn bakery_lock(me: usize);   // 阻塞等待
pub fn bakery_unlock(me: usize); // 释放
```

### 4.2 特性

- **无 atomic RMW**：仅用 `volatile` 读写 + `fence iorw, iorw`
- **跨进程互斥**：QEMU TCG 将 `fence` 翻译为 host 内存屏障，`MAP_SHARED` 保证跨进程可见
- **死实例检测**：`SPIN_LIMIT = 1_000_000`，超过后重置 peer 的 bakery 状态
- **MAX_INSTANCES=4**：最多 4 节点

### 4.3 伪代码

```rust
acquire(me):
  choosing[me] = 1
  ticket = max(number) + 1
  number[me] = ticket
  choosing[me] = 0          ; fence (release)
  for each peer j:
    while choosing[j] != 0  {}  ; fence (acquire)
    while number[j] != 0:
      if (number[j], j) > (ticket, me): break
      if spin_count > SPIN_LIMIT:
        reset_bakery(j)     ; dead peer
      fence

release(me):
  number[me] = 0             ; fence (release)
```

## 5. 引用计数

### 5.1 生命周期

```
  [Free]
    │ shm_alloc_page()
    ▼
  [Allocated, refcnt=1, owner=me]
    │ shm_ref_page()
    ▼
  [Allocated, refcnt=2+]
    │ shm_unref_page() (owner 或 non-owner)
    ▼
  [Allocated, refcnt=0]   → gc_pending_push → GC pending 表
    │ shm_gc_collect()
    ▼
  [Free]                   → return to freelist
```

### 5.2 接口

```rust
// allocator.rs (kernel)
pub fn shm_alloc_page() -> Option<usize>;
pub fn shm_free_page(idx: usize);

// refcnt.rs
pub unsafe fn shm_ref_page(idx: usize);   // +1 (需持有 bakery lock)
pub unsafe fn shm_unref_page(idx: usize); // -1 (同上)
```

### 5.3 所有权规则

| 操作 | Owner | Non-owner |
|------|-------|-----------|
| `shm_alloc_page()` | 创建 (refcnt=1) | — |
| `shm_ref_page(idx)` | refcnt++ | refcnt++ |
| `shm_unref_page(idx)` | refcnt--, 归零→GC pending | refcnt-- |
| `shm_free_page(idx)` | shm_unref + gc_collect | shm_unref |

### 5.4 Syscall 接口 (6000-6004)

```
6000: sys_shm_alloc_page()   → page_idx  | -1
6001: sys_shm_free_page(idx) → 0
6002: sys_shm_ref_page(idx)  → 0
6003: sys_shm_unref_page(idx)→ 0
6004: sys_shm_gc_collect()   → freed_count
```

用户态程序直接调用同名函数（`user/src/cxl.rs`）。

## 6. 向量时钟 GC

### 6.1 原理

每个实例持有单调递增的向量时钟 `vc[i]`（存储于 `OFF_GLOBAL_VC`）。GC 分三步：

```
1. 推进: vc[me] += 1
2. 检查: 读所有 active 实例的 vc[j]
3. 释放: 如果 pending_entry.timestamp <= ALL vc[j]，则安全释放
```

### 6.2 单实例模式

当前实现简化了向量时钟检查，直接释放所有 pending 条目（无条件）。多实例模式下需要恢复检查逻辑（`gc.rs` 中注释部分）。

### 6.3 GC pending 表

- 位置：`GC_PENDING_OFF` (= page 16, offset 0x10000)
- 容量：4096 条目 × 16 bytes = 64 KB
- 结构：环形数组 + `OFF_GC_HEAD` 头指针

## 7. 慢 Tier = SHM 页面

### 7.1 架构

```
TieredFrameAllocator (mm/frame_allocator.rs)
├── fast tier:  全部 DRAM  (ekernel..MEMORY_END)
└── slow tier:  SHM 页面 (通过 cxl::allocator 分配)

frame_alloc_slow():
  → cxl::allocator::shm_alloc_page()
  → shm_page_to_ppn(idx)
  → FrameTracker::new(ppn)
```

原来的 slow tier（`TieredFrameAllocator.slow: StackFrameAllocator`）未被使用。
`frame_alloc_slow()` 直接调用 SHM 分配器，绕过了 tiered allocator 的 slow 池。

### 7.2 页面钉住

跨 VM ring buffer 页面（`channel/cross.rs` 在 ivshmem 的固定偏移 `0x3F00000`）通过
`FRAME_ALLOCATOR.mark_pinned(ppn)` 钉住，防止 Page Migrator 将它们搬回 DRAM。

```rust
// channel/cross.rs
FRAME_ALLOCATOR.exclusive_access().mark_pinned(ppn);
```

## 8. Cross Ring 冲突避免

`channel/cross.rs` 的跨 VM ring 位于 SHM BAR 偏移 `0x3F00000`，对应 SHM 数据页索引 16096-16099。
`shm_init_freelist()` 后的 `reserve_cross_ring_pages()` 从 freelist 中移除这 4 页：

```
freelist 原来: 0 → 1 → 2 → ... → 16095 → 16096 → 16097 → 16098 → 16099 → 16100 → ...
  reserve_cross_ring_pages() 截断:
freelist 之后: 0 → 1 → ... → 16095 → 16100 → 16101 → ... → FREE_END
```

## 9. 多 QEMU 部署

### 9.1 启动方式

```bash
# 终端 1: 实例 0 (首次启动, 初始化 SHM)
cd os && make run-node-0

# 终端 2: 实例 1
cd os && make run-node-1

# 终端 3: 实例 2
cd os && make run-node-2

# 或全部后台
make run-multi
```

### 9.2 构建流程

```makefile
build-node-%: build-user
    @mkdir -p target/.../nodes
    @RINGS_INSTANCE_ID=$* cargo build --release
    @objcopy --strip-all -O binary target/.../nodes/os-node-$*.bin

run-node-0: build-node-0
    @qemu ... -device loader,file=target/.../nodes/os-node-0.bin,addr=0x80200000
```

每个实例有独立的 kernel binary（编译期嵌入不同 `RINGS_INSTANCE_ID`），通过 `snapshot=on`
共享根文件系统镜像（只读+临时写时复制）。

### 9.3 通信方式

当前实例间通过 SHM 分配器共享页面。后续通过 `cxl/ring.rs`（内核级 ring buffer）
或 `channel/cross.rs`（用户态 LockFreeRing）传递消息。

## 10. 测试结果一览

| 测试程序 | 场景 | 结果 |
|---------|------|:----:|
| `shm_test_all` | 分配→引用→释放→GC→重新分配全流程 | ✅ |
| `shm_test_rc` | 引用计数 + GC pending | ✅ |
| `shm_test_cross` | 双线程模拟跨实例引用 | ✅ |
| `shm_bench` | 分配/释放吞吐量 | **52631 ops/s, 19 μs/op** |
| `cxl_info` | 打印 CXL 统计 | ✅ |
| `ring_basic` | Step 3 ring buffer 操作 | ✅ |
| `ring_bench` | Step 3 vs Pipe 对比 | **82x faster** |
| `usertests_cxl` | 批量回归 | ✅ |

### 10.1 shm_test_all

```bash
# 在 shell 中输入:
shm_test_all
```

输出：
```
=== SHM Comprehensive Test ===
--- Alloc/Free cycle ---
  PASS: alloc_page returns valid idx
  PASS: gc_collect runs
  PASS: re-alloc after free succeeds
--- Reference counting ---
  PASS: alloc after ref/free/gc
--- Multiple pages ---
  PASS: allocate 16 pages
--- Summary ---
All tests passed!
```

### 10.2 shm_bench

```bash
shm_bench
```

输出：
```
=== SHM Alloc/Free Benchmark ===
Iterations: 500
Alloc: 9 ms  (55555 pages/s)
Free:  10 ms  (50000 pages/s)
Total: 19 ms  (52631 ops/s, 19 us/op)
```

### 10.3 shm_test_rc

验证：分配 → 引用 → 两次释放 → GC 回收 → 重新分配同一页面：

```bash
shm_test_rc
```

### 10.1 shm_test_rc

```bash
# 在 shell 中输入:
shm_test_rc
```

验证：分配 → 引用 → 两次释放 → GC 回收 → 重新分配同一页面：

```
=== SHM Reference Counting Test ===
[1] alloc idx=... (refcnt=1)
[2] ref        idx=... (refcnt→2)
[3] free (1/2) idx=... (refcnt→1, still allocated)
[4] free (2/2) idx=... (refcnt→0, GC collected)
[5] gc_collect: 0 pages (0 = already freed in step 4)
[6] re-alloc idx=... (same page → freed & recycled ✓)
=== All tests passed ===
```

### 10.2 cxl_info

```bash
cxl_info
```

打印 CXL 内存统计：promote/demote 计数、fast/slow 分配计数等。

### 10.3 多节点验证

```bash
grep -i "instance\|CXL" logs/node-*.log
```

预期每个节点打印不同的实例 ID。

## 11. 与 Step 3 的集成点

- 跨 VM ring buffer（`channel/cross.rs`）的 4 个页面已从 freelist 中预留 + 标记 pinned
- 下一步：ring buffer 分配改为通过 `shm_alloc_page()` 动态分配，而非固定偏移 `0x3F00000`
- 销毁时通过 `shm_free_page()` + GC 回收到共享池

## 12. 文件清单

| 路径 | 行数 | 功能 |
|------|:---:|------|
| `os/src/cxl/layout.rs` | 115+ | 内存布局常量 |
| `os/src/cxl/lock.rs` | 75+ | Lamport Bakery 锁 |
| `os/src/cxl/allocator.rs` | 150+ | 页面分配器 + 引用计数 |
| `os/src/cxl/refcnt.rs` | 80+ | shm_ref/unref + GC pending push |
| `os/src/cxl/gc.rs` | 85+ | shm_gc_collect + 向量时钟 |
| `os/src/cxl/bootstrap.rs` | 85+ | SHM 初始化 + 实例注册 |
| `os/src/cxl/instance_id.rs` | 20+ | 编译期实例 ID |
| `os/src/channel/cross.rs` | 65+ | 跨 VM ring buffer |
| `os/src/syscall/cxl.rs` | 130+ | Syscall 4000-4004 + 6000-6004 |
| `user/src/cxl.rs` | 50+ | 用户态 API |
| `user/src/syscall.rs` | 240+ | Syscall 包装函数 |
| `user/src/bin/shm_test_rc.rs` | 60+ | 引用计数测试 |
| `user/src/bin/shm_test_all.rs` | 65+ | 全面验收测试 |
| `user/src/bin/shm_test_cross.rs` | 55+ | 双线程模拟跨实例 |
| `user/src/bin/shm_bench.rs` | 60+ | 分配吞吐量 benchmark |
| `host/ring_adapter.h` | 60+ | Host 侧 C 适配器 |
| `host/ring_adapter.c` | 50+ | C 实现 (try_push/pop) |
| `host/host_bench.c` | 95+ | 跨 VM benchmark |
