.option push
.option arch, rv64gc

.section .text.trap, "ax"
.balign 4

.equ FRAME_SIZE, 288
.equ SEPC_OFFSET, 248
.equ SSTATUS_OFFSET, 256
.equ SCAUSE_OFFSET, 264
.equ STVAL_OFFSET, 272
.equ SCRATCH_OFFSET, 280
.equ SSTATUS_SIE, (1 << 1)

.global trap_vector
.type trap_vector, @function
trap_vector:
    // sscratch points at a per-hart record containing a dedicated trap stack,
    // hart id, saved interrupted stack pointer, and interrupted t6. Restore
    // sscratch before entering Rust so a synchronous kernel fault reports its
    // real origin instead of treating the interrupted t6 as a scratch pointer.
    csrrw t6, sscratch, t6
    sd sp, 16(t6)
    csrrw sp, sscratch, t6
    sd sp, 32(t6)
    ld sp, 0(t6)
    addi sp, sp, -FRAME_SIZE

    // The Rust frame keeps the syscall ABI first, matching the AArch64
    // backend's direct registers[0..] contract.
    sd a0, 0(sp)
    sd a1, 8(sp)
    sd a2, 16(sp)
    sd a3, 24(sp)
    sd a4, 32(sp)
    sd a5, 40(sp)
    sd a6, 48(sp)
    sd a7, 56(sp)
    sd ra, 64(sp)
    sd gp, 80(sp)
    sd tp, 88(sp)
    sd t0, 96(sp)
    sd t1, 104(sp)
    sd t2, 112(sp)
    sd s0, 120(sp)
    sd s1, 128(sp)
    sd s2, 136(sp)
    sd s3, 144(sp)
    sd s4, 152(sp)
    sd s5, 160(sp)
    sd s6, 168(sp)
    sd s7, 176(sp)
    sd s8, 184(sp)
    sd s9, 192(sp)
    sd s10, 200(sp)
    sd s11, 208(sp)
    sd t3, 216(sp)
    sd t4, 224(sp)
    sd t5, 232(sp)

    // The old x2 and x31 are in the per-hart scratch record.
    ld t0, 16(t6)
    sd t0, 72(sp)
    ld t0, 32(t6)
    sd t0, 240(sp)
    sd t6, SCRATCH_OFFSET(sp)

    csrr t0, sepc
    sd t0, SEPC_OFFSET(sp)
    csrr t0, sstatus
    sd t0, SSTATUS_OFFSET(sp)
    csrr t0, scause
    sd t0, SCAUSE_OFFSET(sp)
    csrr t0, stval
    sd t0, STVAL_OFFSET(sp)

    // Rust code must see the kernel hart id even when the interrupted code
    // was U-mode and owned tp.
    ld tp, 8(t6)
    mv a0, sp
    call rust_trap

    ld t0, SEPC_OFFSET(sp)
    csrw sepc, t0
    // Keep SIE clear during the restore. sret will derive it from SPIE.
    ld t0, SSTATUS_OFFSET(sp)
    li t1, SSTATUS_SIE
    not t1, t1
    and t0, t0, t1
    csrw sstatus, t0

    // Action 1 returns from a built-in U-mode probe to its saved S-mode
    // caller. The marker uses the low bit of the 32-byte-aligned scratch
    // pointer; ordinary interrupt and syscall returns keep the full pointer.
    ld t0, SCRATCH_OFFSET(sp)
    andi t1, t0, 1
    bnez t1, trap_return_kernel

    ld a0, 0(sp)
    ld a1, 8(sp)
    ld a2, 16(sp)
    ld a3, 24(sp)
    ld a4, 32(sp)
    ld a5, 40(sp)
    ld a6, 48(sp)
    ld a7, 56(sp)
    ld ra, 64(sp)
    ld gp, 80(sp)
    ld tp, 88(sp)
    ld t0, 96(sp)
    ld t1, 104(sp)
    ld t2, 112(sp)
    ld s0, 120(sp)
    ld s1, 128(sp)
    ld s2, 136(sp)
    ld s3, 144(sp)
    ld s4, 152(sp)
    ld s5, 160(sp)
    ld s6, 168(sp)
    ld s7, 176(sp)
    ld s8, 184(sp)
    ld s9, 192(sp)
    ld s10, 200(sp)
    ld s11, 208(sp)
    ld t3, 216(sp)
    ld t4, 224(sp)
    ld t5, 232(sp)

    // t0 has already been restored above. Use t6, whose interrupted value is
    // still in the frame, so the trap epilogue cannot corrupt a user GPR.
    ld t6, SCRATCH_OFFSET(sp)
    csrw sscratch, t6
    ld t6, 240(sp)
    ld sp, 72(sp)
    sret

trap_return_kernel:
    andi t0, t0, -2
    csrw sscratch, t0
    ld sp, 24(t0)
    sret

// A future kernel can provide a strong rust_trap implementation. The weak
// fallback keeps this backend linkable and implements only the explicit
// default contract in riscv_default_trap.
.weak rust_trap
.type rust_trap, @function
rust_trap:
    j riscv_default_trap

.section .text, "ax"
.balign 4

.global reset_fp_context
.type reset_fp_context, @function
reset_fp_context:
    fmv.d.x f0, zero
    fmv.d.x f1, zero
    fmv.d.x f2, zero
    fmv.d.x f3, zero
    fmv.d.x f4, zero
    fmv.d.x f5, zero
    fmv.d.x f6, zero
    fmv.d.x f7, zero
    fmv.d.x f8, zero
    fmv.d.x f9, zero
    fmv.d.x f10, zero
    fmv.d.x f11, zero
    fmv.d.x f12, zero
    fmv.d.x f13, zero
    fmv.d.x f14, zero
    fmv.d.x f15, zero
    fmv.d.x f16, zero
    fmv.d.x f17, zero
    fmv.d.x f18, zero
    fmv.d.x f19, zero
    fmv.d.x f20, zero
    fmv.d.x f21, zero
    fmv.d.x f22, zero
    fmv.d.x f23, zero
    fmv.d.x f24, zero
    fmv.d.x f25, zero
    fmv.d.x f26, zero
    fmv.d.x f27, zero
    fmv.d.x f28, zero
    fmv.d.x f29, zero
    fmv.d.x f30, zero
    fmv.d.x f31, zero
    csrw fcsr, zero
    ret

.global save_fp_context
.global save_fp_simd_context
.type save_fp_context, @function
save_fp_context:
save_fp_simd_context:
    fsd f0, 0(a0)
    fsd f1, 8(a0)
    fsd f2, 16(a0)
    fsd f3, 24(a0)
    fsd f4, 32(a0)
    fsd f5, 40(a0)
    fsd f6, 48(a0)
    fsd f7, 56(a0)
    fsd f8, 64(a0)
    fsd f9, 72(a0)
    fsd f10, 80(a0)
    fsd f11, 88(a0)
    fsd f12, 96(a0)
    fsd f13, 104(a0)
    fsd f14, 112(a0)
    fsd f15, 120(a0)
    fsd f16, 128(a0)
    fsd f17, 136(a0)
    fsd f18, 144(a0)
    fsd f19, 152(a0)
    fsd f20, 160(a0)
    fsd f21, 168(a0)
    fsd f22, 176(a0)
    fsd f23, 184(a0)
    fsd f24, 192(a0)
    fsd f25, 200(a0)
    fsd f26, 208(a0)
    fsd f27, 216(a0)
    fsd f28, 224(a0)
    fsd f29, 232(a0)
    fsd f30, 240(a0)
    fsd f31, 248(a0)
    csrr t0, fcsr
    sd t0, 256(a0)
    ret

.global load_fp_context
.global load_fp_simd_context
.type load_fp_context, @function
load_fp_context:
load_fp_simd_context:
    ld t0, 256(a0)
    csrw fcsr, t0
    fld f0, 0(a0)
    fld f1, 8(a0)
    fld f2, 16(a0)
    fld f3, 24(a0)
    fld f4, 32(a0)
    fld f5, 40(a0)
    fld f6, 48(a0)
    fld f7, 56(a0)
    fld f8, 64(a0)
    fld f9, 72(a0)
    fld f10, 80(a0)
    fld f11, 88(a0)
    fld f12, 96(a0)
    fld f13, 104(a0)
    fld f14, 112(a0)
    fld f15, 120(a0)
    fld f16, 128(a0)
    fld f17, 136(a0)
    fld f18, 144(a0)
    fld f19, 152(a0)
    fld f20, 160(a0)
    fld f21, 168(a0)
    fld f22, 176(a0)
    fld f23, 184(a0)
    fld f24, 192(a0)
    fld f25, 200(a0)
    fld f26, 208(a0)
    fld f27, 216(a0)
    fld f28, 224(a0)
    fld f29, 232(a0)
    fld f30, 240(a0)
    fld f31, 248(a0)
    ret

.option pop
