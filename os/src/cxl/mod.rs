pub mod hash_ring;

pub use hash_ring::{HashRing, HashValue};

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use crate::sync::UPIntrFreeCell;
use crate::mm::{PhysPageNum, VirtPageNum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CxlCardId(pub usize);

#[derive(Debug, Clone, Copy, Hash)]
pub enum CardStatus {
    Online,
    Offline,
    MigrateIn,
    MigrateOut,
}

pub struct CxlCardManager {
    pub cards: BTreeMap<CxlCardId, CardStatus>,
    pub hash_ring: HashRing,
    /// PPN -> CardId: the card of all CXL physical pages
    pub ppn2card: BTreeMap<PhysPageNum, CxlCardId>,
    /// PPN -> (VPN, PID): the virtual address and process of a CXL page
    pub ppn2vpn: BTreeMap<PhysPageNum, (VirtPageNum, usize)>,
}

impl CxlCardManager {
    pub fn new() -> Self {
        Self {
            cards: BTreeMap::new(),
            hash_ring: HashRing::new(),
            ppn2card: BTreeMap::new(),
            ppn2vpn: BTreeMap::new(),
        }
    }

    /// track a CXL page: ppn2card + ppn2vpn
    pub fn track_page(&mut self, ppn: PhysPageNum, card_id: CxlCardId, vpn: VirtPageNum, pid: usize) {
        self.ppn2card.insert(ppn, card_id);
        self.ppn2vpn.insert(ppn, (vpn, pid));
    }

    /// only track ppn2card (when VPN/PID information is unknown)
    pub fn track_card_ppn(&mut self, ppn: PhysPageNum, card_id: CxlCardId) {
        self.ppn2card.insert(ppn, card_id);
    }

    /// supplement tracking ppn2vpn
    pub fn track_page_vpn(&mut self, ppn: PhysPageNum, vpn: VirtPageNum, pid: usize) {
        self.ppn2vpn.insert(ppn, (vpn, pid));
    }

    /// remove all tracking of a CXL page
    #[allow(dead_code)]
    pub fn untrack_page(&mut self, ppn: PhysPageNum) {
        self.ppn2card.remove(&ppn);
        self.ppn2vpn.remove(&ppn);
    }

    pub fn get_ppn_card(&self, ppn: PhysPageNum) -> Option<CxlCardId> {
        self.ppn2card.get(&ppn).copied()
    }

    /// get all CXL pages and their VPN/PID on a card
    pub fn get_card_pages(&self, card_id: CxlCardId) -> Vec<(PhysPageNum, VirtPageNum, usize)> {
        self.ppn2card
            .iter()
            .filter(|(_, cid)| **cid == card_id)
            .filter_map(|(ppn, _)| {
                self.ppn2vpn
                    .get(ppn)
                    .map(|(vpn, pid)| (*ppn, *vpn, *pid))
            })
            .collect()
    }

    /// whether the card is on the ring
    pub fn has_card(&self, card_id: CxlCardId) -> bool {
        self.cards.contains_key(&card_id)
    }

    pub fn add_card(&mut self, card_id: CxlCardId) {
        self.cards
            .entry(card_id)
            .and_modify(|status| *status = CardStatus::MigrateIn)
            .or_insert(CardStatus::MigrateIn);
        self.hash_ring.add_card(card_id);
        *self.cards.get_mut(&card_id).unwrap() = CardStatus::Online;
    }
    pub fn remove_card(&mut self, card_id: CxlCardId) {
        *self.cards.get_mut(&card_id).unwrap() = CardStatus::MigrateOut;
        self.hash_ring.remove_card(card_id);
        *self.cards.get_mut(&card_id).unwrap() = CardStatus::Offline;
    }
}

lazy_static! {
  pub static ref CXL_CARD_MANAGER: UPIntrFreeCell<CxlCardManager> =
      unsafe { UPIntrFreeCell::new(CxlCardManager::new()) };
}