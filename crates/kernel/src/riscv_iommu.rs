use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

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

const REG_CAPABILITIES: usize = 0x0000;
const REG_DDTP: usize = 0x0010;
const REG_CQB: usize = 0x0018;
const REG_CQH: usize = 0x0020;
const REG_CQT: usize = 0x0024;
const REG_FQB: usize = 0x0028;
const REG_FQH: usize = 0x0030;
const REG_FQT: usize = 0x0034;
const REG_CQCSR: usize = 0x0048;
const REG_FQCSR: usize = 0x004c;
const REG_IPSR: usize = 0x0054;

const CAP_VERSION_1_0: u64 = 0x10;
const CAP_SV39: u64 = 1 << 9;
const CAP_MSI_FLAT: u64 = 1 << 22;

const DDTP_MODE_MASK: u64 = 0xf;
const DDTP_BUSY: u64 = 1 << 4;
const DDTP_MODE_OFF: u64 = 0;
const DDTP_MODE_2LVL: u64 = 3;

const QUEUE_ENABLE: u32 = 1;
const QUEUE_MEM_FAULT: u32 = 1 << 8;
const QUEUE_OVERFLOW: u32 = 1 << 9;
const QUEUE_ACTIVE: u32 = 1 << 16;
const QUEUE_BUSY: u32 = 1 << 17;
const CQ_ERROR: u32 = QUEUE_MEM_FAULT | (1 << 9) | (1 << 10) | (1 << 11);
const FQ_ERROR: u32 = QUEUE_MEM_FAULT | QUEUE_OVERFLOW;
const IPSR_FIP: u32 = 1 << 1;

const COMMAND_ENTRIES: u32 = 256;
const COMMAND_MASK: u32 = COMMAND_ENTRIES - 1;
const COMMAND_LOG2_SIZE: u64 = 7;
const FAULT_ENTRIES: u32 = 128;
const FAULT_MASK: u32 = FAULT_ENTRIES - 1;
const FAULT_LOG2_SIZE: u64 = 6;
const DEVICE_CONTEXT_WORDS: usize = 8;
const CONTEXTS_PER_PAGE: usize = 4096 / (DEVICE_CONTEXT_WORDS * 8);
const DEVICE_CONTEXTS: usize = 8 * 256;
const CONTEXT_PAGES: usize = DEVICE_CONTEXTS / CONTEXTS_PER_PAGE;
const MAX_DEVICE_DOMAINS: usize = 16;

const CMD_IOTINVAL_VMA: u64 = 1;
const CMD_IOTINVAL_AV: u64 = 1 << 10;
const CMD_IOFENCE_C: u64 = 2 | (1 << 12) | (1 << 13);
const CMD_IODIR_INVAL_DDT: u64 = 3;
const CMD_IODIR_DV: u64 = 1 << 33;

const PTE_V: u64 = 1;
const PTE_RW_USER_AD: u64 = 0xd7;
const IOSATP_MODE_SV39: u64 = 8 << 60;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    Missing,
    Unsupported,
    StreamId,
    Queue,
    Timeout,
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
}

pub struct FaultEvent {
    pub event_type: u8,
    pub stream_id: u32,
    pub address: u64,
}

pub struct FaultDiagnostics {
    pub head: u32,
    pub tail: u32,
    pub csr: u32,
    pub ipsr: u32,
    pub ddtp: u64,
}

pub struct StreamDiagnostics {
    pub tc: u64,
    pub fsc: u64,
    pub pte: u64,
}

#[repr(C, align(4096))]
#[derive(Clone, Copy)]
struct DomainTables {
    level1: [u64; 512],
    level2: [u64; 512],
    level3: [u64; 512],
    gui_level3: [[u64; 512]; 2],
}

const EMPTY_DOMAIN: DomainTables = DomainTables {
    level1: [0; 512],
    level2: [0; 512],
    level3: [0; 512],
    gui_level3: [[0; 512]; 2],
};

#[repr(C, align(4096))]
#[derive(Clone, Copy)]
struct DevicePage([u64; 512]);

#[repr(C, align(4096))]
struct Tables {
    device: [u64; 512],
    device_pages: [DevicePage; CONTEXT_PAGES],
    command: [u64; 512],
    fault: [u64; 512],
    streams: [u32; MAX_DEVICE_DOMAINS],
    domain_count: usize,
    primary_stream: u32,
    has_primary: bool,
    domains: [DomainTables; MAX_DEVICE_DOMAINS],
}

struct TableStorage(UnsafeCell<Tables>);

unsafe impl Sync for TableStorage {}

static TABLES: TableStorage = TableStorage(UnsafeCell::new(Tables {
    device: [0; 512],
    device_pages: [DevicePage([0; 512]); CONTEXT_PAGES],
    command: [0; 512],
    fault: [0; 512],
    streams: [0; MAX_DEVICE_DOMAINS],
    domain_count: 0,
    primary_stream: 0,
    has_primary: false,
    domains: [EMPTY_DOMAIN; MAX_DEVICE_DOMAINS],
}));
static ACTIVE_BASE: AtomicUsize = AtomicUsize::new(0);
static COMMAND_PRODUCER: AtomicU32 = AtomicU32::new(0);

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
    if ACTIVE_BASE.load(Ordering::Acquire) != 0 {
        return Err(Error::Busy);
    }

    let base = crate::arch::phys_to_virt(iommu_physical as u64);
    let capabilities = read64(base + REG_CAPABILITIES);
    if capabilities & 0xff != CAP_VERSION_1_0
        || capabilities & CAP_SV39 == 0
        || capabilities & CAP_MSI_FLAT == 0
    {
        return Err(Error::Unsupported);
    }

    let tables = TABLES.0.get();
    unsafe { core::ptr::write_bytes(tables, 0, 1) };
    let device = unsafe { core::ptr::addr_of_mut!((*tables).device).cast::<u64>() };
    let command = unsafe { core::ptr::addr_of_mut!((*tables).command).cast::<u64>() };
    let fault = unsafe { core::ptr::addr_of_mut!((*tables).fault).cast::<u64>() };
    let device_physical = physical(device)?;
    let command_physical = physical(command)?;
    let fault_physical = physical(fault)?;
    let tables_ref = unsafe { &mut *tables };
    let domain_index = ensure_domain(tables_ref, stream_id)?;
    let domain = &mut tables_ref.domains[domain_index];
    let level1_physical = physical(domain.level1.as_mut_ptr())?;
    let level2_physical = physical(domain.level2.as_mut_ptr())?;
    let level3_physical = physical(domain.level3.as_mut_ptr())?;
    let gui_physical = [
        physical(domain.gui_level3[0].as_mut_ptr())?,
        physical(domain.gui_level3[1].as_mut_ptr())?,
    ];
    domain.level1[0] = branch_descriptor(level2_physical);
    domain.level2[0] = branch_descriptor(level3_physical);
    domain.level2[1] = branch_descriptor(gui_physical[0]);
    domain.level2[2] = branch_descriptor(gui_physical[1]);
    domain.level3[((PROBE_IOVA >> 12) & 0x1ff) as usize] = page_descriptor(dma_physical);
    domain.level3[(((PROBE_IOVA + 4096) >> 12) & 0x1ff) as usize] = page_descriptor(data_physical);
    tables_ref.primary_stream = stream_id;
    tables_ref.has_primary = true;
    tables_ref.streams[domain_index] = stream_id;
    crate::arch::dma_write_barrier();
    install_context(stream_id, level1_physical, false)?;
    crate::arch::dma_write_barrier();

    write64(base + REG_DDTP, DDTP_MODE_OFF);
    wait_ddtp(base, DDTP_MODE_OFF)?;
    write32(base + REG_CQCSR, 0);
    write32(base + REG_FQCSR, 0);

    let command_base = queue_base(command_physical, COMMAND_LOG2_SIZE);
    let fault_base = queue_base(fault_physical, FAULT_LOG2_SIZE);
    write64(base + REG_CQB, command_base);
    write64(base + REG_FQB, fault_base);
    if read64(base + REG_CQB) != command_base || read64(base + REG_FQB) != fault_base {
        return Err(Error::Queue);
    }
    write32(base + REG_CQT, 0);
    write32(base + REG_FQH, 0);
    COMMAND_PRODUCER.store(0, Ordering::Release);
    write32(base + REG_CQCSR, QUEUE_ENABLE | CQ_ERROR);
    wait_queue(base, REG_CQCSR, CQ_ERROR)?;
    write32(base + REG_FQCSR, QUEUE_ENABLE | FQ_ERROR);
    wait_queue(base, REG_FQCSR, FQ_ERROR)?;

    write64(base + REG_DDTP, (device_physical >> 2) | DDTP_MODE_2LVL);
    wait_ddtp(base, DDTP_MODE_2LVL)?;
    ACTIVE_BASE.store(base, Ordering::Release);

    Ok(Domain {
        stream_id,
        iova: DMA_IOVA,
        idr0: capabilities as u32,
        idr5: (capabilities >> 32) as u32,
        gerror: read32(base + REG_FQCSR) & FQ_ERROR,
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
        gui[table][index] = page_descriptor(physical_base + page as u64 * 4096);
    }
    gui[1][256] = page_descriptor(queue_physical);
    crate::arch::dma_write_barrier();
    invalidate_all()
}

pub fn map_gui_aux_page(stream_id: u32, iova: u64, physical_address: u64) -> Result<(), Error> {
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
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if base == 0 {
        return Err(Error::Inactive);
    }
    let table = ((iova - GUI_IOVA) / (512 * 4096)) as usize;
    let index = ((iova >> 12) & 0x1ff) as usize;
    if table >= 2 {
        return Err(Error::Address);
    }
    let tables = unsafe { &mut *TABLES.0.get() };
    let domain_index = ensure_domain(tables, stream_id)?;
    tables.domains[domain_index].gui_level3[table][index] = page_descriptor(physical_address);
    crate::arch::dma_write_barrier();
    invalidate_page(base, iova)
}

pub fn attach_stream(stream_id: u32) -> Result<(), Error> {
    validate_stream(stream_id)?;
    if ACTIVE_BASE.load(Ordering::Acquire) == 0 {
        return Err(Error::Inactive);
    }
    let tables = unsafe { &mut *TABLES.0.get() };
    let domain_index = ensure_domain(tables, stream_id)?;
    let level1 = physical(tables.domains[domain_index].level1.as_mut_ptr())?;
    install_context(stream_id, level1, true)
}

pub fn take_fault() -> Option<FaultEvent> {
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if base == 0 {
        return None;
    }
    let csr = read32(base + REG_FQCSR);
    if csr & QUEUE_MEM_FAULT != 0 {
        return None;
    }
    let head = read32(base + REG_FQH) & FAULT_MASK;
    let tail = read32(base + REG_FQT) & FAULT_MASK;
    if head == tail {
        return None;
    }
    crate::arch::dma_read_barrier();
    let fault = unsafe {
        core::ptr::addr_of_mut!((*TABLES.0.get()).fault)
            .cast::<u64>()
            .add(head as usize * 4)
    };
    let header = unsafe { core::ptr::read_volatile(fault) };
    let address = unsafe { core::ptr::read_volatile(fault.add(2)) };
    write32(base + REG_FQH, head.wrapping_add(1) & FAULT_MASK);
    if csr & QUEUE_OVERFLOW != 0 {
        write32(base + REG_FQCSR, QUEUE_ENABLE | QUEUE_OVERFLOW);
    }
    write32(base + REG_IPSR, IPSR_FIP);
    Some(FaultEvent {
        event_type: (header & 0xff) as u8,
        stream_id: (header >> 40) as u32,
        address,
    })
}

impl Domain {
    pub const fn probe_iova(&self) -> u64 {
        PROBE_IOVA
    }

    pub fn take_fault(&self) -> Option<FaultEvent> {
        take_fault()
    }

    pub fn fault_diagnostics(&self) -> FaultDiagnostics {
        let base = ACTIVE_BASE.load(Ordering::Acquire);
        FaultDiagnostics {
            head: read32(base + REG_FQH),
            tail: read32(base + REG_FQT),
            csr: read32(base + REG_FQCSR),
            ipsr: read32(base + REG_IPSR),
            ddtp: read64(base + REG_DDTP),
        }
    }

    pub fn drain_faults(&self) -> usize {
        let mut drained = 0;
        while drained < FAULT_ENTRIES as usize && take_fault().is_some() {
            drained += 1;
        }
        drained
    }
}

pub fn map_runtime_page(iova: u64, physical_address: u64) -> Result<(), Error> {
    if !runtime_iova(iova) || physical_address & 0xfff != 0 {
        return Err(Error::Address);
    }
    update_runtime_page(iova, page_descriptor(physical_address))
}

pub fn unmap_runtime_page(iova: u64) -> Result<(), Error> {
    if !runtime_iova(iova) {
        return Err(Error::Address);
    }
    update_runtime_page(iova, 0)
}

pub fn stream_diagnostics(stream_id: u32, iova: u64) -> StreamDiagnostics {
    if iova < GUI_IOVA {
        return StreamDiagnostics {
            tc: 0,
            fsc: 0,
            pte: 0,
        };
    }
    let table = ((iova - GUI_IOVA) / (512 * 4096)) as usize;
    let index = ((iova >> 12) & 0x1ff) as usize;
    if validate_stream(stream_id).is_err() || table >= 2 {
        return StreamDiagnostics {
            tc: 0,
            fsc: 0,
            pte: 0,
        };
    }
    let tables = unsafe { &*TABLES.0.get() };
    let context = tables.device_pages[stream_id as usize / CONTEXTS_PER_PAGE]
        .0
        .as_ptr()
        .wrapping_add((stream_id as usize % CONTEXTS_PER_PAGE) * DEVICE_CONTEXT_WORDS);
    let Some(domain_index) = find_domain(tables, stream_id) else {
        return StreamDiagnostics {
            tc: 0,
            fsc: 0,
            pte: 0,
        };
    };
    StreamDiagnostics {
        tc: unsafe { core::ptr::read_volatile(context) },
        fsc: unsafe { core::ptr::read_volatile(context.add(3)) },
        pte: tables.domains[domain_index].gui_level3[table][index],
    }
}

fn install_context(stream_id: u32, level1_physical: u64, invalidate: bool) -> Result<(), Error> {
    validate_stream(stream_id)?;
    let tables = unsafe { &mut *TABLES.0.get() };
    let page = stream_id as usize / CONTEXTS_PER_PAGE;
    let leaf = tables.device_pages[page].0.as_mut_ptr();
    tables.device[page] = (physical(leaf)? >> 2) | 1;
    let context =
        unsafe { leaf.add((stream_id as usize % CONTEXTS_PER_PAGE) * DEVICE_CONTEXT_WORDS) };
    unsafe {
        core::ptr::write_volatile(context, 0);
        for word in 1..DEVICE_CONTEXT_WORDS {
            core::ptr::write_volatile(context.add(word), 0);
        }
        // Stage-one IOTLB entries are keyed by PSCID. Independent device page
        // tables reuse queue IOVAs, so sharing PSCID zero aliases their DMA.
        core::ptr::write_volatile(context.add(2), (u64::from(stream_id) + 1) << 12);
        core::ptr::write_volatile(context.add(3), IOSATP_MODE_SV39 | (level1_physical >> 12));
        crate::arch::dma_write_barrier();
        core::ptr::write_volatile(context, 1);
        crate::arch::dma_write_barrier();
    }
    if invalidate {
        submit_commands(&[
            (
                CMD_IODIR_INVAL_DDT | CMD_IODIR_DV | ((stream_id as u64) << 40),
                0,
            ),
            (CMD_IOTINVAL_VMA, 0),
            (CMD_IOFENCE_C, 0),
        ])?;
    }
    Ok(())
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
    invalidate_page(base, iova)
}

fn invalidate_page(base: usize, iova: u64) -> Result<(), Error> {
    if base == 0 {
        return Err(Error::Inactive);
    }
    submit_commands(&[
        (CMD_IOTINVAL_VMA | CMD_IOTINVAL_AV, (iova & !0xfff) >> 2),
        (CMD_IOFENCE_C, 0),
    ])
}

fn invalidate_all() -> Result<(), Error> {
    submit_commands(&[(CMD_IOTINVAL_VMA, 0), (CMD_IOFENCE_C, 0)])
}

fn submit_commands(commands: &[(u64, u64)]) -> Result<(), Error> {
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if base == 0 || commands.is_empty() || commands.len() >= COMMAND_ENTRIES as usize {
        return Err(Error::Inactive);
    }
    let producer = COMMAND_PRODUCER.load(Ordering::Acquire);
    let expected_head = producer & COMMAND_MASK;
    for _ in 0..1_000_000 {
        let csr = read32(base + REG_CQCSR);
        if csr & CQ_ERROR != 0 {
            return Err(Error::Queue);
        }
        if read32(base + REG_CQH) & COMMAND_MASK == expected_head {
            break;
        }
        core::hint::spin_loop();
    }
    if read32(base + REG_CQH) & COMMAND_MASK != expected_head {
        return Err(Error::Timeout);
    }

    let queue = unsafe { core::ptr::addr_of_mut!((*TABLES.0.get()).command).cast::<u64>() };
    for (offset, (first, second)) in commands.iter().enumerate() {
        let index = producer.wrapping_add(offset as u32) & COMMAND_MASK;
        unsafe {
            core::ptr::write_volatile(queue.add(index as usize * 2), *first);
            core::ptr::write_volatile(queue.add(index as usize * 2 + 1), *second);
        }
    }
    crate::arch::dma_write_barrier();
    let next = producer.wrapping_add(commands.len() as u32);
    write32(base + REG_CQT, next & COMMAND_MASK);
    for _ in 0..1_000_000 {
        let csr = read32(base + REG_CQCSR);
        if csr & CQ_ERROR != 0 {
            return Err(Error::Queue);
        }
        if read32(base + REG_CQH) & COMMAND_MASK == next & COMMAND_MASK {
            COMMAND_PRODUCER.store(next, Ordering::Release);
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(Error::Timeout)
}

fn validate_stream(stream_id: u32) -> Result<(), Error> {
    if stream_id as usize >= DEVICE_CONTEXTS {
        Err(Error::StreamId)
    } else {
        Ok(())
    }
}

fn find_domain(tables: &Tables, stream_id: u32) -> Option<usize> {
    tables.streams[..tables.domain_count]
        .iter()
        .position(|&registered| registered == stream_id)
}

fn ensure_domain(tables: &mut Tables, stream_id: u32) -> Result<usize, Error> {
    validate_stream(stream_id)?;
    if let Some(index) = find_domain(tables, stream_id) {
        return Ok(index);
    }
    if tables.domain_count == MAX_DEVICE_DOMAINS {
        return Err(Error::DomainLimit);
    }
    let index = tables.domain_count;
    let domain = &mut tables.domains[index];
    let level2 = physical(domain.level2.as_mut_ptr())?;
    let level3 = physical(domain.level3.as_mut_ptr())?;
    let gui = [
        physical(domain.gui_level3[0].as_mut_ptr())?,
        physical(domain.gui_level3[1].as_mut_ptr())?,
    ];
    domain.level1[0] = branch_descriptor(level2);
    domain.level2[0] = branch_descriptor(level3);
    domain.level2[1] = branch_descriptor(gui[0]);
    domain.level2[2] = branch_descriptor(gui[1]);
    tables.streams[index] = stream_id;
    tables.domain_count += 1;
    Ok(index)
}

fn runtime_iova(iova: u64) -> bool {
    iova >= DMA_IOVA
        && iova < DMA_IOVA + RUNTIME_IOVA_PAGES * 4096
        && (iova - DMA_IOVA).is_multiple_of(4096)
}

const fn branch_descriptor(physical_address: u64) -> u64 {
    (physical_address & !0xfff) >> 2 | PTE_V
}

const fn page_descriptor(physical_address: u64) -> u64 {
    (physical_address & !0xfff) >> 2 | PTE_RW_USER_AD
}

const fn queue_base(physical_address: u64, log2_size: u64) -> u64 {
    (physical_address & !0xfff) >> 2 | log2_size
}

fn physical(pointer: *mut u64) -> Result<u64, Error> {
    crate::arch::virt_to_phys(pointer as usize as u64)
        .filter(|physical| *physical != 0 && physical & 0xfff == 0)
        .ok_or(Error::Address)
}

fn wait_ddtp(base: usize, expected_mode: u64) -> Result<(), Error> {
    for _ in 0..1_000_000 {
        let value = read64(base + REG_DDTP);
        if value & DDTP_BUSY == 0 {
            return if value & DDTP_MODE_MASK == expected_mode {
                Ok(())
            } else {
                Err(Error::Unsupported)
            };
        }
        core::hint::spin_loop();
    }
    Err(Error::Timeout)
}

fn wait_queue(base: usize, register: usize, errors: u32) -> Result<(), Error> {
    for _ in 0..1_000_000 {
        let value = read32(base + register);
        if value & errors != 0 {
            return Err(Error::Queue);
        }
        if value & QUEUE_BUSY == 0 && value & QUEUE_ACTIVE != 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(Error::Timeout)
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
