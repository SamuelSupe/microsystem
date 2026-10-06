use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const STREAM_ENTRIES: usize = 65536;
const STREAM_BUSES: usize = 8;
const MAX_DEVICE_DOMAINS: usize = 16;
const DMA_IOVA: u64 = 0x0010_0000;
const RUNTIME_IOVA_PAGES: u64 = 32;
const PROBE_IOVA: u64 = 0x0018_0000;
const CMDQ_LOG2_ENTRIES: u32 = 7;
const CMDQ_ENTRIES: u32 = 1 << CMDQ_LOG2_ENTRIES;
const CMDQ_INDEX_MASK: u32 = CMDQ_ENTRIES - 1;
const CMDQ_POSITION_MASK: u32 = (CMDQ_ENTRIES << 1) - 1;
const CMDQ_OP_TLBI_NH_VA: u64 = 0x12;
const CMDQ_OP_SYNC: u64 = 0x46;
const CMDQ_OP_CFGI_STE: u64 = 0x03;
pub const GUI_IOVA: u64 = 0x0020_0000;
pub const GUI_QUEUE_IOVA: u64 = 0x0050_0000;
pub const KEYBOARD_QUEUE_IOVA: u64 = 0x0050_1000;
pub const TABLET_QUEUE_IOVA: u64 = 0x0050_2000;
pub const NET_RX_QUEUE_IOVA: u64 = 0x0050_3000;
pub const NET_TX_QUEUE_IOVA: u64 = 0x0050_4000;
pub const RNG_QUEUE_IOVA: u64 = 0x0050_5000;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    Missing,
    Unsupported,
    StreamId,
    AckTimeout,
    Command,
    Inactive,
    Address,
    Busy,
    DomainLimit,
}

pub struct Domain {
    pub stream_id: u32,
    pub iova: u64,
    pub idr0: u32,
    pub idr5: u32,
    pub gerror: u32,
    base: usize,
    event: *mut u64,
}

pub struct FaultEvent {
    pub event_type: u8,
    pub stream_id: u32,
    pub address: u64,
}

#[repr(C, align(4096))]
#[derive(Clone, Copy)]
struct DomainTables {
    context: [u64; 512],
    level1: [u64; 512],
    level2: [u64; 512],
    level3: [u64; 512],
    gui_level3: [[u64; 512]; 2],
}

const EMPTY_DOMAIN: DomainTables = DomainTables {
    context: [0; 512],
    level1: [0; 512],
    level2: [0; 512],
    level3: [0; 512],
    gui_level3: [[0; 512]; 2],
};

#[repr(C, align(16384))]
#[derive(Clone, Copy)]
struct StreamPage([u64; 256 * 8]);

#[repr(C, align(16384))]
struct Tables {
    stream: [u64; 256],
    stream_pages: [StreamPage; STREAM_BUSES],
    event: [u64; 512],
    command: [u64; 512],
    streams: [u32; MAX_DEVICE_DOMAINS],
    domain_count: usize,
    primary_stream: u32,
    has_primary: bool,
    domains: [DomainTables; MAX_DEVICE_DOMAINS],
    stream_buses: [u16; STREAM_BUSES],
    stream_bus_count: usize,
}

struct TableStorage(UnsafeCell<Tables>);

unsafe impl Sync for TableStorage {}

static TABLES: TableStorage = TableStorage(UnsafeCell::new(Tables {
    stream: [0; 256],
    stream_pages: [StreamPage([0; 256 * 8]); STREAM_BUSES],
    stream_buses: [0; STREAM_BUSES],
    stream_bus_count: 0,
    event: [0; 512],
    command: [0; 512],
    streams: [0; MAX_DEVICE_DOMAINS],
    domain_count: 0,
    primary_stream: 0,
    has_primary: false,
    domains: [EMPTY_DOMAIN; MAX_DEVICE_DOMAINS],
}));
static ACTIVE_BASE: AtomicUsize = AtomicUsize::new(0);
static COMMAND_PRODUCER: AtomicU32 = AtomicU32::new(0);

pub fn create_domain(
    smmu_physical: usize,
    stream_id: u32,
    dma_physical: u64,
    data_physical: u64,
) -> Result<Domain, Error> {
    if smmu_physical == 0 {
        return Err(Error::Missing);
    }
    if stream_id as usize >= STREAM_ENTRIES {
        return Err(Error::StreamId);
    }
    if ACTIVE_BASE.load(Ordering::Acquire) != 0 {
        return Err(Error::Busy);
    }

    let base = crate::arch::phys_to_virt(smmu_physical as u64);
    let idr0 = read32(base);
    let idr1 = read32(base + 0x04);
    let idr5 = read32(base + 0x14);
    if idr0 & (1 << 1) == 0 || idr5 & (1 << 4) == 0 || (idr1 >> 21) & 0x1f < CMDQ_LOG2_ENTRIES
        || (idr0 >> 27) & 3 != 1 || idr1 & 0x3f < 16 {
        return Err(Error::Unsupported);
    }

    let tables = TABLES.0.get();
    unsafe { core::ptr::write_bytes(tables, 0, 1) };
    let stream = unsafe { core::ptr::addr_of_mut!((*tables).stream).cast::<u64>() };
    let event = unsafe { core::ptr::addr_of_mut!((*tables).event).cast::<u64>() };
    let command = unsafe { core::ptr::addr_of_mut!((*tables).command).cast::<u64>() };
    let stream_physical = physical(stream)?;
    let event_physical = physical(event)?;
    let command_physical = physical(command)?;
    let domain_index = ensure_domain(unsafe { &mut *tables }, stream_id)?;
    let domain = unsafe { &mut (*tables).domains[domain_index] };
    let context_physical = physical(domain.context.as_mut_ptr())?;
    domain.level3[((PROBE_IOVA >> 12) & 0x1ff) as usize] = (dma_physical & !0xfff) | 3 | (1 << 10);
    domain.level3[(((PROBE_IOVA + 4096) >> 12) & 0x1ff) as usize] =
        (data_physical & !0xfff) | 3 | (1 << 10);
    domain.level3[((DMA_IOVA >> 12) & 0x1ff) as usize] = 0;
    domain.level3[(((DMA_IOVA + 4096) >> 12) & 0x1ff) as usize] = 0;
    let entry = stream_entry(unsafe { &mut *tables }, stream_id)?;
    unsafe {
        core::ptr::write_volatile(
            entry,
            (context_physical & !0x3f) | 1 | (5 << 1),
        );
    }
    let tables = unsafe { &mut *tables };
    tables.primary_stream = stream_id;
    tables.has_primary = true;
    tables.streams[domain_index] = stream_id;
    crate::arch::dma_write_barrier();

    write32(base + 0x20, 0);
    wait_cr0(base, 0)?;

    write32(base + 0x28, 0x0d75);
    write64_split(base + 0x80, stream_physical);
    write32(base + 0x88, (1 << 16) | (8 << 6) | 16);
    write64_split(base + 0xa0, event_physical | 7);
    write32(base + 0xa8, 0);
    write32(base + 0xac, 0);
    write64_split(base + 0x90, command_physical | CMDQ_LOG2_ENTRIES as u64);
    write32(base + 0x98, 0);
    write32(base + 0x9c, 0);
    COMMAND_PRODUCER.store(0, Ordering::Release);
    write32(base + 0x20, 1 | 4 | 8);
    wait_cr0(base, 1 | 4 | 8)?;
    ACTIVE_BASE.store(base, Ordering::Release);

    Ok(Domain {
        stream_id,
        iova: DMA_IOVA,
        idr0,
        idr5,
        gerror: read32(base + 0x60),
        base,
        event,
    })
}

pub fn map_gui_pages(
    stream_id: u32,
    physical_base: u64,
    pages: usize,
    queue_physical: u64,
) -> Result<(), Error> {
    if physical_base & 0xfff != 0 || queue_physical & 0xfff != 0 || pages == 0 || pages > 768 {
        return Err(Error::Address);
    }
    if ACTIVE_BASE.load(Ordering::Acquire) == 0 {
        return Err(Error::Inactive);
    }
    let tables = unsafe { &mut *TABLES.0.get() };
    let domain_index = ensure_domain(tables, stream_id)?;
    let gui = &mut tables.domains[domain_index].gui_level3;
    for page in 0..pages {
        let table = page / 512;
        let index = page % 512;
        gui[table][index] = (physical_base + page as u64 * 4096) | 3 | (1 << 10);
    }
    gui[1][256] = queue_physical | 3 | (1 << 10);
    crate::arch::dma_write_barrier();
    Ok(())
}

pub fn map_gui_aux_page(stream_id: u32, iova: u64, physical: u64) -> Result<(), Error> {
    if !matches!(
        iova,
        KEYBOARD_QUEUE_IOVA
            | TABLET_QUEUE_IOVA
            | NET_RX_QUEUE_IOVA
            | NET_TX_QUEUE_IOVA
            | RNG_QUEUE_IOVA
    ) || physical & 0xfff != 0
    {
        return Err(Error::Address);
    }
    let table = ((iova - GUI_IOVA) / (512 * 4096)) as usize;
    let index = ((iova >> 12) & 0x1ff) as usize;
    if table >= 2 {
        return Err(Error::Address);
    }
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if base == 0 {
        return Err(Error::Inactive);
    }
    let tables = unsafe { &mut *TABLES.0.get() };
    let domain_index = ensure_domain(tables, stream_id)?;
    tables.domains[domain_index].gui_level3[table][index] = physical | 3 | (1 << 10);
    crate::arch::dma_write_barrier();
    invalidate_runtime_page(base, iova, domain_index)
}

pub fn attach_stream(stream_id: u32) -> Result<(), Error> {
    if stream_id as usize >= STREAM_ENTRIES {
        return Err(Error::StreamId);
    }
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if base == 0 {
        return Err(Error::Inactive);
    }
    let tables = unsafe { &mut *TABLES.0.get() };
    let domain_index = ensure_domain(tables, stream_id)?;
    let context = tables.domains[domain_index].context.as_mut_ptr();
    let entry = stream_entry(tables, stream_id)?;
    unsafe {
        core::ptr::write_volatile(
            entry,
            (physical(context)? & !0x3f) | 1 | (5 << 1),
        );
        crate::arch::dma_write_barrier();
    }
    submit_config_command(base, stream_id)
}

pub fn take_fault() -> Option<FaultEvent> {
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if base == 0 {
        return None;
    }
    let producer = read32(base + 0xa8);
    let consumer = read32(base + 0xac);
    if producer == consumer {
        return None;
    }
    crate::arch::dma_read_barrier();
    let event = unsafe { core::ptr::addr_of_mut!((*TABLES.0.get()).event).cast::<u64>() };
    let slot = ((consumer & 0x7f) as usize) * 4;
    let first = unsafe { core::ptr::read_volatile(event.add(slot)) };
    let address = unsafe { core::ptr::read_volatile(event.add(slot + 2)) };
    write32(base + 0xac, consumer.wrapping_add(1) & 0xff);
    Some(FaultEvent {
        event_type: first as u8,
        stream_id: (first >> 32) as u32,
        address,
    })
}

fn submit_config_command(base: usize, stream_id: u32) -> Result<(), Error> {
    submit_commands(base, [CMDQ_OP_CFGI_STE | ((stream_id as u64) << 32), 0])
}

impl Domain {
    pub const fn probe_iova(&self) -> u64 {
        PROBE_IOVA
    }

    pub fn take_fault(&self) -> Option<FaultEvent> {
        let producer = read32(self.base + 0xa8);
        let consumer = read32(self.base + 0xac);
        if producer == consumer {
            return None;
        }
        crate::arch::dma_read_barrier();
        let first = unsafe { core::ptr::read_volatile(self.event) };
        let address = unsafe { core::ptr::read_volatile(self.event.add(2)) };
        write32(self.base + 0xac, producer);
        Some(FaultEvent {
            event_type: first as u8,
            stream_id: (first >> 32) as u32,
            address,
        })
    }
}

pub fn map_runtime_page(iova: u64, physical_address: u64) -> Result<(), Error> {
    if !runtime_iova(iova) || physical_address & 0xfff != 0 {
        return Err(Error::Address);
    }
    update_runtime_page(iova, (physical_address & !0xfff) | 3 | (1 << 10))
}

pub fn unmap_runtime_page(iova: u64) -> Result<(), Error> {
    if !runtime_iova(iova) {
        return Err(Error::Address);
    }
    update_runtime_page(iova, 0)
}

fn runtime_iova(iova: u64) -> bool {
    iova >= DMA_IOVA
        && iova < DMA_IOVA + RUNTIME_IOVA_PAGES * 4096
        && (iova - DMA_IOVA).is_multiple_of(4096)
}

fn update_runtime_page(iova: u64, descriptor: u64) -> Result<(), Error> {
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if base == 0 {
        return Err(Error::Inactive);
    }
    let tables = unsafe { &mut *TABLES.0.get() };
    if !tables.has_primary {
        return Err(Error::Inactive);
    }
    let domain_index = find_domain(tables, tables.primary_stream).ok_or(Error::Inactive)?;
    tables.domains[domain_index].level3[((iova >> 12) & 0x1ff) as usize] = descriptor;
    crate::arch::dma_write_barrier();
    invalidate_runtime_page(base, iova, domain_index)
}

fn invalidate_runtime_page(base: usize, iova: u64, domain_index: usize) -> Result<(), Error> {
    let asid = domain_index as u64 + 1;
    submit_commands(
        base,
        [CMDQ_OP_TLBI_NH_VA | (asid << 48), (iova & !0xfff) | 1],
    )
}

fn submit_commands(base: usize, first: [u64; 2]) -> Result<(), Error> {
    let producer = COMMAND_PRODUCER.load(Ordering::Acquire);
    for _ in 0..1_000_000 {
        let consumer = read32(base + 0x9c);
        if consumer >> 24 & 0x7f != 0 {
            return Err(Error::Command);
        }
        if consumer & CMDQ_POSITION_MASK == producer {
            break;
        }
        core::hint::spin_loop();
    }
    if read32(base + 0x9c) & CMDQ_POSITION_MASK != producer {
        return Err(Error::AckTimeout);
    }
    let queue = unsafe { core::ptr::addr_of_mut!((*TABLES.0.get()).command).cast::<u64>() };
    let first_slot = (producer & CMDQ_INDEX_MASK) as usize * 2;
    let sync_slot = (producer.wrapping_add(1) & CMDQ_INDEX_MASK) as usize * 2;
    unsafe {
        core::ptr::write_volatile(queue.add(first_slot), first[0]);
        core::ptr::write_volatile(queue.add(first_slot + 1), first[1]);
        core::ptr::write_volatile(queue.add(sync_slot), CMDQ_OP_SYNC);
        core::ptr::write_volatile(queue.add(sync_slot + 1), 0);
        crate::arch::dma_write_barrier();
    }
    let next = producer.wrapping_add(2) & CMDQ_POSITION_MASK;
    write32(base + 0x98, next);
    for _ in 0..1_000_000 {
        let consumer = read32(base + 0x9c);
        if consumer >> 24 & 0x7f != 0 {
            return Err(Error::Command);
        }
        if consumer & CMDQ_POSITION_MASK == next {
            COMMAND_PRODUCER.store(next, Ordering::Release);
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(Error::AckTimeout)
}

fn find_domain(tables: &Tables, stream_id: u32) -> Option<usize> {
    tables.streams[..tables.domain_count]
        .iter()
        .position(|&registered| registered == stream_id)
}

fn stream_entry(tables: &mut Tables, stream_id: u32) -> Result<*mut u64, Error> {
    if stream_id as usize >= STREAM_ENTRIES { return Err(Error::StreamId); }
    let bus = (stream_id >> 8) as u16;
    let index = if let Some(index) = tables.stream_buses[..tables.stream_bus_count].iter().position(|registered| *registered == bus) { index }
    else {
        if tables.stream_bus_count == STREAM_BUSES { return Err(Error::DomainLimit); }
        let index = tables.stream_bus_count;
        let address = physical(tables.stream_pages[index].0.as_mut_ptr())?;
        tables.stream_buses[index] = bus; tables.stream_bus_count += 1;
        // Split=8: SPAN=9, with one 256-entry second-level table per PCI bus.
        tables.stream[bus as usize] = (address & !0x3f) | 9;
        crate::arch::dma_write_barrier();
        index
    };
    Ok(unsafe { tables.stream_pages[index].0.as_mut_ptr().add((stream_id as usize & 255) * 8) })
}

fn ensure_domain(tables: &mut Tables, stream_id: u32) -> Result<usize, Error> {
    if stream_id as usize >= STREAM_ENTRIES {
        return Err(Error::StreamId);
    }
    if let Some(index) = find_domain(tables, stream_id) {
        return Ok(index);
    }
    if tables.domain_count == MAX_DEVICE_DOMAINS {
        return Err(Error::DomainLimit);
    }
    let index = tables.domain_count;
    let domain = &mut tables.domains[index];
    let level1_physical = physical(domain.level1.as_mut_ptr())?;
    let level2_physical = physical(domain.level2.as_mut_ptr())?;
    let level3_physical = physical(domain.level3.as_mut_ptr())?;
    let gui_physical = [
        physical(domain.gui_level3[0].as_mut_ptr())?,
        physical(domain.gui_level3[1].as_mut_ptr())?,
    ];
    domain.context[0] = (25u32 | (1 << 30) | (1 << 31)) as u64
        | (((4u32 | (1 << 9) | (1 << 13) | (1 << 14)) as u64) << 32)
        | ((index as u64 + 1) << 48);
    domain.context[1] = level1_physical & !0xf;
    domain.level1[0] = level2_physical | 3;
    domain.level2[0] = level3_physical | 3;
    domain.level2[1] = gui_physical[0] | 3;
    domain.level2[2] = gui_physical[1] | 3;
    tables.streams[index] = stream_id;
    tables.domain_count += 1;
    Ok(index)
}

fn physical(pointer: *mut u64) -> Result<u64, Error> {
    crate::arch::virt_to_phys(pointer as usize as u64)
        .filter(|physical| *physical != 0 && physical & 0xfff == 0)
        .ok_or(Error::Address)
}

fn wait_cr0(base: usize, expected: u32) -> Result<(), Error> {
    for _ in 0..1_000_000 {
        if read32(base + 0x24) & 0xd == expected {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(Error::AckTimeout)
}

fn read32(address: usize) -> u32 {
    unsafe { core::ptr::read_volatile(address as *const u32) }
}

fn write32(address: usize, value: u32) {
    unsafe { core::ptr::write_volatile(address as *mut u32, value) }
}

fn write64_split(address: usize, value: u64) {
    write32(address, value as u32);
    write32(address + 4, (value >> 32) as u32);
}
