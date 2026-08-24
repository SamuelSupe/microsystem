#[cfg(target_arch = "aarch64")]
pub mod aarch64;

#[cfg(target_arch = "aarch64")]
pub use aarch64 as selected;

#[cfg(target_arch = "riscv64")]
pub use riscv64 as selected;

#[cfg(target_arch = "x86_64")]
pub use x86_64 as selected;

pub fn phys_to_virt(physical: u64) -> usize {
    selected::phys_to_virt(physical)
}

pub fn virt_to_phys(virtual_address: u64) -> Option<u64> {
    selected::virt_to_phys(virtual_address)
}

pub fn dma_barrier() {
    selected::dma_barrier()
}

pub fn dma_write_barrier() {
    selected::dma_write_barrier()
}

pub fn dma_read_barrier() {
    selected::dma_read_barrier()
}

pub fn clock_nanos() -> u64 {
    selected::clock_nanos()
}

pub fn is_syscall(cause: u64) -> bool {
    selected::is_syscall(cause)
}

pub fn flush_icache() {
    selected::flush_icache()
}

pub fn page_table_branch(physical: u64) -> u64 {
    selected::page_table_branch(physical)
}

pub fn page_table_entry(physical: u64, flags: u64) -> u64 {
    selected::page_table_entry(physical, flags)
}

pub fn page_table_physical(descriptor: u64) -> u64 {
    selected::page_table_physical(descriptor)
}

pub fn page_table_present(descriptor: u64) -> bool {
    selected::page_table_present(descriptor)
}

pub fn address_space_root(physical: u64, asid: u16) -> u64 {
    selected::address_space_root(physical, asid)
}

pub fn normal_page_flags() -> u64 {
    selected::normal_page_flags()
}

pub fn user_page_flags(writable: bool, executable: bool) -> u64 {
    selected::user_page_flags(writable, executable)
}

pub fn device_page_flags() -> u64 {
    selected::device_page_flags()
}

pub fn user_page_accessible(descriptor: u64, write: bool) -> bool {
    selected::user_page_accessible(descriptor, write)
}

pub fn kernel_high_half_entry() -> u64 {
    selected::kernel_high_half_entry()
}

pub fn kernel_mmio_entry() -> u64 {
    selected::kernel_mmio_entry()
}

pub fn kernel_pci_mmio_entry() -> u64 {
    selected::kernel_pci_mmio_entry()
}

#[cfg(target_arch = "riscv64")]
pub mod riscv64;

#[cfg(target_arch = "x86_64")]
pub mod x86_64;
