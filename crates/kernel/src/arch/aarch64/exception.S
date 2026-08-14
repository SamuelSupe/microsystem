.arch armv8-a+fp+simd
.section .text, "ax"
.equ EXCEPTION_FRAME_SIZE, 256

.macro save_lower_context
    sub sp, sp, #EXCEPTION_FRAME_SIZE
    stp x0, x1, [sp, #0]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    stp x8, x9, [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    str x30, [sp, #240]
.endm

.macro restore_lower_context
    ldp x0, x1, [sp, #0]
    ldp x2, x3, [sp, #16]
    ldp x4, x5, [sp, #32]
    ldp x6, x7, [sp, #48]
    ldp x8, x9, [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldp x16, x17, [sp, #128]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30, [sp, #240]
    add sp, sp, #EXCEPTION_FRAME_SIZE
.endm

.balign 2048
.global vectors
vectors:
    b exception_current_sp0_sync
    .balign 128
    b exception_current_sp0_irq
    .balign 128
    b exception_unhandled
    .balign 128
    b exception_unhandled
    .balign 128
    b exception_current_spx_sync
    .balign 128
    b exception_current_spx_irq
    .balign 128
    b exception_unhandled
    .balign 128
    b exception_unhandled
    .balign 128
    b exception_lower_sync
    .balign 128
    b exception_lower_irq
    .balign 128
    b exception_unhandled
    .balign 128
    b exception_unhandled
    .balign 128
    b exception_unhandled
    .balign 128
    b exception_unhandled
    .balign 128
    b exception_unhandled
    .balign 128
    b exception_unhandled

exception_current_sp0_sync:
exception_current_spx_sync:
exception_unhandled:
    sub sp, sp, #32
    stp x0, x1, [sp]
    stp x29, x30, [sp, #16]
    mrs x0, esr_el1
    mrs x1, elr_el1
    mrs x2, far_el1
    bl rust_exception
1:
    wfe
    b 1b

exception_current_sp0_irq:
exception_current_spx_irq:
exception_lower_irq:
    save_lower_context
    mov x0, sp
    bl rust_irq
    restore_lower_context
    eret

exception_lower_sync:
    save_lower_context
    mov x0, sp
    mrs x1, esr_el1
    mrs x2, elr_el1
    mrs x3, far_el1
    bl rust_lower_sync
    cmp x0, #1
    b.eq 3f
    cmp x0, #2
    b.eq 2f
2:
    restore_lower_context
    eret
3:
    ldr x9, =user_demo_resume
    msr elr_el1, x9
    mov x9, #0x5
    msr spsr_el1, x9
    add sp, sp, #EXCEPTION_FRAME_SIZE
    eret

.macro save_kernel_callee
    sub sp, sp, #96
    stp x19, x20, [sp, #0]
    stp x21, x22, [sp, #16]
    stp x23, x24, [sp, #32]
    stp x25, x26, [sp, #48]
    stp x27, x28, [sp, #64]
    stp x29, x30, [sp, #80]
.endm

.macro restore_kernel_callee
    ldp x19, x20, [sp, #0]
    ldp x21, x22, [sp, #16]
    ldp x23, x24, [sp, #32]
    ldp x25, x26, [sp, #48]
    ldp x27, x28, [sp, #64]
    ldp x29, x30, [sp, #80]
    add sp, sp, #96
.endm

reset_fp_simd_registers:
    movi v0.16b, #0
    movi v1.16b, #0
    movi v2.16b, #0
    movi v3.16b, #0
    movi v4.16b, #0
    movi v5.16b, #0
    movi v6.16b, #0
    movi v7.16b, #0
    movi v8.16b, #0
    movi v9.16b, #0
    movi v10.16b, #0
    movi v11.16b, #0
    movi v12.16b, #0
    movi v13.16b, #0
    movi v14.16b, #0
    movi v15.16b, #0
    movi v16.16b, #0
    movi v17.16b, #0
    movi v18.16b, #0
    movi v19.16b, #0
    movi v20.16b, #0
    movi v21.16b, #0
    movi v22.16b, #0
    movi v23.16b, #0
    movi v24.16b, #0
    movi v25.16b, #0
    movi v26.16b, #0
    movi v27.16b, #0
    movi v28.16b, #0
    movi v29.16b, #0
    movi v30.16b, #0
    movi v31.16b, #0
    msr fpcr, xzr
    msr fpsr, xzr
    ret

.global save_fp_simd_context
save_fp_simd_context:
    stp q0, q1, [x0, #0]
    stp q2, q3, [x0, #32]
    stp q4, q5, [x0, #64]
    stp q6, q7, [x0, #96]
    stp q8, q9, [x0, #128]
    stp q10, q11, [x0, #160]
    stp q12, q13, [x0, #192]
    stp q14, q15, [x0, #224]
    stp q16, q17, [x0, #256]
    stp q18, q19, [x0, #288]
    stp q20, q21, [x0, #320]
    stp q22, q23, [x0, #352]
    stp q24, q25, [x0, #384]
    stp q26, q27, [x0, #416]
    stp q28, q29, [x0, #448]
    stp q30, q31, [x0, #480]
    mrs x1, fpcr
    mrs x2, fpsr
    str x1, [x0, #512]
    str x2, [x0, #520]
    ret

.global load_fp_simd_context
load_fp_simd_context:
    ldr x1, [x0, #512]
    ldr x2, [x0, #520]
    msr fpcr, x1
    msr fpsr, x2
    ldp q0, q1, [x0, #0]
    ldp q2, q3, [x0, #32]
    ldp q4, q5, [x0, #64]
    ldp q6, q7, [x0, #96]
    ldp q8, q9, [x0, #128]
    ldp q10, q11, [x0, #160]
    ldp q12, q13, [x0, #192]
    ldp q14, q15, [x0, #224]
    ldp q16, q17, [x0, #256]
    ldp q18, q19, [x0, #288]
    ldp q20, q21, [x0, #320]
    ldp q22, q23, [x0, #352]
    ldp q24, q25, [x0, #384]
    ldp q26, q27, [x0, #416]
    ldp q28, q29, [x0, #448]
    ldp q30, q31, [x0, #480]
    ret

.global enter_user_demo
enter_user_demo:
    save_kernel_callee
    bl reset_fp_simd_registers
    mov x9, #0x00600000
    msr sp_el0, x9
    mov x9, #0x00400000
    msr elr_el1, x9
    msr spsr_el1, xzr
    eret

.global enter_user_fault_demo
enter_user_fault_demo:
    save_kernel_callee
    bl reset_fp_simd_registers
    mov x9, #0x00600000
    msr sp_el0, x9
    mov x9, #0x00400000
    add x9, x9, #(user_fault_entry - user_entry)
    msr elr_el1, x9
    msr spsr_el1, xzr
    eret

.global enter_user_smp_demo
enter_user_smp_demo:
    save_kernel_callee
    bl reset_fp_simd_registers
    mov x9, #0x00600000
    msr sp_el0, x9
    mov x9, #0x00400000
    add x9, x9, #(user_smp_entry - user_entry)
    msr elr_el1, x9
    msr spsr_el1, xzr
    eret

.global enter_user_rr
enter_user_rr:
    save_kernel_callee
    bl reset_fp_simd_registers
    mov x9, #0x00600000
    msr sp_el0, x9
    msr elr_el1, x0
    mov x0, x1
    mov x1, x2
    mov x2, x3
    mov x3, x4
    mov x4, x5
    mov x5, x6
    mov x6, x7
    msr spsr_el1, xzr
    eret

.global enter_user_service
enter_user_service:
    save_kernel_callee
    bl reset_fp_simd_registers
    mov x9, #0x00800000
    msr sp_el0, x9
    msr elr_el1, x0
    mov x0, x1
    mov x1, x2
    mov x2, x3
    mov x3, x4
    mov x4, x5
    mov x5, x6
    mov x6, x7
    msr spsr_el1, xzr
    eret
user_demo_resume:
    restore_kernel_callee
    ret

.section .rodata, "a"
.balign 8
.global secondary_entry_phys_value
secondary_entry_phys_value:
    .quad secondary_entry
