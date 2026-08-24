.section .multiboot, "a"
.balign 8
multiboot_header:
.long 0xe85250d6
.long 0
.long multiboot_header_end - multiboot_header
.long -(0xe85250d6 + (multiboot_header_end - multiboot_header))
.word 0
.word 0
.long 8
multiboot_header_end:

.section .text.boot, "ax"
.code32
.global _start
.type _start, @function
_start:
    cli
    mov $boot_stack_top, %esp
    mov %ebx, multiboot_info

    mov $boot_pdpt, %eax
    or $0x003, %eax
    mov %eax, boot_pml4
    movl $0, boot_pml4 + 4
    mov %eax, boot_pml4 + 256 * 8
    movl $0, boot_pml4 + 256 * 8 + 4

    mov $boot_pd0, %eax
    or $0x003, %eax
    mov %eax, boot_pdpt
    movl $0, boot_pdpt + 4
    mov $boot_pd1, %eax
    or $0x003, %eax
    mov %eax, boot_pdpt + 8
    movl $0, boot_pdpt + 12
    mov $boot_pd2, %eax
    or $0x003, %eax
    mov %eax, boot_pdpt + 16
    movl $0, boot_pdpt + 20
    mov $boot_pd3, %eax
    or $0x003, %eax
    mov %eax, boot_pdpt + 24
    movl $0, boot_pdpt + 28

    xor %ecx, %ecx
1:
    mov %ecx, %eax
    shl $21, %eax
    or $0x083, %eax
    mov %ecx, %ebx
    shr $9, %ebx
    and $3, %ebx
    cmp $0, %ebx
    je 2f
    cmp $1, %ebx
    je 3f
    cmp $2, %ebx
    je 4f
    lea boot_pd3, %edi
    jmp 5f
2:
    lea boot_pd0, %edi
    jmp 5f
3:
    lea boot_pd1, %edi
    jmp 5f
4:
    lea boot_pd2, %edi
5:
    mov %ecx, %edx
    and $0x1ff, %edx
    mov %eax, (%edi,%edx,8)
    mov %ecx, %ebp
    shr $11, %ebp
    mov %ebp, 4(%edi,%edx,8)
    inc %ecx
    cmp $2048, %ecx
    jne 1b

    mov %cr4, %eax
    or $(1 << 5 | 1 << 9 | 1 << 10), %eax
    mov %eax, %cr4
    mov $boot_pml4, %eax
    mov %eax, %cr3
    mov $0xc0000080, %ecx
    rdmsr
    or $(1 << 8 | 1 << 11), %eax
    wrmsr
    mov %cr0, %eax
    and $~(1 << 2 | 1 << 3), %eax
    or $(1 << 31 | 1 << 16 | 1 << 5 | 1 << 1 | 1), %eax
    mov %eax, %cr0
    lgdt boot_gdt_pointer
    ljmp $0x08, $long_mode_entry

.code64
long_mode_entry:
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    xor %ax, %ax
    mov %ax, %fs
    mov %ax, %gs
    movabs $0xffff800000000000, %rax
    add %rax, %rsp
    xor %rbp, %rbp
    xor %rdi, %rdi
    movabs $kernel_main, %rax
    call *%rax
6:
    hlt
    jmp 6b

.balign 16
.global ap_trampoline
ap_trampoline:
.code16
    cli
    xor %ax, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    mov $0x7c00, %sp
    lgdt 0x8000 + (ap_gdt_pointer - ap_trampoline)
    mov %cr0, %eax
    or $1, %eax
    mov %eax, %cr0
    ljmpl $0x08, $0x8000 + (ap_protected_mode - ap_trampoline)

.code32
ap_protected_mode:
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    mov %cr4, %eax
    or $(1 << 5 | 1 << 9 | 1 << 10), %eax
    mov %eax, %cr4
    mov $boot_pml4, %eax
    mov %eax, %cr3
    mov $0xc0000080, %ecx
    rdmsr
    or $(1 << 8 | 1 << 11), %eax
    wrmsr
    mov %cr0, %eax
    and $~(1 << 2 | 1 << 3), %eax
    or $(1 << 31 | 1 << 16 | 1 << 5 | 1 << 1 | 1), %eax
    mov %eax, %cr0
    ljmpl $0x18, $0x8000 + (ap_long_mode - ap_trampoline)

.code64
ap_long_mode:
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    movabs $ap_boot_stack_top_high, %rsp
    xor %rbp, %rbp
    movabs $secondary_main, %rax
    call *%rax
1:
    hlt
    jmp 1b

.code16
.balign 8
ap_gdt:
    .quad 0
    .quad 0x00cf9a000000ffff
    .quad 0x00cf92000000ffff
    .quad 0x00af9a000000ffff
ap_gdt_end:
ap_gdt_pointer:
    .word ap_gdt_end - ap_gdt - 1
    .long 0x8000 + (ap_gdt - ap_trampoline)
.global ap_trampoline_end
ap_trampoline_end:
.code64

.section .rodata.boot, "a"
.balign 8
boot_gdt:
    .quad 0
    .quad 0x00af9a000000ffff
    .quad 0x00af92000000ffff
    .quad 0x00affa000000ffff
    .quad 0x00aff2000000ffff
boot_gdt_end:
boot_gdt_pointer:
    .word boot_gdt_end - boot_gdt - 1
    .long boot_gdt

.section .data.boot, "aw"
.balign 8
multiboot_info:
    .quad 0

.section .bss.boot, "aw", @nobits
.balign 4096
.global boot_pml4
boot_pml4:
    .skip 4096
.global boot_pdpt
boot_pdpt:
    .skip 4096
boot_pd0:
    .skip 4096
boot_pd1:
    .skip 4096
boot_pd2:
    .skip 4096
boot_pd3:
    .skip 4096

.balign 4096
.global boot_user_pml4
boot_user_pml4:
    .skip 4096
.global boot_user_pdpt
boot_user_pdpt:
    .skip 4096
.global boot_user_pd
boot_user_pd:
    .skip 4096
.global boot_user_pt
boot_user_pt:
    .skip 4096

.balign 4096
.global boot_task1_pml4
boot_task1_pml4:
    .skip 4096
.global boot_task1_pdpt
boot_task1_pdpt:
    .skip 4096
.global boot_task1_pd
boot_task1_pd:
    .skip 4096
.global boot_task1_pt
boot_task1_pt:
    .skip 4096

.balign 4096
.global boot_rr_data
boot_rr_data:
    .skip 4096
.global boot_task1_rr_data
boot_task1_rr_data:
    .skip 4096

.balign 4096
.global boot_user_stack
boot_user_stack:
    .skip 4096
.global boot_task1_stack
boot_task1_stack:
    .skip 4096

.balign 16
boot_stack:
    .skip 0x10000
boot_stack_top:

.balign 16
ap_boot_stack:
    .skip 0x10000
.global ap_boot_stack_top
ap_boot_stack_top:

.section .text.user, "ax"
.balign 4096
.global user_entry
user_entry:
    xor %rdi, %rdi
    mov $1, %rax
    int $0x80
1:
    pause
    jmp 1b

.global user_fault_entry
user_fault_entry:
    mov 0x00600000, %rax
    jmp user_fault_entry

.global user_smp_entry
user_smp_entry:
    xor %rdi, %rdi
    mov $0x04c4b400, %rcx
1:
    inc %rdi
    loop 1b
    mov $1, %rax
    int $0x80
2:
    pause
    jmp 2b

.global user_rr_entry
user_rr_entry:
    movq %xmm0, %rax
    incq 16(%rdi)
    cmpq 24(%rdi), %rax
    je 1f
    incq 8(%rdi)
1:
    mov (%rdi), %rax
    inc %rax
    mov %rax, (%rdi)
    cmp $20000000, %rax
    jb user_rr_entry
.global user_rr_exit
user_rr_exit:
    xor %rdi, %rdi
    mov $1, %rax
    int $0x80
    jmp user_rr_exit

.global user_ipc_client
user_ipc_client:
    mov $0xcafe, %rsi
    mov $2, %rax
    int $0x80
    xor %rdi, %rdi
    mov $1, %rax
    int $0x80
    jmp user_ipc_client

.global user_ipc_server
user_ipc_server:
    mov $3, %rax
    int $0x80
    mov $1, %rdi
    mov $0xbeef, %rsi
    mov $4, %rax
    int $0x80
    jmp user_ipc_server
