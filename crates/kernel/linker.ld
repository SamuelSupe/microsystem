OUTPUT_ARCH(aarch64)
ENTRY(_start)

KERNEL_OFFSET = 0xffffff8000000000;
KERNEL_PHYS = 0x40200000;

PHDRS
{
    boot PT_LOAD FLAGS(7);
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = KERNEL_PHYS;
    .boot : ALIGN(4096)
    {
        KEEP(*(.text.boot))
        KEEP(*(.text.user))
        KEEP(*(.rodata.boot))
        *(.bss.boot)
    } :boot

    . = ALIGN(4096);
    __boot_end = .;
    boot_user_l3_high = boot_user_l3 + KERNEL_OFFSET;
    boot_ttbr0_l1_high = boot_ttbr0_l1 + KERNEL_OFFSET;
    boot_task1_ttbr0_l1_high = boot_task1_ttbr0_l1 + KERNEL_OFFSET;
    boot_rr_data_high = boot_rr_data + KERNEL_OFFSET;
    boot_task1_rr_data_high = boot_task1_rr_data + KERNEL_OFFSET;
    user_entry_high = user_entry + KERNEL_OFFSET;
    user_rr_entry_high = user_rr_entry + KERNEL_OFFSET;
    user_rr_exit_high = user_rr_exit + KERNEL_OFFSET;
    user_ipc_client_high = user_ipc_client + KERNEL_OFFSET;
    user_ipc_server_high = user_ipc_server + KERNEL_OFFSET;
    . += KERNEL_OFFSET;
    __kernel_start = .;

    .text : AT(ADDR(.text) - KERNEL_OFFSET)
    {
        *(.text .text.*)
    } :text

    .rodata ALIGN(4096) : AT(ADDR(.rodata) - KERNEL_OFFSET)
    {
        *(.rodata .rodata.*)
    } :rodata

    .data ALIGN(4096) : AT(ADDR(.data) - KERNEL_OFFSET)
    {
        *(.data .data.*)
    } :data

    .bss ALIGN(4096) (NOLOAD) : AT(ADDR(.bss) - KERNEL_OFFSET)
    {
        __bss_start = .;
        *(.bss .bss.* COMMON)
        __bss_end = .;
    } :data

    . = ALIGN(4096);
    __kernel_end = .;

    /DISCARD/ :
    {
        *(.comment)
        *(.eh_frame*)
        *(.note*)
    }
}
