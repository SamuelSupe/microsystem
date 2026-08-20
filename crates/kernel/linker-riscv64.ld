OUTPUT_ARCH(riscv)
ENTRY(_start)

KERNEL_OFFSET = 0xffffffc000000000;
KERNEL_PHYS = 0x80200000;

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
    __kernel_load_start = .;

    .boot : ALIGN(4096)
    {
        KEEP(*(.text.boot))
        KEEP(*(.text.user))
        KEEP(*(.rodata.boot))
        KEEP(*(.data.boot))
        *(.bss.boot)
    } :boot

    . = ALIGN(4096);
    __boot_end = .;
    boot_root_high = boot_root + KERNEL_OFFSET;
    boot_user_l1_high = boot_user_l1 + KERNEL_OFFSET;
    boot_user_l2_high = boot_user_l2 + KERNEL_OFFSET;
    boot_rr_data_high = boot_rr_data + KERNEL_OFFSET;
    boot_task1_root_high = boot_task1_root + KERNEL_OFFSET;
    boot_task1_user_l1_high = boot_task1_user_l1 + KERNEL_OFFSET;
    boot_task1_user_l2_high = boot_task1_user_l2 + KERNEL_OFFSET;
    boot_task1_rr_data_high = boot_task1_rr_data + KERNEL_OFFSET;
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
        *(.sdata .sdata.*)
    } :data

    .bss ALIGN(4096) (NOLOAD) : AT(ADDR(.bss) - KERNEL_OFFSET)
    {
        __bss_start = .;
        *(.bss .bss.* COMMON)
        *(.sbss .sbss.*)
        __bss_end = .;
    } :data

    . = ALIGN(4096);
    __kernel_end = .;
    __kernel_load_end = __kernel_end - KERNEL_OFFSET;

    /DISCARD/ :
    {
        *(.comment)
        *(.eh_frame*)
        *(.note*)
    }
}
