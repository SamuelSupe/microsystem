#!/usr/bin/env bash

# Shared target and QEMU selection for the launch wrappers.  Keep the
# architecture-specific machine/device knobs here so standalone wrappers and
# the xtask entrypoint agree on artifact locations and firmware boot mode.
MICROSYSTEM_ARCH="${ARCH:-aarch64}"
case "$MICROSYSTEM_ARCH" in
  aarch64)
    MICROSYSTEM_TARGET='aarch64-unknown-none-softfloat'
    MICROSYSTEM_QEMU_BINARY='qemu-system-aarch64'
    MICROSYSTEM_QEMU_PLATFORM_ARGS=(
      -machine 'virt-7.2,virtualization=on,gic-version=3,iommu=smmuv3'
      -cpu cortex-a72
    )
    MICROSYSTEM_QEMU_BOOT_ARGS=(
      -L /usr/lib/ipxe/qemu
      -no-reboot
      -semihosting-config enable=on,target=native
    )
    MICROSYSTEM_VIRTIO_PCI_OPTIONS='disable-legacy=on,iommu_platform=on,romfile=,'
    ;;
  riscv64)
    MICROSYSTEM_TARGET='riscv64gc-unknown-none-elf'
    MICROSYSTEM_QEMU_BINARY='qemu-system-riscv64'
    MICROSYSTEM_QEMU_PLATFORM_ARGS=(
      -machine 'virt,iommu-sys=on'
      -bios default
      -cpu rv64
    )
    MICROSYSTEM_QEMU_BOOT_ARGS=(
      -no-reboot
    )
    MICROSYSTEM_VIRTIO_PCI_OPTIONS='disable-legacy=on,iommu_platform=on,romfile=,'
    ;;
  *)
    echo "qemu-arch: ARCH must be aarch64 or riscv64 (got $MICROSYSTEM_ARCH)" >&2
    return 2 2>/dev/null || exit 2
    ;;
esac
