.section .text, "ax"
.code64

.global x86_reload_segments
x86_reload_segments:
    pushq $0x08
    lea 1f(%rip), %rax
    push %rax
    lretq
1:
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    ret

.macro push_registers
    push %r15
    push %r14
    push %r13
    push %r12
    push %r11
    push %rbp
    push %rax
    push %rcx
    push %rbx
    push %r9
    push %r8
    push %r10
    push %rdx
    push %rsi
    push %rdi
.endm

.macro pop_registers
    pop %rdi
    pop %rsi
    pop %rdx
    pop %r10
    pop %r8
    pop %r9
    pop %rbx
    pop %rcx
    pop %rax
    pop %rbp
    pop %r11
    pop %r12
    pop %r13
    pop %r14
    pop %r15
.endm

.macro no_error_handler name, vector
.global \name
\name:
    pushq $0
    pushq $\vector
    jmp x86_common_trap
.endm

.macro error_handler name, vector
.global \name
\name:
    pushq $\vector
    jmp x86_common_trap
.endm

no_error_handler x86_int80, 0x80
no_error_handler x86_timer, 0x20
no_error_handler x86_reschedule, 0xf1
no_error_handler x86_device, 0x31
no_error_handler x86_invalid_opcode, 6
no_error_handler x86_spurious, 0xff
no_error_handler x86_default_exception, 0xfe
error_handler x86_double_fault, 8
error_handler x86_invalid_tss, 10
error_handler x86_segment_not_present, 11
error_handler x86_stack_segment_fault, 12
error_handler x86_general_protection, 13
error_handler x86_page_fault, 14
error_handler x86_alignment_check, 17
error_handler x86_control_protection, 21
error_handler x86_vmm_communication, 29
error_handler x86_security_exception, 30

x86_common_trap:
    cld
    push_registers
    mov %rsp, %rdi
    mov %rsp, %r12
    and $-16, %rsp
    call x86_handle_trap
    mov %r12, %rsp
    cmp $1, %rax
    je x86_resume_kernel
    pop_registers
    add $16, %rsp
    iretq

x86_resume_kernel:
    movabs $0xffff8000fee00020, %rax
    mov (%rax), %eax
    shr $24, %eax
    and $1, %eax
    lea saved_kernel_rsp(%rip), %rdx
    mov (%rdx,%rax,8), %rsp
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %rbp
    pop %rbx
    sti
    ret

.macro save_kernel_call
    push %rbx
    push %rbp
    push %r12
    push %r13
    push %r14
    push %r15
    movabs $0xffff8000fee00020, %rax
    mov (%rax), %eax
    shr $24, %eax
    and $1, %eax
    lea saved_kernel_rsp(%rip), %rdx
    mov %rsp, (%rdx,%rax,8)
.endm

.macro enter_user entry, stack
    mov $0x23, %eax
    push %rax
    push \stack
    pushq $0x202
    mov $0x1b, %eax
    push %rax
    push \entry
    iretq
.endm

x86_reset_fp_simd:
    # FNINIT marks the x87 stack empty but does not erase its physical payload.
    fninit
    fldz
    fldz
    fldz
    fldz
    fldz
    fldz
    fldz
    fldz
    fninit
    ldmxcsr x86_default_mxcsr(%rip)
    pxor %xmm0, %xmm0
    pxor %xmm1, %xmm1
    pxor %xmm2, %xmm2
    pxor %xmm3, %xmm3
    pxor %xmm4, %xmm4
    pxor %xmm5, %xmm5
    pxor %xmm6, %xmm6
    pxor %xmm7, %xmm7
    pxor %xmm8, %xmm8
    pxor %xmm9, %xmm9
    pxor %xmm10, %xmm10
    pxor %xmm11, %xmm11
    pxor %xmm12, %xmm12
    pxor %xmm13, %xmm13
    pxor %xmm14, %xmm14
    pxor %xmm15, %xmm15
    ret

.global enter_user_demo
enter_user_demo:
    save_kernel_call
    call x86_reset_fp_simd
    xor %rdi, %rdi
    xor %rsi, %rsi
    xor %rdx, %rdx
    xor %r10, %r10
    xor %r8, %r8
    xor %r9, %r9
    mov $0x00400000, %r11
    mov $0x00600000, %r12
    enter_user %r11, %r12

.global enter_user_fault_demo
enter_user_fault_demo:
    save_kernel_call
    call x86_reset_fp_simd
    movabs $user_fault_entry_high, %r11
    movabs $user_entry_high, %r12
    sub %r12, %r11
    add $0x00400000, %r11
    mov $0x00600000, %r12
    enter_user %r11, %r12

.global enter_user_smp_demo
enter_user_smp_demo:
    save_kernel_call
    call x86_reset_fp_simd
    movabs $user_smp_entry, %r11
    movabs $user_entry, %r12
    sub %r12, %r11
    add $0x00400000, %r11
    mov $0x00600000, %r12
    enter_user %r11, %r12

.global enter_user_rr
enter_user_rr:
    save_kernel_call
    call x86_reset_fp_simd
    mov $0x11, %eax
    movq %rax, %xmm0
    mov %rdi, %r11
    mov %rsi, %rdi
    mov $0x00600000, %r12
    enter_user %r11, %r12

.global enter_user_service
enter_user_service:
    mov 8(%rsp), %rax
    mov 16(%rsp), %r11
    mov %rdi, %r12
    mov %rsi, %rdi
    mov %rdx, %rsi
    mov %rcx, %rdx
    mov %r8, %rcx
    mov %r9, %r8
    mov %rax, %r9
    save_kernel_call
    call x86_reset_fp_simd
    mov $0x00800000, %rbx
    sub $24, %rbx
    movq $0, (%rbx)
    mov %r11, 8(%rbx)
    enter_user %r12, %rbx

.global enter_user_with_stack
enter_user_with_stack:
    mov %rdi, %r12
    mov %rsi, %rbx
    mov %rdx, %r11
    mov 0(%r11), %rdi
    mov 8(%r11), %rsi
    mov 16(%r11), %rdx
    mov 24(%r11), %rcx
    mov 32(%r11), %r8
    mov 40(%r11), %r9
    mov 48(%r11), %r11
    save_kernel_call
    call x86_reset_fp_simd
    sub $24, %rbx
    movq $0, (%rbx)
    mov %r11, 8(%rbx)
    enter_user %r12, %rbx

.section .bss, "aw", @nobits
.balign 16
saved_kernel_rsp:
    .skip 16

.section .rodata, "a"
.balign 4
x86_default_mxcsr:
    .long 0x1f80
