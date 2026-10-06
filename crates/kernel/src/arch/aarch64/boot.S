.arch armv8-a+fp+simd
.section .text.boot, "ax"
.balign 4096
.global _start
.global secondary_entry

.equ KERNEL_OFFSET, 0xffffff8000000000
.equ STACK_SIZE, 0x10000

_start:
    msr daifset, #0xf
    mov x21, x0
    mrs x20, mpidr_el1
    and x20, x20, #0xff
    cbnz x20, secondary_park
    bl normalize_el
    bl initialize_fp_simd
    // QEMU's ELF entry may omit x0; preserve a firmware-provided DTB when present.
    mov x0, x21
    cbnz x0, 0f
    mov x0, #0x40000000
0:
    bl set_boot_stack
    bl setup_mmu
    ldr x1, =kernel_main
    br x1

secondary_park:
    wfe
    b secondary_park

secondary_entry:
    msr daifset, #0xf
    bl normalize_el
    bl initialize_fp_simd
    mrs x20, mpidr_el1
    and x20, x20, #0xff
    bl set_boot_stack
    bl setup_mmu
    ldr x1, =secondary_main
    br x1

normalize_el:
    mrs x1, CurrentEL
    cmp x1, #0x8
    b.ne 1f
    mov x1, #(1 << 31)
    msr hcr_el2, x1
    mov x1, #3
    msr cnthctl_el2, x1
    msr cntvoff_el2, xzr
    mov x1, #0xf
    msr icc_sre_el2, x1
    mrs x1, cptr_el2
    bic x1, x1, #(1 << 10)
    msr cptr_el2, x1
    isb
    mov x1, #0x3c5
    msr spsr_el2, x1
    adr x1, 1f
    msr elr_el2, x1
    eret
1:
    ret

initialize_fp_simd:
    mrs x1, cpacr_el1
    orr x1, x1, #(3 << 20)
    msr cpacr_el1, x1
    isb
    ret

set_boot_stack:
    adr x1, boot_stacks
    add x20, x20, #1
    mov x2, #STACK_SIZE
    madd x1, x20, x2, x1
    mov sp, x1
    ret

setup_mmu:
    adr x2, boot_ttbr0_l1
    adr x3, boot_ttbr1_l1

    mov x4, #0x705
    movk x4, #0x4000, lsl #16
    str x4, [x2, #8]
    str x4, [x3, #8]

    mov x4, #0x401
    movk x4, #0x0060, lsl #48
    str x4, [x3]

    mov x4, #0x401
    movk x4, #0x0040, lsl #32
    movk x4, #0x0060, lsl #48
    str x4, [x3, #2048]

    adr x7, boot_user_l2
    mov x5, x7
    orr x5, x5, #3
    str x5, [x2]
    adr x8, boot_user_l3
    mov x5, x8
    orr x5, x5, #3
    str x5, [x7, #16]

    adr x5, user_entry
    and x5, x5, #0xfffffffffffff000
    mov x6, #0x7c7
    movk x6, #0x0020, lsl #48
    orr x5, x5, x6
    str x5, [x8]

    adr x5, boot_user_stack
    mov x6, #0x747
    movk x6, #0x0060, lsl #48
    orr x5, x5, x6
    str x5, [x8, #4088]

    adr x5, boot_rr_data
    mov x6, #0x747
    movk x6, #0x0060, lsl #48
    orr x5, x5, x6
    str x5, [x8, #128]

    adr x11, boot_task1_ttbr0_l1
    adr x12, boot_task1_user_l2
    mov x5, x12
    orr x5, x5, #3
    str x5, [x11]
    adr x13, boot_task1_user_l3
    mov x5, x13
    orr x5, x5, #3
    str x5, [x12, #16]

    adr x5, user_entry
    and x5, x5, #0xfffffffffffff000
    mov x6, #0x7c7
    movk x6, #0x0020, lsl #48
    orr x5, x5, x6
    str x5, [x13]

    adr x5, boot_task1_stack
    mov x6, #0x747
    movk x6, #0x0060, lsl #48
    orr x5, x5, x6
    str x5, [x13, #4088]

    adr x5, boot_task1_rr_data
    mov x6, #0x747
    movk x6, #0x0060, lsl #48
    orr x5, x5, x6
    str x5, [x13, #128]

    msr ttbr0_el1, x2
    msr ttbr1_el1, x3
    mov x4, #0xff00
    msr mair_el1, x4
    ldr x4, =0x00000002b5193519
    msr tcr_el1, x4
    dsb ish
    isb
    mrs x4, sctlr_el1
    orr x4, x4, #(1 << 0)
    orr x4, x4, #(1 << 2)
    orr x4, x4, #(1 << 12)
    msr sctlr_el1, x4
    isb
    ldr x4, =KERNEL_OFFSET
    add sp, sp, x4
    ret

.balign 4096
.global boot_ttbr0_l1
boot_ttbr0_l1:
    .skip 4096
boot_ttbr1_l1:
    .skip 4096
boot_user_l2:
    .skip 4096
.global boot_user_l3
boot_user_l3:
    .skip 4096

.balign 4096
.global boot_task1_ttbr0_l1
boot_task1_ttbr0_l1:
    .skip 4096
boot_task1_user_l2:
    .skip 4096
boot_task1_user_l3:
    .skip 4096

.balign 4096
boot_user_stack:
    .skip 4096

.balign 4096
.global boot_rr_data
boot_rr_data:
    .skip 4096

.balign 4096
.global boot_task1_stack
boot_task1_stack:
    .skip 4096

.balign 4096
.global boot_task1_rr_data
boot_task1_rr_data:
    .skip 4096

.balign 16
boot_stacks:
    .skip STACK_SIZE * 2

.section .text.user, "ax"
.balign 4096
.global user_entry
user_entry:
    adr x0, user_message
    mov x1, #(user_message_end - user_message)
    mov x8, #19
    svc #0
    mov x0, #0
    mov x8, #1
    svc #0
1:
    wfe
    b 1b
user_message:
    .ascii "[user] init entered EL0 through SVC ABI\n"
user_message_end:
.global user_fault_entry
user_fault_entry:
    adr x0, user_fault_message
    mov x1, #(user_fault_message_end - user_fault_message)
    mov x8, #19
    svc #0
    mov x0, #-1
    mov x1, #4
    mov x8, #19
    svc #0
    mov x0, #0x00600000
    ldr x1, [x0]
1:
    b 1b
user_fault_message:
    .ascii "[user] isolation probe entered EL0\n"
user_fault_message_end:
.balign 4
.global user_smp_entry
user_smp_entry:
    mov x2, #0
    movz x3, #0xb400
    movk x3, #0x04c4, lsl #16
1:
    add x2, x2, #1
    cmp x2, x3
    b.ne 1b
    mov x0, x2
    mov x8, #1
    svc #0
2:
    b 2b

.balign 4
.macro initialize_simd_register register, index
    add x3, x2, #\index
    dup \register\().2d, x3
.endm

.macro check_simd_register register, index
    add x4, x2, #\index
    umov x3, \register\().d[0]
    cmp x3, x4
    b.ne 3f
    umov x3, \register\().d[1]
    cmp x3, x4
    b.ne 3f
.endm

.global user_rr_entry
user_rr_entry:
    ldr x2, [x0, #8]
    initialize_simd_register v0, 0
    initialize_simd_register v1, 1
    initialize_simd_register v2, 2
    initialize_simd_register v3, 3
    initialize_simd_register v4, 4
    initialize_simd_register v5, 5
    initialize_simd_register v6, 6
    initialize_simd_register v7, 7
    initialize_simd_register v8, 8
    initialize_simd_register v9, 9
    initialize_simd_register v10, 10
    initialize_simd_register v11, 11
    initialize_simd_register v12, 12
    initialize_simd_register v13, 13
    initialize_simd_register v14, 14
    initialize_simd_register v15, 15
    initialize_simd_register v16, 16
    initialize_simd_register v17, 17
    initialize_simd_register v18, 18
    initialize_simd_register v19, 19
    initialize_simd_register v20, 20
    initialize_simd_register v21, 21
    initialize_simd_register v22, 22
    initialize_simd_register v23, 23
    initialize_simd_register v24, 24
    initialize_simd_register v25, 25
    initialize_simd_register v26, 26
    initialize_simd_register v27, 27
    initialize_simd_register v28, 28
    initialize_simd_register v29, 29
    initialize_simd_register v30, 30
    initialize_simd_register v31, 31
    ldr x5, [x0, #32]
    ldr x6, [x0, #40]
    msr fpcr, x5
    msr fpsr, x6
1:
    check_simd_register v0, 0
    check_simd_register v1, 1
    check_simd_register v2, 2
    check_simd_register v3, 3
    check_simd_register v4, 4
    check_simd_register v5, 5
    check_simd_register v6, 6
    check_simd_register v7, 7
    check_simd_register v8, 8
    check_simd_register v9, 9
    check_simd_register v10, 10
    check_simd_register v11, 11
    check_simd_register v12, 12
    check_simd_register v13, 13
    check_simd_register v14, 14
    check_simd_register v15, 15
    check_simd_register v16, 16
    check_simd_register v17, 17
    check_simd_register v18, 18
    check_simd_register v19, 19
    check_simd_register v20, 20
    check_simd_register v21, 21
    check_simd_register v22, 22
    check_simd_register v23, 23
    check_simd_register v24, 24
    check_simd_register v25, 25
    check_simd_register v26, 26
    check_simd_register v27, 27
    check_simd_register v28, 28
    check_simd_register v29, 29
    check_simd_register v30, 30
    check_simd_register v31, 31
    mrs x3, fpcr
    cmp x3, x5
    b.ne 3f
    mrs x3, fpsr
    cmp x3, x6
    b.ne 3f
    ldr x3, [x0, #24]
    add x3, x3, #1
    str x3, [x0, #24]
2:
    ldr x1, [x0]
    add x1, x1, #1
    str x1, [x0]
    b 1b
3:
    ldr x3, [x0, #16]
    add x3, x3, #1
    str x3, [x0, #16]
    b 2b

.global user_rr_exit
user_rr_exit:
    mov x0, #0
    mov x8, #1
    svc #0
3:
    b 3b

.balign 4
.global user_ipc_client
user_ipc_client:
    mov x0, #1
    mov x1, #0x1234
    mov x8, #2
    svc #0
    mov x1, #0x00410000
    str x0, [x1]
    mov x0, #0
    mov x8, #1
    svc #0
4:
    b 4b

.global user_ipc_server
user_ipc_server:
    mov x0, #1
    mov x8, #3
    svc #0
    mov x2, #0x00410000
    str x0, [x2]
    lsl x1, x0, #1
    mov x0, #1
    mov x8, #4
    svc #0
5:
    b 5b
