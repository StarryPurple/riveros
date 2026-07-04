use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::CxlCardId;

pub struct HashRing {
    ring: BTreeMap<u64, CxlCardId>,
    nodes: Vec<CxlCardId>,
}

impl HashRing {
    pub fn new() -> Self {
        Self { ring: BTreeMap::new(), nodes: Vec::new() }
    }
    pub fn add_node(&mut self, id: CxlCardId) {
        todo!()
    }
    pub fn remove_node(&mut self, id: CxlCardId) -> Option<CxlCardId> {
        todo!()
    }
    pub fn route(&self, key: u64) -> Option<&CxlCardId> {
        todo!()
    }
    pub fn node_count(&self) -> usize {
        todo!()
    }
}
