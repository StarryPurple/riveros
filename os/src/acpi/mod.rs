// File written by agent.
use crate::config::MEMORY_END;
use alloc::vec::Vec;

/// ACPI RSDP (Root System Descriptor Pointer)
#[repr(C, packed)]
struct Rsdp {
    signature: [u8; 8],     // "RSD PTR "
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_addr: u32,         // RSDT physical address (legacy)
    length: u32,
    xsdt_addr: u64,         // XSDT physical address (ACPI 2.0+)
    ext_checksum: u8,
}

/// ACPI SDT Header (System Description Table)
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}

/// CXL Fixed Memory Window Structure
#[repr(C, packed)]
pub struct Cfmws {
    pub header: SdtHeader,   // type=0 (CXL Fixed Memory Window)
    pub reserved: [u8; 4],
    pub base_hpa: u64,       // CXL window base (Host Physical Address)
    pub window_size: u64,    // window size
    pub interleave_gran: u32, // interleave granularity (log2)
    pub interleave_ways: u16, // number of interleave ways
    pub interleave_id: u16,  // interleave set identifier
    pub host_bridge_count: u16, // number of host bridges
    pub host_bridge_list: [u32; 0], // flexible array
}

fn rsdp_search() -> Option<&'static Rsdp> {
    // Search for "RSD PTR " in physical memory
    // Start from RustSBI area (0x80000000) to MEMORY_END
    let signature: [u8; 8] = *b"RSD PTR ";
    let mut addr = 0x8000_0000usize;
    let end = MEMORY_END;
    while addr + 0x10 <= end {
        let ptr = addr as *const u8;
        let mut found = true;
        for i in 0..8 {
            if unsafe { ptr.add(i).read() } != signature[i] {
                found = false;
                break;
            }
        }
        if found {
            let rsdp = unsafe { &*(addr as *const Rsdp) };
            // Verify checksum for the first 20 bytes
            let sum: u8 = (0..20).map(|i| unsafe { *(addr as *const u8).add(i) }).fold(0u8, |a, b| a.wrapping_add(b));
            if sum == 0 {
                return Some(rsdp);
            }
        }
        addr += 0x10; // 16-byte aligned search
    }
    None
}

fn sdt_verify(addr: usize) -> bool {
    let hdr = unsafe { &*(addr as *const SdtHeader) };
    let len = hdr.length as usize;
    let sum: u8 = (0..len).map(|i| unsafe { *(addr as *const u8).add(i) }).fold(0u8, |a, b| a.wrapping_add(b));
    sum == 0
}

/// Find CEDT table and parse CFMWS entries
/// Returns list of (base, size) for each CXL memory window
fn parse_cedt(xsdt_addr: usize) -> &'static Cfmws {
    let xsdt = unsafe { &*(xsdt_addr as *const SdtHeader) };
    let entry_count = (xsdt.length as usize - core::mem::size_of::<SdtHeader>()) / core::mem::size_of::<usize>();
    let entries = unsafe { core::slice::from_raw_parts((xsdt_addr + core::mem::size_of::<SdtHeader>()) as *const usize, entry_count) };
    for &entry in entries {
        let hdr = unsafe { &*(entry as *const SdtHeader) };
        if &hdr.signature == b"CEDT" && sdt_verify(entry) {
            // parse the entry
        }
    }
}
