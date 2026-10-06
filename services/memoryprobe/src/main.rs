#![no_std]
#![no_main]

use core::panic::PanicInfo;
use microsystem_abi::{Status, virtual_memory as vm};

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let base =
        microsystem_user_rt::virtual_map(0, vm::PAGE_BUDGET as u64 * 4096, vm::READ | vm::WRITE)
            .unwrap_or_else(|_| microsystem_user_rt::exit(1));
    let result = microsystem_user_rt::virtual_commit(base, vm::PAGE_BUDGET as u64 * 4096);
    let mut stats = vm::StatsV1::default();
    microsystem_user_rt::virtual_stats(&mut stats).unwrap_or_else(|_| microsystem_user_rt::exit(2));
    match result {
        Err(Status::NoMemory) if stats.resident_pages == 0 && stats.allocation_failures != 0 => {
            let _ = microsystem_user_rt::debug_write_u64(
                b"[memoryprobe] NoMemory pid=",
                pid,
                b" resident=0 atomic=true\n",
            );
            microsystem_user_rt::exit(0)
        }
        Ok(()) if stats.resident_pages == vm::PAGE_BUDGET => {
            let _ = microsystem_user_rt::debug_write_u64(
                b"[memoryprobe] allocated pid=",
                pid,
                b" resident=4096\n",
            );
            loop {
                let _ = microsystem_user_rt::yield_now();
            }
        }
        _ => microsystem_user_rt::exit(3),
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(4)
}
