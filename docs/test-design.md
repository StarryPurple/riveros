# RiverOS s5-hosts 测试套件设计文档

## 概述

本文档描述在 rCore-Tutorial-v3 基准代码之上新增的所有 CXL/SHM 相关测试程序，含单实例自动化套件 `usertests_s5` 和多实例手动测试。

新增测试命名特征字段：

- `shm_` — Step 5 SHM 分配/引用计数/GC
- `thread_cxl_` — 多线程 CXL 业务逻辑
- `xfer_` — 数据面/控制面并发通信
- `dist_` / `token_` / `calc_` / `mbox_` — 多节点协作业务

---

## 运行模式

| Makefile 目标 | 后端 | 节点数 | 输出 |
|---|---|---|---|
| `make fresh-shm run-node-0` | `/dev/shm/cxl.mm` (fresh) | 1 | `-nographic` |
| `make run-node-1` | `/dev/shm/cxl.mm` (shared) | 1 | `-nographic` |
| `make run-node-2` | `/dev/shm/cxl.mm` (shared) | 1 | `-nographic` |

> **必须最先启动 Node 0**，由它调用 `fresh-shm` 初始化 SHM。

---

## 自动化套件 `usertests_s5`

25 个单实例测试，shell 中运行 `usertests_s5` 一键全跑。

### SHM 页管理 — 6 个

| 测试 | 参数 | 验证点 |
|---|---|---|
| `shm_test_rc` | — | refcnt 1→2→1→0→re-alloc |
| `shm_test_all` | — | alloc/free + refcnt + 16 页批量 |
| `shm_bench` | 800 iters | alloc + free 吞吐 (ops/s, us/op) |
| `shm_test_gc` | — | VC GC 及时性 + 显式 gc_collect |
| `shm_test_integ` | 500 pages | 验证 cross-ring 保留页不可分配 |
| `shm_test_cross` | 线程模拟 | 跨线程 refcnt sharing + GC |

### SHM 压力 — 3 个

| 测试 | 参数 | 压力维度 |
|---|---|---|
| `shm_test_stress_concurrent` | 500 页 × 2 workers × 3 轮 | 并发 alloc/free |
| `shm_test_stress_gc` | 500 页 batch GC | 批量 pending → 一次 collect |
| `shm_test_stress_refcount` | 200 refs | 高引用计数 (200 → 0) |

### CXL mmap — 4 个

| 测试 | 验证点 |
|---|---|
| `cxl_info` | meminfo 统计查询 |
| `mmap_cxl` | cxl_mmap → 写读 → munmap → 复用 |
| `peterson_cxl` | CXL 内存上的 Peterson 互斥 |
| `threads_arg_cxl` | CXL 内存上的线程参数传递 |

### Ring/Channel — 3 个

| 测试 | 说明 |
|---|---|
| `ring_basic` | fork busy-poll round-trip |
| `ring_bench` | yield/hybrid/busy/pipe 对比 |
| `channel_bench_cxl` | Channel vs Pipe 延迟 (1000 msg) |

### 多线程业务 — 4 个

| 测试 | 线程 | 验证 |
|---|---|---|
| `thread_cxl_sum` | 4 workers | 2048 元素 Map-Reduce |
| `thread_cxl_maps` | 4 workers × 8 maps | 并发 mmap/munmap |
| `thread_cxl_matmul` | 4 workers | 64×64 矩阵乘法 |
| `thread_cxl_wal` | 1 writer + recovery | WAL checksum 崩溃恢复 |

---

## 多实例手动测试

### 双节点 (node-0 + node-1)

| 测试 | 参数 | 说明 |
|---|---|---|
| `shm_multi_server` / `_client` | 20 iters | client-server alloc/ref/free |
| `shm_multi_bulk_0` / `_1` | 128 pages | 批量页传输 |
| `shm_multi_roundtrip_0` / `_1` | 48 rounds | 多轮 alloc/send/ref/ack/free |
| `shm_multi_gc_sync_0` / `_1` | — | 跨节点 VC GC |
| `thread_cxl_token_0` / `_1` | 20 tokens | Token Bucket |
| `thread_cxl_phil_0` / `_1` | 3 meals | 跨 VM 哲学家 (backoff+retry) |
| `dist_sort_merge` | 8×256 numbers | 分布式排序归并管线 |
| `xfer_firehose` | 2000 burst | 单向推送 |
| `xfer_async_rpc` | 256 requests | 异步 RPC |
| `xfer_dataplane` | 4KB pattern | 数据面+控制面分离 |

### 三节点 (node-0 + node-1 + node-2)

| 测试 | 参数 | 说明 |
|---|---|---|
| `token_ring` | 12 passes | 令牌环流转 |
| `calc_dist` | 32 tasks × 2 workers | 分布式计算 |
| `mbox_3node` | — | 三节点互致问候 |

### 独立节点 (node-2)

| 测试 | 说明 |
|---|---|
| `shm_node_independent` | 独立 alloc/free，不参与 ring |

---

## 消息完整性

所有跨节点测试使用统一的消息校验：

```rust
msg_seal(&mut m, sender)   // byte 57 = sender, bytes 58-59 = u16 additive checksum
msg_verify(&m) -> Option<usize>  // 返回 sender 或 None
```

消息格式：`[0..57 payload][57 sender][58..59 checksum]`

---

## 测试执行方式总结

| 方式 | 命令 | 覆盖 |
|---|---|---|
| **自动化** | `usertests_s5` | 21 个单实例 SHM/CXL/Ring/多线程 |
| **手动单实例** | 直接输入测试名 | 同上，逐个运行 |
| **手动多实例** | 先 node-0，后 node-1(+node-2)，分别输入 | 跨 VM 全部测试 |
| **需手动终止** | `thread_cxl_barrier`(单节点), `cxl_ring_test`(无对端) | `pkill qemu` |

## 文件命名约定

| 前缀/中缀 | 类型 | 实例 |
|---|---|---|
| `shm_test_` | 单实例 SHM | `shm_test_rc`, `_all`, `_gc`, `_integ`, `_cross` |
| `shm_test_stress_` | SHM 压力 | `_concurrent`, `_gc`, `_refcount` |
| `shm_bench` | SHM 基准 | `shm_bench` |
| `shm_node` / `shm_multi_` | 多实例交互 | `shm_node0/1`, `shm_multi_server/client` |
| `shm_multi_*_0/_1` | 双节点对 | `shm_multi_bulk_0/1`, `_roundtrip_0/1`, `_gc_sync_0/1` |
| `thread_cxl_` | 单节点多线程 | `thread_cxl_sum`, `_maps`, `_matmul`, `_wal` |
| `thread_cxl_*_0/_1` | 跨 VM 多线程 | `thread_cxl_token_0/1`, `_phil_0/1` |
| `thread_cxl_barrier` | 自感知双角色 | `thread_cxl_barrier` (单二进制, get_instance_id) |
| `dist_sort_merge` | 自感知双角色 | `dist_sort_merge` (Master/Worker) |
| `token_ring` / `calc_dist` / `mbox_3node` | 多节点协作 | 三节点业务逻辑 |
| `xfer_` | 数据面/控制面 | `xfer_firehose`, `_async_rpc`, `_dataplane` |
| `ring_` / `cxl_ring_` / `channel_bench_cxl` | Ring 通信 | `ring_basic`, `ring_bench`, `channel_bench_cxl` |
| `mmap_cxl` / `*_cxl` | CXL mmap | `mmap_cxl`, `peterson_cxl`, `cxl_info` |
