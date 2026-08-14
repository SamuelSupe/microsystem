use core::fmt::{self, Write};
use core::sync::atomic::{AtomicUsize, Ordering};

const KERNEL_OFFSET: usize = 0xffffff8000000000;
static PL011_BASE: AtomicUsize = AtomicUsize::new(KERNEL_OFFSET + 0x0900_0000);
static PRINT_NEXT: AtomicUsize = AtomicUsize::new(0);
static PRINT_SERVING: AtomicUsize = AtomicUsize::new(0);
const DR: usize = 0x00;
const FR: usize = 0x18;
const FR_TXFF: u32 = 1 << 5;

pub struct Uart;

impl Uart {
    fn read_reg(offset: usize) -> u32 {
        let base = PL011_BASE.load(Ordering::Relaxed);
        unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
    }
    fn write_reg(offset: usize, value: u32) {
        let base = PL011_BASE.load(Ordering::Relaxed);
        unsafe { core::ptr::write_volatile((base + offset) as *mut u32, value) }
    }

    pub fn put(byte: u8) {
        while Self::read_reg(FR) & FR_TXFF != 0 {
            core::hint::spin_loop();
        }
        Self::write_reg(DR, byte as u32);
    }
}

pub fn set_physical_base(physical: usize) {
    if physical != 0 {
        PL011_BASE.store(KERNEL_OFFSET + physical, Ordering::Relaxed);
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
