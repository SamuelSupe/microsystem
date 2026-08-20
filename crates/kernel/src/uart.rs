use core::fmt::{self, Write};
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_arch = "aarch64")]
static UART_BASE: AtomicUsize = AtomicUsize::new(0xffff_ff80_0900_0000);
#[cfg(target_arch = "riscv64")]
static UART_BASE: AtomicUsize = AtomicUsize::new(0xffff_ffc0_1000_0000);
static PRINT_NEXT: AtomicUsize = AtomicUsize::new(0);
static PRINT_SERVING: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "aarch64")]
const TX_OFFSET: usize = 0x00;
#[cfg(target_arch = "aarch64")]
const READY_OFFSET: usize = 0x18;
#[cfg(target_arch = "aarch64")]
const READY_MASK: u32 = 1 << 5;
#[cfg(target_arch = "riscv64")]
const TX_OFFSET: usize = 0x00;
#[cfg(target_arch = "riscv64")]
const READY_OFFSET: usize = 0x05;
#[cfg(target_arch = "riscv64")]
const READY_MASK: u32 = 1 << 5;

pub struct Uart;

impl Uart {
    fn read_reg(offset: usize) -> u32 {
        let base = UART_BASE.load(Ordering::Relaxed);
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::ptr::read_volatile((base + offset) as *const u8) as u32
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::ptr::read_volatile((base + offset) as *const u32)
        }
    }
    fn write_reg(offset: usize, value: u32) {
        let base = UART_BASE.load(Ordering::Relaxed);
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::ptr::write_volatile((base + offset) as *mut u8, value as u8)
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::ptr::write_volatile((base + offset) as *mut u32, value)
        }
    }

    pub fn put(byte: u8) {
        #[cfg(target_arch = "aarch64")]
        while Self::read_reg(READY_OFFSET) & READY_MASK != 0 {
            core::hint::spin_loop();
        }
        #[cfg(target_arch = "riscv64")]
        while Self::read_reg(READY_OFFSET) & READY_MASK == 0 {
            core::hint::spin_loop();
        }
        Self::write_reg(TX_OFFSET, byte as u32);
    }
}

pub fn set_physical_base(physical: usize) {
    if physical != 0 {
        UART_BASE.store(
            crate::arch::phys_to_virt(physical as u64),
            Ordering::Relaxed,
        );
    }
}

impl Write for Uart {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' {
                Self::put(b'\r');
            }
            Self::put(byte);
        }
        Ok(())
    }
}

pub fn print(args: fmt::Arguments<'_>) {
    let ticket = PRINT_NEXT.fetch_add(1, Ordering::Relaxed);
    while PRINT_SERVING.load(Ordering::Acquire) != ticket {
        core::hint::spin_loop();
    }
    let _ = Uart.write_fmt(args);
    PRINT_SERVING.store(ticket.wrapping_add(1), Ordering::Release);
}

#[macro_export]
macro_rules! kprint { ($($arg:tt)*) => { $crate::uart::print(format_args!($($arg)*)) }; }

#[macro_export]
macro_rules! kprintln {
    () => { $crate::kprint!("\n") };
    ($fmt:expr) => { $crate::kprint!(concat!($fmt, "\n")) };
    ($fmt:expr, $($arg:tt)*) => { $crate::kprint!(concat!($fmt, "\n"), $($arg)*) };
}
