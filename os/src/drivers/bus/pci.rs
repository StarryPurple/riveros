/// Agent code generation used for this file.
use alloc::vec::Vec;

const ECAM_BASE: usize = 0x3000_0000;

#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    #[allow(unused)]
    pub prog_if: u8,
    #[allow(unused)]
    pub rev: u8,
    #[allow(unused)]
    pub bars: [Option<(u64, u64)>; 6],
}

fn ecam_addr(bus: u8, dev: u8, func: u8, offset: u8) -> usize {
    ECAM_BASE
        | ((bus as usize) << 20)
        | ((dev as usize) << 15)
        | ((func as usize) << 12)
        | (offset as usize)
}

fn read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    unsafe { (ecam_addr(bus, dev, func, offset) as *const u32).read_volatile() }
}

fn write32(bus: u8, dev: u8, func: u8, offset: u8, val: u32) {
    unsafe { (ecam_addr(bus, dev, func, offset) as *mut u32).write_volatile(val) }
}

impl PciDevice {
    fn try_new(bus: u8, dev: u8, func: u8) -> Option<Self> {
        let vendor_id = read32(bus, dev, func, 0) as u16;
        if vendor_id == 0xFFFF {
            return None;
        }
        let dev_vendor = read32(bus, dev, func, 0);
        let vendor_id = dev_vendor as u16;
        let device_id = (dev_vendor >> 16) as u16;
        let class_rev = read32(bus, dev, func, 0x08);
        let rev = class_rev as u8;
        let prog_if = (class_rev >> 8) as u8;
        let subclass = (class_rev >> 16) as u8;
        let class_code = (class_rev >> 24) as u8;

        let mut bars = [None; 6];
        for i in 0..6 {
            let off = 0x10 + (i as u8) * 4;
            let bar_raw = read32(bus, dev, func, off);
            if bar_raw == 0 {
                continue;
            }
            write32(bus, dev, func, off, 0xFFFF_FFFF);
            let bar_size_raw = read32(bus, dev, func, off);
            write32(bus, dev, func, off, bar_raw);

            if bar_size_raw == 0 {
                continue;
            }
            let (base, size) = if bar_raw & 1 == 0 {
                // MEM BAR (bit 0 = 0)
                let is_64bit = (bar_raw >> 1) & 0x3 == 0x2;
                if is_64bit && i < 5 {
                    let high = read32(bus, dev, func, off + 4);
                    let base_lo = bar_raw & !0xF;
                    let base = ((high as u64) << 32) | (base_lo as u64);
                    let size_lo = bar_size_raw & !0xF;
                    #[allow(unused)]
                    let size_high = read32(bus, dev, func, off + 4);
                    write32(bus, dev, func, off + 4, 0xFFFF_FFFF);
                    let size_high_mask = read32(bus, dev, func, off + 4);
                    write32(bus, dev, func, off + 4, high);
                    let size =
                        (!(((size_high_mask as u64) << 32) | (size_lo as u64))).wrapping_add(1);
                    bars[i] = Some((base, size));
                    bars[i + 1] = Some((0, 0));
                    continue;
                } else {
                    let base = (bar_raw & !0xF) as u64;
                    let size = (!(bar_size_raw & !0xF)).wrapping_add(1) as u64;
                    (base, size)
                }
            } else {
                // IO BAR (bit 0 = 1)
                let base = (bar_raw & !0x3) as u64;
                let size = (!(bar_size_raw & !0x3)).wrapping_add(1) as u64;
                (base, size)
            };
            bars[i] = Some((base, size));
        }

        Some(PciDevice {
            bus,
            dev,
            func,
            vendor_id,
            device_id,
            class_code,
            subclass,
            prog_if,
            rev,
            bars,
        })
    }
}

pub fn pci_scan() -> Vec<PciDevice> {
    let mut devices = Vec::new();
    for bus in 0..=12 {
        for dev in 0..=31 {
            for func in 0..=7 {
                if let Some(d) = PciDevice::try_new(bus, dev, func) {
                    devices.push(d);
                    if func == 0 {
                        let header_type = (read32(bus, dev, 0, 0x0C) >> 16) as u8;
                        if header_type & 0x80 == 0 {
                            break;
                        }
                    }
                }
            }
        }
    }
    devices
}

pub fn is_cxl_type3(device: &PciDevice) -> bool {
    device.class_code == 0x05 && device.subclass == 0x02
}
