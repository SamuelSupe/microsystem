OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

KERNEL_OFFSET = 0xffff800000000000;
KERNEL_PHYS = 0x00100000;

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
        KEEP(*(.multiboot))
        KEEP(*(.text.boot))
        KEEP(*(.text.user))
        KEEP(*(.rodata.boot))
        KEEP(*(.data.boot))
        *(.bss.boot)
    } :boot

    . = ALIGN(4096);
    __boot_end = .;
    boot_pdpt_high = boot_pdpt + KERNEL_OFFSET;
    boot_user_pml4_high = boot_user_pml4 + KERNEL_OFFSET;
    boot_user_pdpt_high = boot_user_pdpt + KERNEL_OFFSET;
    boot_user_pd_high = boot_user_pd + KERNEL_OFFSET;
    boot_user_pt_high = boot_user_pt + KERNEL_OFFSET;
    boot_task1_pml4_high = boot_task1_pml4 + KERNEL_OFFSET;
    boot_task1_pdpt_high = boot_task1_pdpt + KERNEL_OFFSET;
    boot_task1_pd_high = boot_task1_pd + KERNEL_OFFSET;
    boot_task1_pt_high = boot_task1_pt + KERNEL_OFFSET;
    boot_rr_data_high = boot_rr_data + KERNEL_OFFSET;
    boot_task1_rr_data_high = boot_task1_rr_data + KERNEL_OFFSET;
    boot_user_stack_high = boot_user_stack + KERNEL_OFFSET;
    boot_task1_stack_high = boot_task1_stack + KERNEL_OFFSET;
    ap_trampoline_high = ap_trampoline + KERNEL_OFFSET;
    ap_trampoline_end_high = ap_trampoline_end + KERNEL_OFFSET;
    ap_boot_stack_top_high = ap_boot_stack_top + KERNEL_OFFSET;
    user_entry_high = user_entry + KERNEL_OFFSET;
    user_fault_entry_high = user_fault_entry + KERNEL_OFFSET;
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
        *(.got .got.*)
    } :data

    .bss ALIGN(4096) (NOLOAD) : AT(ADDR(.bss) - KERNEL_OFFSET)
    {
        __bss_start = .;
        *(.bss .bss.* COMMON)
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
