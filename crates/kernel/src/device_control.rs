use core::cell::UnsafeCell;

use crate::dtb::PlatformInfo;
use crate::kprintln;
use crate::pci::{self, UserTransportGrant};
use crate::{arch, smmu};
use microsystem_abi::Status;

struct PlatformCell(UnsafeCell<Option<PlatformInfo>>);

unsafe impl Sync for PlatformCell {}

static PLATFORM: PlatformCell = PlatformCell(UnsafeCell::new(None));

pub fn configure(platform: PlatformInfo) {
    unsafe { *PLATFORM.0.get() = Some(platform) };
}

pub fn ecam_physical() -> Option<u64> {
    unsafe { (*PLATFORM.0.get()).map(|platform| platform.pcie_base as u64) }
}

pub fn platform() -> Option<PlatformInfo> {
    unsafe { *PLATFORM.0.get() }
}

pub fn pcie_window() -> Option<(u64, u64)> {
    unsafe {
        (*PLATFORM.0.get()).map(|platform| {
            (
                platform.pcie_mmio_base as u64,
                platform.pcie_mmio_bytes as u64,
            )
        })
    }
}

pub fn activate(slot: u8, function: u8) -> Result<UserTransportGrant, Status> {
    let platform = unsafe { (*PLATFORM.0.get()).ok_or(Status::NotFound)? };
    let device =
        pci::virtio_block_at(platform.pcie_base, slot, function).ok_or(Status::NotFound)?;
    kprintln!(
        "[device] EL0 devmgr requested activation for {:02x}:{:02x}.{}",
        device.bus,
        device.slot,
        device.function
    );
    let transport = match pci::inspect_configured_virtio_block(
        platform.pcie_base,
        device,
        platform.pcie_mmio_base,
        platform.pcie_mmio_bytes,
    ) {
        Ok(transport) => transport,
        Err(error) => {
            kprintln!("[device] EL0 BAR validation failed: {:?}", error);
            return Err(Status::Invalid);
        }
    };
    kprintln!(
        "[virtio] validated EL0 BAR assignment queues={} capacity-sectors={} notify-multiplier={}",
        transport.queues,
        transport.capacity_sectors,
        transport.notify_multiplier
    );
    let pin = pci::interrupt_pin(platform.pcie_base, device);
    #[cfg(not(target_arch = "x86_64"))]
    let irq = platform
        .intx_irq(device.slot, pin)
        .ok_or(Status::NotFound)?;
    #[cfg(target_arch = "x86_64")]
    let irq = pci::interrupt_line(platform.pcie_base, device).ok_or(Status::NotFound)?;
    let requester_id = device.requester_id();
    let stream_id = platform.stream_id(requester_id).ok_or(Status::NotFound)?;
    #[cfg(target_arch = "aarch64")]
    let iommu_base = platform.smmu_base;
    #[cfg(target_arch = "riscv64")]
    let iommu_base = platform.riscv_iommu_base;
    #[cfg(target_arch = "x86_64")]
    let iommu_base = platform.smmu_base;
    let domain = smmu::create_domain(
        iommu_base,
        stream_id,
        transport.queue_physical(),
        transport.data_physical(),
    )
    .map_err(|_| Status::Io)?;
    #[cfg(target_arch = "aarch64")]
    let iommu_name = "SMMUv3";
    #[cfg(target_arch = "riscv64")]
    let iommu_name = "RISC-V";
    #[cfg(target_arch = "x86_64")]
    let iommu_name = "Intel VT-d";
    kprintln!(
        "[iommu] {} domain stream-id={:#x} iova={:#x} idr0={:#x} idr5={:#x} gerror={:#x} cmdq=true",
        iommu_name,
        domain.stream_id,
        domain.iova,
        domain.idr0,
        domain.idr5,
        domain.gerror
    );

    transport.negotiate_features().map_err(|_| Status::Io)?;
    transport
        .prepare_fault_probe(domain.probe_iova())
        .map_err(|_| Status::Io)?;
    kprintln!("[virtio] EL1 isolation queue armed; block data I/O delegated to EL0");
    let probe = transport.probe_dma_fault(domain.probe_iova());
    let mut event = domain.take_fault();
    for _ in 0..100_000 {
        if event.is_some() {
            break;
        }
        core::hint::spin_loop();
        event = domain.take_fault();
    }
    #[cfg(target_arch = "riscv64")]
    if event.is_none() {
        let diagnostics = domain.fault_diagnostics();
        kprintln!(
            "[iommu] fault-probe missing sentinel={} iova={:#x} completion-error={} fqh={:#x} fqt={:#x} fqcsr={:#x} ipsr={:#x} ddtp={:#x}",
            probe.sentinel_intact,
            probe.attempted_iova,
            probe.completion_error,
            diagnostics.head,
            diagnostics.tail,
            diagnostics.csr,
            diagnostics.ipsr,
            diagnostics.ddtp
        );
    }
    let event = event.ok_or(Status::Io)?;
    #[cfg(target_arch = "aarch64")]
    let expected_fault = 0x10;
    #[cfg(target_arch = "riscv64")]
    let expected_fault = 0x0f;
    #[cfg(target_arch = "x86_64")]
    let expected_fault = 0x01;
    let blocked = event.event_type == expected_fault
        && event.address == probe.attempted_iova
        && probe.sentinel_intact;
    kprintln!(
        "[iommu] fault-probe blocked={} sentinel={} event={:#x} stream-id={:#x} iova={:#x} completion-error={}",
        blocked,
        probe.sentinel_intact,
        event.event_type,
        event.stream_id,
        event.address,
        probe.completion_error
    );
    if !blocked {
        return Err(Status::Io);
    }

    // Stop the deliberate bad-DMA probe before any production queues start.
    transport
        .reset_for_user(platform.pcie_base, device)
        .map_err(|_| Status::Io)?;
    for _ in 0..256 {
        let Some(residual) = smmu::take_fault() else {
            break;
        };
        if residual.stream_id != stream_id {
            kprintln!("[iommu] unexpected fault before production stream={:#x} address={:#x}", residual.stream_id, residual.address);
            return Err(Status::Io);
        }
    }
    pci::reserve_hotplug_window(
        platform.pcie_base,
        platform.pcie_mmio_base,
        platform.pcie_mmio_bytes,
    )
    .map_err(|_| Status::Io)?;
    if pci::virtio_device_at(platform.pcie_base, 7, 0, pci::VIRTIO_RNG_MODERN).is_some() {
        crate::random::activate(platform).map_err(|_| Status::Io)?;
    }
    if pci::virtio_device_at(platform.pcie_base, 3, 0, pci::VIRTIO_GPU_MODERN).is_some() {
        crate::gpu::activate(platform).map_err(|_| Status::Io)?;
        crate::input::activate(platform).map_err(|_| Status::Io)?;
        crate::service_runtime::enable_gui_services();
    }
    crate::net::activate(platform).map_err(|_| Status::Io)?;

    #[cfg(target_arch = "aarch64")]
    let irq_result = arch::selected::bind_device_irq(platform.gicd_base, irq, transport.isr);
    #[cfg(target_arch = "riscv64")]
    let irq_result = arch::selected::bind_device_irq(platform.plic_base, irq, transport.isr);
    #[cfg(target_arch = "x86_64")]
    let irq_result = arch::selected::bind_device_irq(platform.gicr_base, irq, transport.isr);
    irq_result.map_err(|_| Status::Io)?;

    #[cfg(target_arch = "riscv64")]
    {
        let drained = domain.drain_faults();
        kprintln!("[iommu] fault queue residual-drained={}", drained);
    }
    #[cfg(target_arch = "aarch64")]
    kprintln!(
        "[irq] virtio-blk INTx pin={} gic-id={} bound cpu0",
        pin,
        irq
    );
    #[cfg(target_arch = "riscv64")]
    kprintln!(
        "[irq] virtio-blk INTx pin={} plic-id={} bound cpu0",
        pin,
        irq
    );
    #[cfg(target_arch = "x86_64")]
    kprintln!(
        "[irq] virtio-blk INTx pin={} ioapic-id={} bound cpu0",
        pin,
        irq
    );
    kprintln!("[virtio] boot probe reset; EL0 devmgr owns PCI discovery, BARs and features");

    Ok(transport.user_grant(domain.iova, device.config_physical(platform.pcie_base)))
}
