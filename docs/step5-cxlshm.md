# Step 5：分布式共享与容错抽象 — s5-hosts 实现总结

## 一句话概括

s5-hosts 在 QEMU ivshmem 上实现了 **CXL.mem Type 3 共享内存池的软件等价模型**：通过 bakery lock（跨 VM 互斥）+ refcount（引用追踪）+ vector clock GC（分布式垃圾回收）+ ring/mailbox（控制面通信），多台虚拟机可以安全地**分配、共享、释放**一块物理共享内存，并在实例崩溃时自动回收资源。

## 与真实 CXL.mem 的对应关系

| 真实 CXL.mem 协议栈 | s5-hosts 实现 |
|---|---|
| CXL.io (PCIe 枚举, DVSEC) | PCI BAR scan + ivshmem 检测 (`pci.rs`) |
| CXL.mem (HDM decoder, 地址映射) | ivshmem BAR identity mapping (`MapType::Identical`) |
| CXL.cache (MESI coherence) | **不存在硬件替代**：用 bakery lock + refcnt + VC GC 软件模拟 |
| Fabric Manager (池分配, 生命周期) | `shm_alloc_page` / `shm_free_page` + `gc_drain_on_join` |
| 多主机共享内存 | 多 QEMU 实例通过 `ivshmem-plain` + `/dev/shm/cxl.mm` 共享 |
| 控制面通知 (doorbell/mailbox) | SPSC ring buffer + 4 节点 MPSC mailbox |
| 数据面 (load/store 直访) | `cxl_mmap` (FramedShm) → 用户态直接 `load/store` |
| 两层级内存 (DRAM + CXL) | `TieredFrameAllocator` (fast/slow tier) + `PageMigrator` |

## 系统架构

```
Node 0 (kernel)             Node 1 (kernel)             Node 2 (kernel)
  │                           │                           │
  │ Bakery Lock(0)            │ Bakery Lock(1)            │ Bakery Lock(2)
  │ shm_alloc_page()          │ shm_ref_page()            │ shm_free_page()
  │ shm_gc_collect()          │ cxl_mbox_send()           │ cxl_mbox_recv()
  │                           │                           │
  └─────────────┬─────────────┴─────────────┬─────────────┘
                │                           │
       ┌────────┴───────────────────────────┴────────┐
       │        ivshmem BAR (64MB, /dev/shm/cxl.mm)  │
       │                                              │
       │  页 0: header (magic, bakery, VC, ring, mbox)│
       │  页 1-7: freelist                            │
       │  页 8-14: refcount[]                         │
       │  页 15: owner[]                              │
       │  页 16-31: GC pending                        │
       │  页 32+: data pages (16352 页)                │
       │  0x3F00000-0x3F07FFF: cross-ring + mailboxes │
       └──────────────────────────────────────────────┘
```

## 核心机制

### 1. Lamport Bakery Lock

纯 `plain load/store + fence` 实现，不依赖原子指令——因为跨 QEMU 实例的 ivshmem BAR 不支持硬件 CAS。死实例检测：`SPIN_LIMIT=1M` 次自旋后重置对方 bakery 状态。

**文件**: `os/src/cxl/lock.rs`

### 2. 页面分配器

Freelist 单向链表。分配/释放均在 bakery lock 保护下。Cross-ring + mailbox 共 9 页从 freelist 中永久预留 (`reserve_cross_ring_pages`)。

**文件**: `os/src/cxl/allocator.rs`

### 3. 引用计数 + Vector Clock GC

每个页面有 `ref_count`（引用数）和 `owner`（所有实例 ID）。当 refcnt 降至 0 时进入 GC pending 队列。`shm_gc_collect()` 推进本地 vector clock，仅当所有活跃实例的 VC >= entry timestamp 时才真正回收页面。

**文件**: `os/src/cxl/refcnt.rs`, `os/src/cxl/gc.rs`

### 4. MPSC Mailbox (多节点通信)

每个节点一个 4KB mailbox (64 槽 × 64B)。多生产者（其他节点）通过 bakery lock 串行写入；消费者（本节点）无锁读取。支持 4 节点全互联通信。

**文件**: `os/src/cxl/mbox.rs`

### 5. Session 清理 (重新加入 SHM 时的 7 步全清)

| 步骤 | 操作 | 清理内容 |
|---|---|---|
| 1 | `gc_drain_on_join()` | 强制回收所有 GC pending 页 |
| 2 | `reset_vc()` | 向量时钟归零 |
| 3 | `reset_tx()` | TX ring 清零 (head+tail+flags+data) |
| 4 | `reset_rx()` | RX ring 清零 |
| 5 | `reset_cross_area()` | 36KB cross-ring 区全零 |
| 6 | `mbox_reset_all()` | 4 个 mailbox 清零 |
| 7 | `reset_all_bakery()` | 全部 4 个 bakery slot 清零 |

**文件**: `os/src/cxl/bootstrap.rs`

## 共享内存布局

```
偏移        大小     内容
──────────────────────────────────────────────
0x000       8        SHM_MAGIC
0x008       4        N_INSTANCES
0x040      16        Bakery choosing[4]
0x044      32        Bakery number[4]
0x080       4        Freelist head
0x090      32        向量时钟 VC[4]
0x0B0       4        GC pending head
0x0C0       8        RX ring head/tail
0x0D0      16        MBOX head[4]
0x0E0      16        MBOX tail[4]
0x100       8        TX ring head/tail
0x200    4096        TX entries (64×64B)
0x1000    28KB        freelist (页 1-7)
0x8000    28KB        refcount[] (页 8-14)
0xF000     4KB        owner[] (页 15)
0x10000   64KB        GC pending (页 16-31)
0x20000  ~63MB        数据页 (页 32+)
0x3F00000  8KB        Cross Ring 0 (Host→Guest)
0x3F02000  8KB        Cross Ring 1 (Guest→Host)
0x3F03000  4KB        RX ring entries
0x3F04000  4KB        MBOX[0] (Node 0)
0x3F05000  4KB        MBOX[1] (Node 1)
0x3F06000  4KB        MBOX[2] (Node 2)
0x3F07000  4KB        MBOX[3] (Node 3)
```

## 系统调用接口

### SHM 页面管理 (Step 5)
| ID | 函数 | 作用 |
|---|---|---|
| 6000 | `sys_shm_alloc_page()` | 分配共享页 |
| 6001 | `sys_shm_free_page(idx)` | 释放共享页 |
| 6002 | `sys_shm_ref_page(idx)` | 增加引用 |
| 6003 | `sys_shm_unref_page(idx)` | 减少引用 |
| 6004 | `sys_shm_gc_collect()` | 执行垃圾回收 |
| 6005 | `sys_get_instance_id()` | 查询本机实例 ID |

### Ring 通信 (Step 3)
| ID | 函数 | 方向 |
|---|---|---|
| 4003 | `sys_cxl_tx_push(buf)` | Node0 → TX ring |
| 4004 | `sys_cxl_tx_pop(buf)` | TX ring → Node1 |
| 4005 | `sys_cxl_rx_push(buf)` | Node1 → RX ring |
| 4006 | `sys_cxl_rx_pop(buf)` | RX ring → Node0 |

### Mailbox 通信
| ID | 函数 | 作用 |
|---|---|---|
| 6006 | `sys_cxl_mbox_send(target, buf)` | 向目标节点 mailbox 发送 |
| 6007 | `sys_cxl_mbox_recv(buf)` | 从本节点 mailbox 接收 |

### CXL mmap
| ID | 函数 | 作用 |
|---|---|---|
| 4000 | `sys_cxl_meminfo(buf)` | 查询统计 |
| 4001 | `sys_cxl_mmap(size)` | 映射 CXL 页到用户态 |
| 4002 | `sys_cxl_munmap(ptr, size)` | 解除映射 |

### 消息完整性
| API | 作用 |
|---|---|
| `msg_seal(&mut m, sender)` | 附加 sender(byte 57) + u16 additive checksum(bytes 58-59) |
| `msg_verify(&m) -> Option<usize>` | 验证 checksum，返回 sender |

## 测试套件

### 单实例自动化 (`usertests_s5`)

25 个测试一键全跑：

| 类别 | 测试 |
|---|---|
| SHM 页管理 | `shm_test_rc`, `shm_test_all`, `shm_bench`(800 iters), `shm_test_gc`, `shm_test_integ`(500 pages), `shm_test_cross` |
| SHM 压力 | `stress_concurrent`(500 pages), `stress_gc`(500), `stress_refcount`(200 refs) |
| CXL mmap | `cxl_info`, `mmap_cxl`, `peterson_cxl`, `threads_arg_cxl` |
| Ring/Channel | `ring_basic`, `ring_bench`, `channel_bench_cxl` |
| 多线程业务 | `thread_cxl_sum`, `thread_cxl_maps`, `thread_cxl_matmul`, `thread_cxl_wal` |

运行：shell 中输入 `usertests_s5`

### 多实例手动测试

需同时启动 Node0 + Node1 (+ Node2)：

| 测试 | 节点数 | 说明 |
|---|---|---|
| `shm_multi_server` / `shm_multi_client` | 2 | 基础 client-server |
| `shm_multi_bulk_0/1` | 2 | 128 页批量传输 |
| `shm_multi_roundtrip_0/1` | 2 | 48 轮往返 |
| `shm_multi_gc_sync_0/1` | 2 | 跨节点 GC 同步 |
| `thread_cxl_token_0/1` | 2 | Token Bucket 限速器 |
| `thread_cxl_phil_0/1` | 2 | 跨 VM 哲学家 |
| `token_ring` | 3 | 令牌环 |
| `calc_dist` | 3 | 分布式计算器 |
| `mbox_3node` | 3 | 三节点互致问候 |
| `dist_sort_merge` | 2 | 分布式排序归并 (256×8 numbers) |
| `xfer_firehose` | 2 | 2000 条消息单向推送 |
| `xfer_async_rpc` | 2 | 256 异步 RPC |
| `xfer_dataplane` | 2 | 4KB 数据面直访 + ring 控制面 |

### 启动命令

```bash
# 终端 1：Node 0（必须先启）
make -C os fresh-shm run-node-0

# 终端 2：Node 1
make -C os run-node-1

# 终端 3：Node 2
make -C os run-node-2
```

## 与 Step 3/4 的关系

- **Step 3 (ring)**: ring 保留页 (cross-ring area) 由 Step 5 的 `reserve_cross_ring_pages()` 永久预留，`shm_test_integ` 验证不可分配性
- **Step 4 (hashring)**: 独立于 s4-hash 分支，单机多 CXL pool 场景。s5 面向多主机共享单池场景
- **三 branch 整合**: s3(ring) + s4(hashring) + s5(mailbox/GC) 代码独立分属三个分支，完整 CXL.mem 模拟需合并
