#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::Layout;
use core::panic::PanicInfo;
use microsystem_abi::{Status, virtual_memory as vm};

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let _ = microsystem_user_rt::debug_write_u64(
        b"[app] page-fault probe pid=",
        pid,
        b" entered EL0\n",
    );
    let leaked = verify_virtual_memory();
    if pid & 1 != 0 {
        microsystem_user_rt::virtual_protect(leaked, vm::READ).unwrap();
        unsafe { (leaked as *mut u8).write_volatile(2) };
    }
    unsafe { microsystem_user_rt::fault_probe(0x0060_0000) };
    microsystem_user_rt::exit(1)
}

fn verify_virtual_memory() -> u64 {
    let mut stats = vm::StatsV1::default();
    microsystem_user_rt::virtual_stats(&mut stats).unwrap();
    let base =
        microsystem_user_rt::virtual_map(0, vm::PAGE_BUDGET as u64 * 4096, vm::READ | vm::WRITE)
            .unwrap();
    assert_eq!(
        microsystem_user_rt::virtual_map(0, 4096, vm::READ),
        Err(Status::NoMemory)
    );
    microsystem_user_rt::virtual_stats(&mut stats).unwrap();
    assert_eq!(stats.resident_pages, 0);
    unsafe {
        assert_eq!((base as *const u8).read_volatile(), 0);
        (base as *mut u8).write_volatile(42);
        ((base + vm::PAGE_BUDGET as u64 * 4096 - 1) as *mut u8).write_volatile(17);
    }
    microsystem_user_rt::virtual_stats(&mut stats).unwrap();
    assert_eq!(stats.resident_pages, 2);
    microsystem_user_rt::virtual_resize(base, 8192).unwrap();
    microsystem_user_rt::virtual_stats(&mut stats).unwrap();
    assert_eq!(stats.resident_pages, 1);
    microsystem_user_rt::virtual_resize(base, 16384).unwrap();
    assert_eq!(unsafe { ((base + 12288) as *const u8).read_volatile() }, 0);
    microsystem_user_rt::virtual_protect(base, vm::READ).unwrap();
    assert_eq!(unsafe { (base as *const u8).read_volatile() }, 42);
    assert_eq!(
        microsystem_user_rt::virtual_protect(base, vm::WRITE),
        Err(Status::AccessDenied)
    );
    microsystem_user_rt::virtual_unmap(base).unwrap();
    microsystem_user_rt::virtual_stats(&mut stats).unwrap();
    assert_eq!(stats.resident_pages, 0);
    assert_eq!(stats.regions, 0);
    let small = Layout::from_size_align(64, 16).unwrap();
    let small_pointer = unsafe { alloc::alloc::alloc(small) };
    assert!(!small_pointer.is_null());
    unsafe {
        small_pointer.write_volatile(1);
        assert_eq!(small_pointer.read_volatile(), 1);
        alloc::alloc::dealloc(small_pointer, small);
    }
    microsystem_user_rt::virtual_stats(&mut stats).unwrap();
    let baseline = stats.resident_pages;
    let large = Layout::from_size_align(2 * 1024 * 1024, 4096).unwrap();
    let pointer = unsafe { alloc::alloc::alloc(large) };
    assert!(!pointer.is_null());
    unsafe {
        pointer.write_volatile(9);
        pointer.add(large.size() - 1).write_volatile(10);
        assert_eq!(pointer.read_volatile(), 9);
        alloc::alloc::dealloc(pointer, large);
    }
    microsystem_user_rt::virtual_stats(&mut stats).unwrap();
    assert_eq!(stats.resident_pages, baseline);
    assert_eq!(stats.regions, 0);
    let leaked = microsystem_user_rt::virtual_map(0, 4096, vm::READ | vm::WRITE).unwrap();
    unsafe { (leaked as *mut u8).write_volatile(1) };
    let _ = microsystem_user_rt::debug_write(
        b"[mm] anonymous reserve demand-zero grow shrink protect quota heap-growth=true\n",
    );
    leaked
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    struct Diagnostic;
    impl core::fmt::Write for Diagnostic {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            let _ = microsystem_user_rt::debug_write(text.as_bytes());
            Ok(())
        }
    }
    let _ = core::fmt::write(&mut Diagnostic, format_args!("[probe] {}\n", info));
    microsystem_user_rt::exit(2)
}
