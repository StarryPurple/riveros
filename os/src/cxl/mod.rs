pub mod hash_ring;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CxlCardId(pub usize);

pub use hash_ring::{HashRing};