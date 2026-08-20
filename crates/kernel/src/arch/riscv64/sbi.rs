use core::arch::asm;

pub const EXT_BASE: usize = 0x10;
pub const EXT_TIME: usize = 0x5449_4d45;
pub const EXT_IPI: usize = 0x0073_5049;
pub const EXT_RFENCE: usize = 0x5246_4e43;
pub const EXT_HSM: usize = 0x0048_534d;
pub const EXT_SRST: usize = 0x5352_5354;

pub const FID_SET_TIMER: usize = 0;
pub const FID_SEND_IPI: usize = 0;
pub const FID_REMOTE_SFENCE_VMA_ASID: usize = 2;
pub const FID_HART_START: usize = 0;
pub const FID_HART_STOP: usize = 1;
pub const FID_HART_GET_STATUS: usize = 2;
pub const FID_SYSTEM_RESET: usize = 0;

pub const RESET_TYPE_SHUTDOWN: usize = 0;
pub const RESET_TYPE_COLD_REBOOT: usize = 1;
pub const RESET_TYPE_WARM_REBOOT: usize = 2;
pub const RESET_REASON_NONE: usize = 0;
pub const RESET_REASON_SYSTEM_FAILURE: usize = 1;

pub const SBI_SUCCESS: isize = 0;
pub const SBI_ERR_FAILED: isize = -1;
pub const SBI_ERR_NOT_SUPPORTED: isize = -2;
pub const SBI_ERR_INVALID_PARAM: isize = -3;
pub const SBI_ERR_DENIED: isize = -4;
pub const SBI_ERR_INVALID_ADDRESS: isize = -5;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SbiRet {
    pub error: isize,
    pub value: usize,
}

impl SbiRet {
    pub const fn ok(value: usize) -> Self {
        Self {
            error: SBI_SUCCESS,
            value,
        }
    }

    pub const fn error(error: isize) -> Self {
        Self { error, value: 0 }
    }

    pub const fn is_ok(self) -> bool {
        self.error == SBI_SUCCESS
    }
}

#[inline(always)]
pub fn call(extension: usize, function: usize, arguments: [usize; 6]) -> SbiRet {
    let mut error = 0usize;
    let mut value = 0usize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") arguments[0] => error,
            inlateout("a1") arguments[1] => value,
            in("a2") arguments[2],
            in("a3") arguments[3],
            in("a4") arguments[4],
            in("a5") arguments[5],
            in("a6") function,
            in("a7") extension,
            options(nostack)
        );
    }
    SbiRet {
        error: error as isize,
        value,
    }
}

pub fn set_timer(time: u64) -> SbiRet {
    call(EXT_TIME, FID_SET_TIMER, [time as usize, 0, 0, 0, 0, 0])
}

pub fn send_ipi(mask: u64, mask_base: usize) -> SbiRet {
    call(
        EXT_IPI,
        FID_SEND_IPI,
        [mask as usize, mask_base, 0, 0, 0, 0],
    )
}

pub fn send_ipi_hart(hart: usize) -> SbiRet {
    if hart >= usize::BITS as usize {
        return SbiRet::error(SBI_ERR_INVALID_PARAM);
    }
    send_ipi((1usize << hart) as u64, 0)
}

pub fn send_ipi_all() -> SbiRet {
    // SBI v0.2+ reserves hart_mask_base == ULONG_MAX for all available
    // harts. A full bit mask would also select unavailable hart IDs and may
    // make the complete request fail with SBI_ERR_INVALID_PARAM.
    send_ipi(0, usize::MAX)
}

pub fn remote_sfence_vma_asid(
    mask: u64,
    mask_base: usize,
    start_address: usize,
    size: usize,
    asid: u16,
) -> SbiRet {
    call(
        EXT_RFENCE,
        FID_REMOTE_SFENCE_VMA_ASID,
        [
            mask as usize,
            mask_base,
            start_address,
            size,
            asid as usize,
            0,
        ],
    )
}

pub fn hart_start(hart: u64, start_address: u64, opaque: u64) -> SbiRet {
    call(
        EXT_HSM,
        FID_HART_START,
        [
            hart as usize,
            start_address as usize,
            opaque as usize,
            0,
            0,
            0,
        ],
    )
}

pub fn hart_stop() -> SbiRet {
    call(EXT_HSM, FID_HART_STOP, [0; 6])
}

pub fn hart_get_status(hart: u64) -> SbiRet {
    call(EXT_HSM, FID_HART_GET_STATUS, [hart as usize, 0, 0, 0, 0, 0])
}

pub fn system_reset(reset_type: usize, reason: usize) -> SbiRet {
    call(EXT_SRST, FID_SYSTEM_RESET, [reset_type, reason, 0, 0, 0, 0])
}
