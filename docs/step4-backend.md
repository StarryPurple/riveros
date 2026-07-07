# Step 4：分布式后端支持

## 一句话概括

这个模块给操作系统加了一个"智能内存管理员"。当系统有**多块 CXL 内存（内存池）**时，它能自动决定新的内存分配该从哪个池取，并在池增减时快速调整，保持各池负载均衡。

## 背景：为什么需要多内存池？

现实场景中，一台服务器可能插了多张 CXL 内存扩展卡：

```
CPU
├── 本地 DRAM（快速，但容量小）
├── CXL 内存池 A（较慢，容量 256GB）
├── CXL 内存池 B（较慢，容量 256GB）
└── CXL 内存池 C（较慢，容量 512GB）
```

不同池的延迟和容量不同。我们需要一个**调度算法**来决定：

- 新分配的内存该放哪个池？
- 如何让每个池的负载差不多？
- 如果拔掉一个池，里面的数据怎么搬走？
- 如果加一个新池，怎么把部分数据移过去？

## 解决方案：一致性哈希（Consistent Hashing）

### 传统哈希的问题

```
传统哈希：page_key % pool_count = pool_index

如果 pool_count 从 3 变成 4：
几乎所有的 key 都会映射到不同的池 → 需要搬移几乎所有数据！
```

### 一致性哈希的原理

一致性哈希把每个内存池映射到一个**虚拟圆环**上的多个点：

```
        池 A          池 A
          ⬤           ⬤
     ⬤               ⬤ 池 B
 池 C                   
     ⬤               ⬤ 池 B
          ⬤           ⬤
        池 C          池 C
```

当分配一个页面时，计算它的哈希值，在圆环上顺时针找到第一个池。

**池增减时的优势**：
- 增加一个池：只需要重新分配** 1/N 的页面**（N 是总池数）
- 减少一个池：只影响该池原本负责的 **1/N 页面**
- 对比传统哈希：池增减时需要重新分配**所有页面**

### 虚拟节点

每个物理内存池在圆环上映射为**多个虚拟节点**（例如 100 个）。这有两个好处：

1. **负载更均匀**：圆环上的点越多，分布越均匀
2. **支持不同权重的池**：大容量的池可以有更多虚拟节点

```
池 A（容量大）→ 200 个虚拟节点
池 B（容量小）→ 50 个虚拟节点
```

## 设计架构

```
用户态 / 内核分配请求
        │
        ▼
 ┌─────────────────┐
 │ 哈希调度器        │
 │ (Consistent Hash) │
 └─────┬───────┬────┘
       │       │
       ▼       ▼
 ┌─────────┐ ┌─────────┐
 │ CXL 池 A │ │ CXL 池 B │
 │ 管理模块  │ │ 管理模块  │
 └──────────┘ └──────────┘
```

### 数据结构

```rust
// 内存池描述
pub struct MemoryPool {
    pub id: usize,
    pub base_addr: PhysAddr,    // 物理起始地址
    pub size: usize,            // 总大小
    pub free_pages: u64,        // 剩余页数
    pub virtual_nodes: u32,     // 虚拟节点数量
    pub weight: u32,            // 权重（用于负载均衡）
}

// 一致性哈希环
pub struct ConsistentHashRing {
    ring: BTreeMap<u64, usize>,    // 哈希值 → 池 ID
    pools: Vec<MemoryPool>,        // 所有池
}
```

### 核心操作

```rust
// 分配页面：计算哈希 → 找到对应池 → 从该池分配
fn allocate_page(&self, key: u64) -> Option<PhysAddr> {
    let hash = self.hash(key);
    let pool_id = self.ring.range(hash..).next()  // 顺时针找第一个
        .or_else(|| self.ring.iter().next())      // 到末尾则回到开头
        .map(|(_, id)| *id)?;
    self.pools[pool_id].allocate_page()
}

// 注册新池：分配虚拟节点，插入环
fn register_pool(&mut self, pool: MemoryPool) {
    for i in 0..pool.virtual_nodes {
        let vnode_key = hash(pool.id, i);
        self.ring.insert(vnode_key, pool.id);
    }
    // 触发数据迁移（被这个新节点"抢走"的页面要搬过来）
    self.migrate_on_add(pool.id);
}

// 注销池：从环上移除所有虚拟节点
fn unregister_pool(&mut self, pool_id: usize) {
    self.ring.retain(|_, v| *v != pool_id);
    // 触发数据迁移（该池的页面要搬到新位置）
    self.migrate_on_remove(pool_id);
}
```

### 哈希函数

使用简单的非加密哈希（快就行）：

```rust
fn hash(key: u64) -> u64 {
    // 使用 xxhash 或简单自旋哈希
    key.wrapping_mul(0x9e3779b97f4a7c15)
        .rotate_left(31)
        .wrapping_mul(0xbf58476d1ce4e5b9)
}
```

## 系统调用接口

| 编号 | 函数 | 作用 |
|:---:|------|------|
| 7000 | `sys_pool_register(base, size)` | 注册一个 CXL 内存池 |
| 7001 | `sys_pool_unregister(id)` | 注销一个内存池 |
| 7002 | `sys_pool_alloc_page(key)` | 用一致性哈希分配一页 |
| 7003 | `sys_pool_info()` | 查询所有池的状态 |

## 与 Step 3 和 Step 5 的关系

```
         Step 5（跨主机共享）
              │
      一致性哈希调度 ← Step 4
              │
    ┌─────────┴─────────┐
    │                   │
  Step 3             Step 2
 (ring buffer)    (页面迁移)
```

- **Step 4 给 Step 3 提供 CXL 内存**：ring buffer 的数据页可以从一致性哈希选择的内存池分配
- **Step 4 给 Step 5 提供池管理**：跨主机共享时，各主机的本地 CXL 池通过一致性哈希协调

## 测试方案

### 单元测试

```rust
fn test_consistent_hash() {
    let mut ring = ConsistentHashRing::new();
    ring.register_pool(pool_a, 100);  // 100 个虚拟节点
    ring.register_pool(pool_b, 100);

    // 验证：分配 1000 次，两个池的分配比例接近 1:1
    let mut count_a = 0;
    let mut count_b = 0;
    for i in 0..1000u64 {
        let pool = ring.allocate_page(i).unwrap();
        match pool { 0 => count_a += 1, _ => count_b += 1 }
    }
    assert!(count_a > 400 && count_a < 600);  // ~50%
}
```

### 集成测试

```bash
# 注册两个模拟 CXL 池
pool_register(0x44000000, 64M)   # 池 A
pool_register(0x48000000, 128M)  # 池 B

# 分配 100 页
for i in 0..100 {
    let ppn = pool_alloc_page(i);
    // ppn 会根据一致性哈希在 A 和 B 之间分布
}

# 注销池 A，观察页面迁移
pool_unregister(0)
# 原本在 A 的页面数据自动迁移到 B
```

## 参考实现

### 一致性哈希的核心代码（伪代码）

```rust
pub fn find_pool(&self, key: &[u8]) -> Option<&MemoryPool> {
    if self.ring.is_empty() {
        return None;
    }
    let hash = self.do_hash(key);
    // 在 BTreeMap 中找到第一个 ≥ hash 的条目
    // BTreeMap 内部是红黑树，查找 O(log N)
    match self.ring.range(hash..).next() {
        Some((_, pool_id)) => Some(&self.pools[*pool_id]),
        None => {
            // 到末尾了，回到开头（环）
            let (_, pool_id) = self.ring.iter().next().unwrap();
            Some(&self.pools[*pool_id])
        }
    }
}
```

### 虚拟节点生成

```rust
pub fn add_pool(&mut self, pool: &MemoryPool) {
    for i in 0..pool.virtual_nodes {
        // 每个虚拟节点用 (pool_id, i) 作为键生成哈希
        let mut hasher = DefaultHasher::new();
        hasher.write_u64(pool.id as u64);
        hasher.write_u32(i);
        let vnode_hash = hasher.finish();
        self.ring.insert(vnode_hash, pool.id);
    }
}
```

## 预期性能

| 操作 | 时间复杂度 | 说明 |
|------|:--------:|------|
| 查找池 | O(log N) | N = 虚拟节点总数 |
| 注册池 | O(K log N) | K = 该池的虚拟节点数 |
| 注销池 | O(K log N) | 只需移除 K 个条目 |
| 数据迁移 | O(M) | M = 需要迁移的页面数 |

## 与现有框架的集成

当前系统已有 `TieredFrameAllocator`（支持 fast/slow 两层）。Step 4 可以：

1. **替换 slow tier**：不再用单一的 slow allocator，而是通过一致性哈希选择 CXL 池
2. **扩展 `MapType`**：增加 `MapType::Pooled`，走一致性哈希分配
3. **复用 `cxl::allocator`**：每个 CXL 池内部用 bakery-lock 保护的 freelist

## 快速入门

```rust
// 1. 注册两个 CXL 内存池
let pool_a = sys_pool_register(0x4400_0000, 64 * 1024 * 1024);
let pool_b = sys_pool_register(0x4800_0000, 128 * 1024 * 1024);

// 2. 用一致性哈希分配页面
let ppn1 = sys_pool_alloc_page(b"process1:page1");  // 自动选择池
let ppn2 = sys_pool_alloc_page(b"process2:page1");  // 自动选择池

// 3. 查询各池状态
let info = sys_pool_info();
// info.pools[0].free_pages, info.pools[1].free_pages, ...
```
