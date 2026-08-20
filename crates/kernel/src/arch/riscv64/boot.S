.option push
.option arch, rv64gc

.section .text.boot, "ax"
.balign 4

.equ STACK_SIZE, 0x4000
.equ TRAP_STACK_SIZE, 0x10000
.equ MAX_HARTS, 8
.equ KERNEL_OFFSET, 0xffffffc000000000
.equ USER_ENTRY_VA, 0x00400000
.equ USER_STACK_TOP, 0x00800000
.equ SSTATUS_SPIE, (1 << 5)
.equ SSTATUS_SPP, (1 << 8)
.equ SSTATUS_SIE, (1 << 1)
.equ SSTATUS_FS, (3 << 13)
.equ SATP_MODE_SV39, (8 << 60)
.equ BOOT_PTE_FLAGS, 0xcf
.equ USER_PTE_FLAGS, 0xdb
.equ RR_PTE_FLAGS, 0xd7

.global _start
.type _start, @function
_start:
    csrw sie, zero
    csrw sstatus, zero
    mv tp, a0

    andi t0, a0, MAX_HARTS - 1
    slli t0, t0, 14
    la sp, boot_stacks
    add sp, sp, t0
    li t1, STACK_SIZE
    add sp, sp, t1

    la t6, trap_scratch
    slli t0, a0, 6
    add t6, t6, t0
    andi t0, a0, MAX_HARTS - 1
    slli t0, t0, 16
    la t1, trap_stacks
    add t1, t1, t0
    li t0, TRAP_STACK_SIZE
    add t1, t1, t0
    sd t1, 0(t6)
    sd tp, 8(t6)
    csrw sscratch, t6

    li t0, SSTATUS_FS
    csrs sstatus, t0
    // Install a bootstrap Sv39 map: identity RAM, high-half RAM, and the
    // high-half MMIO window used by phys_to_virt().
    la t0, boot_root
    la t1, boot_user_l1
    srli t2, t1, 12
    slli t2, t2, 10
    ori t2, t2, 1
    sd t2, 0(t0)
    la t0, boot_user_l2
    srli t2, t0, 12
    slli t2, t2, 10
    ori t2, t2, 1
    la t1, boot_user_l1
    sd t2, 16(t1)
    la t0, user_entry
    srli t2, t0, 12
    slli t2, t2, 10
    ori t2, t2, USER_PTE_FLAGS
    la t1, boot_user_l2
    sd t2, 0(t1)
    la t0, user_entry
    li t1, -4096
    and t0, t0, t1
    la t1, boot_user_l3
    li t2, USER_PTE_FLAGS
    li t3, 8
    li t5, 4096
1:
    srli t4, t0, 12
    slli t4, t4, 10
    or t4, t4, t2
    sd t4, 0(t1)
    add t0, t0, t5
    addi t1, t1, 8
    addi t3, t3, -1
    bnez t3, 1b
    la t0, boot_rr_data
    srli t2, t0, 12
    slli t2, t2, 10
    ori t2, t2, RR_PTE_FLAGS
    la t1, boot_user_l2
    sd t2, 128(t1)
    la t0, boot_root
    li t1, 0x80000000
    srli t1, t1, 12
    slli t1, t1, 10
    ori t1, t1, BOOT_PTE_FLAGS
    sd t1, 16(t0)
    li t6, 2048
    add t6, t0, t6
    sd t1, 16(t6)
    li t1, BOOT_PTE_FLAGS
    sd t1, 0(t6)
    li t1, 0x40000000
    srli t1, t1, 12
    slli t1, t1, 10
    ori t1, t1, BOOT_PTE_FLAGS
    sd t1, 8(t6)
    la t1, boot_root
    srli t1, t1, 12
    li t2, SATP_MODE_SV39
    or t1, t1, t2
    csrw satp, t1
    sfence.vma zero, zero
    fence.i
    la t0, trap_vector_pointer
    ld t0, 0(t0)
    csrw stvec, t0

    mv a0, a1
    la t0, kernel_main_pointer
    ld t0, 0(t0)
    li t1, KERNEL_OFFSET
    add sp, sp, t1
    jr t0

.global secondary_entry
.type secondary_entry, @function
secondary_entry:
    csrw sie, zero
    csrw sstatus, zero
    mv tp, a0

    andi t0, a0, MAX_HARTS - 1
    slli t0, t0, 14
    la sp, boot_stacks
    add sp, sp, t0
    li t1, STACK_SIZE
    add sp, sp, t1

    la t6, trap_scratch
    slli t0, a0, 6
    add t6, t6, t0
    andi t0, a0, MAX_HARTS - 1
    slli t0, t0, 16
    la t1, trap_stacks
    add t1, t1, t0
    li t0, TRAP_STACK_SIZE
    add t1, t1, t0
    sd t1, 0(t6)
    sd tp, 8(t6)
    csrw sscratch, t6

    li t0, SSTATUS_FS
    csrs sstatus, t0
    la t0, boot_root
    la t1, boot_user_l1
    srli t2, t1, 12
    slli t2, t2, 10
    ori t2, t2, 1
    sd t2, 0(t0)
    la t0, boot_user_l2
    srli t2, t0, 12
    slli t2, t2, 10
    ori t2, t2, 1
    la t1, boot_user_l1
    sd t2, 16(t1)
    la t0, user_entry
    srli t2, t0, 12
    slli t2, t2, 10
    ori t2, t2, USER_PTE_FLAGS
    la t1, boot_user_l2
    sd t2, 0(t1)
    la t0, user_entry
    li t1, -4096
    and t0, t0, t1
    la t1, boot_user_l3
    li t2, USER_PTE_FLAGS
    li t3, 8
    li t5, 4096
2:
    srli t4, t0, 12
    slli t4, t4, 10
    or t4, t4, t2
    sd t4, 0(t1)
    add t0, t0, t5
    addi t1, t1, 8
    addi t3, t3, -1
    bnez t3, 2b
    la t0, boot_rr_data
    srli t2, t0, 12
    slli t2, t2, 10
    ori t2, t2, RR_PTE_FLAGS
    la t1, boot_user_l2
    sd t2, 128(t1)
    la t0, boot_root
    li t1, 0x80000000
    srli t1, t1, 12
    slli t1, t1, 10
    ori t1, t1, BOOT_PTE_FLAGS
    sd t1, 16(t0)
    li t6, 2048
    add t6, t0, t6
    sd t1, 16(t6)
    li t1, BOOT_PTE_FLAGS
    sd t1, 0(t6)
    li t1, 0x40000000
    srli t1, t1, 12
    slli t1, t1, 10
    ori t1, t1, BOOT_PTE_FLAGS
    sd t1, 8(t6)
    la t1, boot_root
    srli t1, t1, 12
    li t2, SATP_MODE_SV39
    or t1, t1, t2
    csrw satp, t1
    sfence.vma zero, zero
    fence.i
    la t0, trap_vector_pointer
    ld t0, 0(t0)
    csrw stvec, t0
    la t0, secondary_main_pointer
    ld t0, 0(t0)
    li t1, KERNEL_OFFSET
    add sp, sp, t1
    jr t0

.section .rodata.boot, "a"
.balign 8
trap_vector_pointer:
    .dword trap_vector
kernel_main_pointer:
    .dword kernel_main
secondary_main_pointer:
    .dword secondary_main

.section .bss.boot, "aw", @nobits
.balign 16
.global trap_scratch
trap_scratch:
    .skip 64 * MAX_HARTS

.balign 4096
.global boot_root
boot_root:
    .skip 4096

.balign 4096
.global boot_user_l1
boot_user_l1:
    .skip 4096

.balign 4096
.global boot_user_l2
boot_user_l2:
    .skip 4096

.balign 4096
boot_user_l3:
    .skip 4096

.balign 4096
.global boot_rr_data
boot_rr_data:
    .skip 4096

.balign 4096
.global boot_task1_root
boot_task1_root:
    .skip 4096

.balign 4096
.global boot_task1_user_l1
boot_task1_user_l1:
    .skip 4096

.balign 4096
.global boot_task1_user_l2
boot_task1_user_l2:
    .skip 4096

.balign 4096
.global boot_task1_rr_data
boot_task1_rr_data:
    .skip 4096

.balign 16
.global boot_stacks
boot_stacks:
    .skip STACK_SIZE * MAX_HARTS

.balign 16
trap_stacks:
    .skip TRAP_STACK_SIZE * MAX_HARTS

.section .text.user, "ax"
.balign 4096

.global user_entry
.type user_entry, @function
user_entry:
    li a0, 0
    li a7, 1
    ecall
1:
    wfi
    j 1b

.global user_fault_entry
.type user_fault_entry, @function
user_fault_entry:
    li t0, 0x00600000
    ld t1, 0(t0)
    j user_fault_entry

.global user_smp_entry
.type user_smp_entry, @function
user_smp_entry:
    li t0, 0
    li t1, 0x04c4b400
1:
    addi t0, t0, 1
    bne t0, t1, 1b
    mv a0, t0
    li a7, 1
    ecall
2:
    wfi
    j 2b

.global user_rr_entry
.type user_rr_entry, @function
.macro initialize_fp_register register, index
    addi t5, t2, \index
    fmv.d.x \register, t5
.endm

.macro check_fp_register register, index
    addi t5, t2, \index
    fmv.x.d t4, \register
    bne t4, t5, 3f
.endm

user_rr_entry:
    ld t2, 8(a0)
    initialize_fp_register f0, 0
    initialize_fp_register f1, 1
    initialize_fp_register f2, 2
    initialize_fp_register f3, 3
    initialize_fp_register f4, 4
    initialize_fp_register f5, 5
    initialize_fp_register f6, 6
    initialize_fp_register f7, 7
    initialize_fp_register f8, 8
    initialize_fp_register f9, 9
    initialize_fp_register f10, 10
    initialize_fp_register f11, 11
    initialize_fp_register f12, 12
    initialize_fp_register f13, 13
    initialize_fp_register f14, 14
    initialize_fp_register f15, 15
    initialize_fp_register f16, 16
    initialize_fp_register f17, 17
    initialize_fp_register f18, 18
    initialize_fp_register f19, 19
    initialize_fp_register f20, 20
    initialize_fp_register f21, 21
    initialize_fp_register f22, 22
    initialize_fp_register f23, 23
    initialize_fp_register f24, 24
    initialize_fp_register f25, 25
    initialize_fp_register f26, 26
    initialize_fp_register f27, 27
    initialize_fp_register f28, 28
    initialize_fp_register f29, 29
    initialize_fp_register f30, 30
    initialize_fp_register f31, 31
    ld t3, 32(a0)
    csrw fcsr, t3
1:
    check_fp_register f0, 0
    check_fp_register f1, 1
    check_fp_register f2, 2
    check_fp_register f3, 3
    check_fp_register f4, 4
    check_fp_register f5, 5
    check_fp_register f6, 6
    check_fp_register f7, 7
    check_fp_register f8, 8
    check_fp_register f9, 9
    check_fp_register f10, 10
    check_fp_register f11, 11
    check_fp_register f12, 12
    check_fp_register f13, 13
    check_fp_register f14, 14
    check_fp_register f15, 15
    check_fp_register f16, 16
    check_fp_register f17, 17
    check_fp_register f18, 18
    check_fp_register f19, 19
    check_fp_register f20, 20
    check_fp_register f21, 21
    check_fp_register f22, 22
    check_fp_register f23, 23
    check_fp_register f24, 24
    check_fp_register f25, 25
    check_fp_register f26, 26
    check_fp_register f27, 27
    check_fp_register f28, 28
    check_fp_register f29, 29
    check_fp_register f30, 30
    check_fp_register f31, 31
    csrr t4, fcsr
    bne t4, t3, 3f
    ld t0, 24(a0)
    addi t0, t0, 1
    sd t0, 24(a0)
2:
    ld t0, 0(a0)
    addi t0, t0, 1
    sd t0, 0(a0)
    j 1b
3:
    ld t0, 16(a0)
    addi t0, t0, 1
    sd t0, 16(a0)
    j 2b

.global user_rr_exit
.type user_rr_exit, @function
user_rr_exit:
    li a0, 0
    li a7, 1
    ecall
    ret

.global user_ipc_client
.type user_ipc_client, @function
user_ipc_client:
    li a0, 1
    li a1, 0x1234
    li a7, 2
    ecall
    li a0, 0
    li a7, 1
    ecall
    ret

.global user_ipc_server
.type user_ipc_server, @function
user_ipc_server:
    li a0, 1
    li a7, 3
    ecall
    slli a1, a0, 1
    li a0, 1
    li a7, 4
    ecall
1:
    j 1b

.section .text, "ax"
.balign 4

.macro save_kernel_callee
    addi sp, sp, -112
    sd ra, 0(sp)
    sd s0, 8(sp)
    sd s1, 16(sp)
    sd s2, 24(sp)
    sd s3, 32(sp)
    sd s4, 40(sp)
    sd s5, 48(sp)
    sd s6, 56(sp)
    sd s7, 64(sp)
    sd s8, 72(sp)
    sd s9, 80(sp)
    sd s10, 88(sp)
    sd s11, 96(sp)
    sd gp, 104(sp)
    csrr t6, sscratch
    sd sp, 24(t6)
.endm

.macro restore_kernel_callee
    ld ra, 0(sp)
    ld s0, 8(sp)
    ld s1, 16(sp)
    ld s2, 24(sp)
    ld s3, 32(sp)
    ld s4, 40(sp)
    ld s5, 48(sp)
    ld s6, 56(sp)
    ld s7, 64(sp)
    ld s8, 72(sp)
    ld s9, 80(sp)
    ld s10, 88(sp)
    ld s11, 96(sp)
    ld gp, 104(sp)
    addi sp, sp, 112
.endm

.global enter_user_with_stack
.type enter_user_with_stack, @function
enter_user_with_stack:
    save_kernel_callee
    mv t0, a0
    mv t1, a1
    ld a0, 0(a2)
    ld a1, 8(a2)
    ld a2, 16(a2)
    ld a3, 24(a2)
    ld a4, 32(a2)
    ld a5, 40(a2)
    ld a6, 48(a2)
    csrw sepc, t0
    mv sp, t1
    j enter_user_return

.global enter_user_service
.type enter_user_service, @function
enter_user_service:
    save_kernel_callee
    mv t0, a0
    mv a0, a1
    mv a1, a2
    mv a2, a3
    mv a3, a4
    mv a4, a5
    mv a5, a6
    mv a6, a7
    csrw sepc, t0
    li sp, USER_STACK_TOP
    j enter_user_return

.global enter_user_rr
.type enter_user_rr, @function
enter_user_rr:
    j enter_user_service

enter_user_return:
    csrr t1, sstatus
    li t2, SSTATUS_SPP | SSTATUS_SIE
    not t2, t2
    and t1, t1, t2
    li t2, SSTATUS_SPIE
    or t1, t1, t2
    csrw sstatus, t1
    sret

.global riscv_user_resume
.type riscv_user_resume, @function
riscv_user_resume:
    restore_kernel_callee
    ret

.global enter_user_demo
.type enter_user_demo, @function
enter_user_demo:
    li a0, USER_ENTRY_VA
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    li a5, 0
    li a6, 0
    li a7, 0
    j enter_user_service

.global enter_user_fault_demo
.type enter_user_fault_demo, @function
enter_user_fault_demo:
    mv t6, ra
    li a0, USER_ENTRY_VA
    call riscv_user_fault_entry_physical
    mv a1, a0
    call riscv_user_entry_physical
    sub a1, a1, a0
    li a0, USER_ENTRY_VA
    add a0, a0, a1
    mv ra, t6
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    li a5, 0
    li a6, 0
    li a7, 0
    j enter_user_service

.global enter_user_smp_demo
.type enter_user_smp_demo, @function
enter_user_smp_demo:
    mv t6, ra
    li a0, USER_ENTRY_VA
    call riscv_user_smp_entry_physical
    mv a1, a0
    call riscv_user_entry_physical
    sub a1, a1, a0
    li a0, USER_ENTRY_VA
    add a0, a0, a1
    mv ra, t6
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    li a5, 0
    li a6, 0
    li a7, 0
    j enter_user_service

// The kernel is linked in the high half while the bootstrap/user text and
// scratch storage remain in the physical boot segment.  These helpers keep
// those cross-segment addresses out of Rust PC-relative relocations.
.section .text, "ax"
.balign 4

.global riscv_trap_scratch_physical
.type riscv_trap_scratch_physical, @function
riscv_trap_scratch_physical:
    la t0, riscv_trap_scratch_pointer
    ld a0, 0(t0)
    ret

.global riscv_secondary_entry_physical
.type riscv_secondary_entry_physical, @function
riscv_secondary_entry_physical:
    la t0, riscv_secondary_entry_pointer
    ld a0, 0(t0)
    ret

.global riscv_user_entry_physical
.type riscv_user_entry_physical, @function
riscv_user_entry_physical:
    la t0, riscv_user_entry_pointer
    ld a0, 0(t0)
    ret

.global riscv_user_rr_entry_physical
.type riscv_user_rr_entry_physical, @function
riscv_user_rr_entry_physical:
    la t0, riscv_user_rr_entry_pointer
    ld a0, 0(t0)
    ret

.global riscv_user_fault_entry_physical
.type riscv_user_fault_entry_physical, @function
riscv_user_fault_entry_physical:
    la t0, riscv_user_fault_entry_pointer
    ld a0, 0(t0)
    ret

.global riscv_user_smp_entry_physical
.type riscv_user_smp_entry_physical, @function
riscv_user_smp_entry_physical:
    la t0, riscv_user_smp_entry_pointer
    ld a0, 0(t0)
    ret

.global riscv_user_rr_exit_physical
.type riscv_user_rr_exit_physical, @function
riscv_user_rr_exit_physical:
    la t0, riscv_user_rr_exit_pointer
    ld a0, 0(t0)
    ret

.global riscv_user_ipc_client_physical
.type riscv_user_ipc_client_physical, @function
riscv_user_ipc_client_physical:
    la t0, riscv_user_ipc_client_pointer
    ld a0, 0(t0)
    ret

.global riscv_user_ipc_server_physical
.type riscv_user_ipc_server_physical, @function
riscv_user_ipc_server_physical:
    la t0, riscv_user_ipc_server_pointer
    ld a0, 0(t0)
    ret

.section .rodata, "a"
.balign 8
riscv_trap_scratch_pointer:
    .dword trap_scratch
riscv_secondary_entry_pointer:
    .dword secondary_entry
riscv_user_entry_pointer:
    .dword user_entry
riscv_user_rr_entry_pointer:
    .dword user_rr_entry
riscv_user_fault_entry_pointer:
    .dword user_fault_entry
riscv_user_smp_entry_pointer:
    .dword user_smp_entry
riscv_user_rr_exit_pointer:
    .dword user_rr_exit
riscv_user_ipc_client_pointer:
    .dword user_ipc_client
riscv_user_ipc_server_pointer:
    .dword user_ipc_server

.option pop
