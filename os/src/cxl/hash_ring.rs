use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use super::CxlCardId;

pub const VNODES_PER_CARD: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VnodeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HashValue(pub u64);

pub struct VnodeIdPool {
    pub next_id: u64,
    pub recycled: Vec<u64>,
}
impl VnodeIdPool {
    pub fn new() -> Self {
        Self {
            next_id: 0,
            recycled: Vec::new(),
        }
    }
    pub fn allocate(&mut self) -> VnodeId {
        if let Some(id) = self.recycled.pop() {
            VnodeId(id)
        } else {
            let id = self.next_id;
            self.next_id += 1;
            VnodeId(id)
        }
    }
    pub fn allocate_batch(&mut self, count: usize) -> Vec<VnodeId> {
        let mut ids = Vec::new();
        for _ in 0..count {
            ids.push(self.allocate());
        }
        ids
    }
    pub fn recycle(&mut self, id: VnodeId) {
        self.recycled.push(id.0);
    }
}

/// Compute the ring position for a virtual node.
fn vnode_ring_pos(card_id: CxlCardId, vnode_id: VnodeId) -> HashValue {
    let mut h = card_id.0 as u64;
    h = h.wrapping_mul(0x9e3779b97f4a7c15);
    h ^= vnode_id.0;
    HashValue(h.wrapping_mul(0x9e3779b97f4a7c15))
}

/// Hash a user-provided key to a ring position.
fn hash_key(key: u64) -> HashValue {
    let mut h = key;
    h = h.wrapping_mul(0x9e3779b97f4a7c15);
    h ^= h >> 31;
    HashValue(h.wrapping_mul(0x9e3779b97f4a7c15))
}

pub struct HashRing {
    /// Ring position (hashed) -> CXL card.
    vnode2card: BTreeMap<HashValue, CxlCardId>,
    /// For each card, the set of virtual node IDs it owns.
    card2vnodes: BTreeMap<CxlCardId, BTreeSet<VnodeId>>,
    /// VnodeId -> ring position, for removal.
    vnode_ring_pos: BTreeMap<VnodeId, HashValue>,
    /// Pool for recycling VnodeIds.
    vnode_id_pool: VnodeIdPool,
}

impl HashRing {
    pub fn new() -> Self {
        Self {
            vnode2card: BTreeMap::new(),
            card2vnodes: BTreeMap::new(),
            vnode_ring_pos: BTreeMap::new(),
            vnode_id_pool: VnodeIdPool::new(),
        }
    }

    pub fn add_card(&mut self, card_id: CxlCardId) {
        let vnodes = self.vnode_id_pool.allocate_batch(VNODES_PER_CARD);
        let vnode_set: BTreeSet<VnodeId> = vnodes.iter().copied().collect();
        for &vnode in &vnodes {
            let pos = vnode_ring_pos(card_id, vnode);
            self.vnode2card.insert(pos, card_id);
            self.vnode_ring_pos.insert(vnode, pos);
        }
        self.card2vnodes
            .entry(card_id)
            .and_modify(|existing| existing.extend(vnode_set.iter().copied()))
            .or_insert(vnode_set);
    }

    pub fn remove_card(&mut self, card_id: CxlCardId) -> Option<CxlCardId> {
        self.card2vnodes.remove(&card_id).map(|vnodes| {
            for vnode in vnodes {
                if let Some(pos) = self.vnode_ring_pos.remove(&vnode) {
                    self.vnode2card.remove(&pos);
                }
                self.vnode_id_pool.recycle(vnode);
            }
            card_id
        })
    }

    pub fn route(&self, key: u64) -> Option<&CxlCardId> {
        if self.vnode2card.is_empty() {
            return None;
        }
        let hash = hash_key(key);
        self.vnode2card
            .range(hash..)
            .next()
            .map(|(_, card)| card)
            .or_else(|| {
                self.vnode2card
                    .first_key_value()
                    .map(|(_, card)| card)
            })
    }

    pub fn node_count(&self) -> usize {
        self.card2vnodes.len()
    }
}
