use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

const DMA_IOVA: u64 = 0x0010_0000;
const RUNTIME_IOVA_PAGES: u64 = 32;
const PROBE_IOVA: u64 = 0x0018_0000;

pub const GUI_IOVA: u64 = 0x0020_0000;
pub const GUI_QUEUE_IOVA: u64 = 0x0050_0000;
pub const KEYBOARD_QUEUE_IOVA: u64 = 0x0050_1000;
pub const TABLET_QUEUE_IOVA: u64 = 0x0050_2000;
pub const NET_RX_QUEUE_IOVA: u64 = 0x0050_3000;
pub const NET_TX_QUEUE_IOVA: u64 = 0x0050_4000;
pub const RNG_QUEUE_IOVA: u64 = 0x0050_5000;

const REG_CAP: usize = 0x08;
const REG_ECAP: usize = 0x10;
const REG_GCMD: usize = 0x18;
const REG_GSTS: usize = 0x1c;
const REG_RTADDR: usize = 0x20;
const REG_CCMD: usize = 0x28;
const REG_FSTS: usize = 0x34;

const GCMD_TE: u32 = 1 << 31;
const GCMD_SRTP: u32 = 1 << 30;
const GSTS_TES: u32 = 1 << 31;
const GSTS_RTPS: u32 = 1 << 30;
const CCMD_ICC: u64 = 1 << 63;
const CCMD_GLOBAL: u64 = 1 << 61;
const IOTLB_IVT: u64 = 1 << 63;
const IOTLB_GLOBAL: u64 = 1 << 60;
const IOTLB_DRAIN_READS: u64 = 1 << 49;
const IOTLB_DRAIN_WRITES: u64 = 1 << 48;

const PTE_READ: u64 = 1;
const PTE_WRITE: u64 = 1 << 1;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    Missing,
    Unsupported,
    StreamId,
    Timeout,
    Inactive,
    Address,
}

pub struct Domain {
    pub stream_id: u32,
    pub iova: u64,
    pub idr0: u32,
    pub idr5: u32,
    pub gerror: u32,
}

pub struct FaultEvent {
    pub event_type: u8,
    pub stream_id: u32,
    pub address: u64,
}

#[repr(C, align(4096))]
struct Tables {
    root: [u64; 512],
    context: [u64; 512],
    pml4: [u64; 512],
    pdpt: [u64; 512],
    pd: [u64; 512],
    low: [u64; 512],
    gui: [[u64; 512]; 2],
}

struct TableCell(UnsafeCell<Tables>);
unsafe impl Sync for TableCell {}

static TABLES: TableCell = TableCell(UnsafeCell::new(Tables {
    root: [0; 512],
    context: [0; 512],
    pml4: [0; 512],
    pdpt: [0; 512],
    pd: [0; 512],
    low: [0; 512],
    gui: [[0; 512]; 2],
}));
static ACTIVE_BASE: AtomicUsize = AtomicUsize::new(0);

pub fn create_domain(
    iommu_physical: usize,
    stream_id: u32,
    dma_physical: u64,
    data_physical: u64,
) -> Result<Domain, Error> {
    if iommu_physical == 0 {
        return Err(Error::Missing);
    }
    validate_stream(stream_id)?;
    if dma_physical & 0xfff != 0 || data_physical & 0xfff != 0 {
        return Err(Error::Address);
    }
    let base = crate::arch::phys_to_virt(iommu_physical as u64);
    let cap = read64(base + REG_CAP);
    let ecap = read64(base + REG_ECAP);
    if cap == 0 || cap == u64::MAX || cap & (1 << 10) == 0 {
        return Err(Error::Unsupported);
    }

    let tables = unsafe { &mut *TABLES.0.get() };
    unsafe { core::ptr::write_bytes(tables, 0, 1) };
    let root = physical(tables.root.as_mut_ptr())?;
    let context = physical(tables.context.as_mut_ptr())?;
    let pml4 = physical(tables.pml4.as_mut_ptr())?;
    let pdpt = physical(tables.pdpt.as_mut_ptr())?;
    let pd = physical(tables.pd.as_mut_ptr())?;
    let low = physical(tables.low.as_mut_ptr())?;
    let gui = [
        physical(tables.gui[0].as_mut_ptr())?,
        physical(tables.gui[1].as_mut_ptr())?,
    ];

    tables.root[0] = context | 1;
    tables.pml4[0] = table_descriptor(pdpt);
    tables.pdpt[0] = table_descriptor(pd);
    tables.pd[0] = table_descriptor(low);
    tables.pd[1] = table_descriptor(gui[0]);
    tables.pd[2] = table_descriptor(gui[1]);
    tables.low[((PROBE_IOVA >> 12) & 0x1ff) as usize] = page_descriptor(dma_physical);
    tables.low[(((PROBE_IOVA + 4096) >> 12) & 0x1ff) as usize] = page_descriptor(data_physical);
    install_context(tables, stream_id, pml4);
    crate::arch::dma_write_barrier();

    write32(base + REG_GCMD, 0);
    wait32(base + REG_GSTS, GSTS_TES, 0)?;
    write64(base + REG_RTADDR, root);
    write32(base + REG_GCMD, GCMD_SRTP);
    wait32(base + REG_GSTS, GSTS_RTPS, GSTS_RTPS)?;
    invalidate_context(base)?;
    invalidate_iotlb(base, ecap)?;
    write32(base + REG_GCMD, GCMD_TE);
    wait32(base + REG_GSTS, GSTS_TES, GSTS_TES)?;
    ACTIVE_BASE.store(base, Ordering::Release);

    Ok(Domain {
        stream_id,
        iova: DMA_IOVA,
        idr0: cap as u32,
        idr5: (cap >> 32) as u32,
        gerror: read32(base + REG_FSTS),
    })
}

pub fn map_gui_pages(physical_base: u64, pages: usize, queue_physical: u64) -> Result<(), Error> {
    if physical_base & 0xfff != 0 || queue_physical & 0xfff != 0 || pages == 0 || pages > 768 {
        return Err(Error::Address);
    }
    let tables = unsafe { &mut *TABLES.0.get() };
    for page in 0..pages {
        tables.gui[page / 512][page % 512] = page_descriptor(physical_base + page as u64 * 4096);
    }
    tables.gui[1][256] = page_descriptor(queue_physical);
    crate::arch::dma_write_barrier();
    invalidate_active()
}

pub fn map_gui_aux_page(iova: u64, physical_address: u64) -> Result<(), Error> {
    if !matches!(
        iova,
        KEYBOARD_QUEUE_IOVA
            | TABLET_QUEUE_IOVA
            | NET_RX_QUEUE_IOVA
            | NET_TX_QUEUE_IOVA
            | RNG_QUEUE_IOVA
    ) || physical_address & 0xfff != 0
    {
        return Err(Error::Address);
    }
    let table = ((iova - GUI_IOVA) / (512 * 4096)) as usize;
    let index = ((iova >> 12) & 0x1ff) as usize;
    if table >= 2 {
        return Err(Error::Address);
    }
    unsafe { &mut *TABLES.0.get() }.gui[table][index] = page_descriptor(physical_address);
    crate::arch::dma_write_barrier();
    invalidate_active()
}

pub fn attach_stream(stream_id: u32) -> Result<(), Error> {
    validate_stream(stream_id)?;
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if base == 0 {
        return Err(Error::Inactive);
    }
    let tables = unsafe { &mut *TABLES.0.get() };
    let pml4 = physical(tables.pml4.as_mut_ptr())?;
    install_context(tables, stream_id, pml4);
    crate::arch::dma_write_barrier();
    invalidate_context(base)?;
    invalidate_active()
}

pub fn take_fault() -> Option<FaultEvent> {
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if base == 0 || read32(base + REG_FSTS) & 0x3 == 0 {
        return None;
    }
    let cap = read64(base + REG_CAP);
    let fault_base = base + (((cap >> 24) & 0x3ff) as usize * 16);
    let records = (((cap >> 40) & 0xff) + 1) as usize;
    for record in 0..records {
        let address = fault_base + record * 16;
        let low = read64(address);
        let high = read64(address + 8);
        if high & (1 << 63) == 0 {
            continue;
        }
        let stream_id = (high & 0xffff) as u32;
        write64(address + 8, high | (1 << 63));
        write32(base + REG_FSTS, read32(base + REG_FSTS) & 0x7f);
        return Some(FaultEvent {
            event_type: 1,
            stream_id,
            address: low & !0xfff,
        });
    }
    None
}

impl Domain {
    pub const fn probe_iova(&self) -> u64 {
        PROBE_IOVA
    }

    pub fn take_fault(&self) -> Option<FaultEvent> {
        take_fault()
    }
}

pub fn map_runtime_page(iova: u64, physical_address: u64) -> Result<(), Error> {
    if !runtime_iova(iova) || physical_address & 0xfff != 0 {
        return Err(Error::Address);
    }
    update_runtime(iova, page_descriptor(physical_address))
}

pub fn unmap_runtime_page(iova: u64) -> Result<(), Error> {
    if !runtime_iova(iova) {
        return Err(Error::Address);
    }
    update_runtime(iova, 0)
}

fn install_context(tables: &mut Tables, stream_id: u32, pml4: u64) {
    let index = (stream_id & 0xff) as usize * 2;
    tables.context[index + 1] = 2 | (1 << 8);
    crate::arch::dma_write_barrier();
    tables.context[index] = pml4 | 1;
}

fn update_runtime(iova: u64, descriptor: u64) -> Result<(), Error> {
    if ACTIVE_BASE.load(Ordering::Acquire) == 0 {
        return Err(Error::Inactive);
    }
    unsafe { &mut *TABLES.0.get() }.low[((iova >> 12) & 0x1ff) as usize] = descriptor;
    crate::arch::dma_write_barrier();
    invalidate_active()
}

fn invalidate_active() -> Result<(), Error> {
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if base == 0 {
        return Err(Error::Inactive);
    }
    invalidate_iotlb(base, read64(base + REG_ECAP))
}

fn invalidate_context(base: usize) -> Result<(), Error> {
    write64(base + REG_CCMD, CCMD_ICC | CCMD_GLOBAL);
    wait64_clear(base + REG_CCMD, CCMD_ICC)
}

fn invalidate_iotlb(base: usize, ecap: u64) -> Result<(), Error> {
    let offset = ((ecap >> 8) & 0x3ff) as usize * 16 + 8;
    write64(
        base + offset,
        IOTLB_IVT | IOTLB_GLOBAL | IOTLB_DRAIN_READS | IOTLB_DRAIN_WRITES,
    );
    wait64_clear(base + offset, IOTLB_IVT)
}

fn wait32(address: usize, mask: u32, expected: u32) -> Result<(), Error> {
    for _ in 0..1_000_000 {
        if read32(address) & mask == expected {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(Error::Timeout)
}

fn wait64_clear(address: usize, mask: u64) -> Result<(), Error> {
    for _ in 0..1_000_000 {
        if read64(address) & mask == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(Error::Timeout)
}

fn validate_stream(stream_id: u32) -> Result<(), Error> {
    if stream_id >> 8 == 0 {
        Ok(())
    } else {
        Err(Error::StreamId)
    }
}

fn runtime_iova(iova: u64) -> bool {
    iova >= DMA_IOVA
        && iova < DMA_IOVA + RUNTIME_IOVA_PAGES * 4096
        && (iova - DMA_IOVA).is_multiple_of(4096)
}

const fn table_descriptor(physical: u64) -> u64 {
    (physical & !0xfff) | PTE_READ | PTE_WRITE
}

const fn page_descriptor(physical: u64) -> u64 {
    (physical & !0xfff) | PTE_READ | PTE_WRITE
}

fn physical(pointer: *mut u64) -> Result<u64, Error> {
    crate::arch::virt_to_phys(pointer as usize as u64)
        .filter(|physical| *physical != 0 && physical & 0xfff == 0)
        .ok_or(Error::Address)
}

fn read32(address: usize) -> u32 {
    unsafe { core::ptr::read_volatile(address as *const u32) }
}

fn read64(address: usize) -> u64 {
    unsafe { core::ptr::read_volatile(address as *const u64) }
}

fn write32(address: usize, value: u32) {
    unsafe { core::ptr::write_volatile(address as *mut u32, value) }
}

fn write64(address: usize, value: u64) {
    unsafe { core::ptr::write_volatile(address as *mut u64, value) }
}
