#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod compiler;
mod lexer;
mod permission;
mod value;
mod vm;

pub mod json;
pub mod stdlib;

pub use compiler::{Chunk, CompileError, Op};
pub use permission::{Access, Permission, PermissionSet, Resource};
pub use value::{ErrorValue, Value};
pub use vm::{Host, Limits, Vm, VmError, VmOutcome};

pub const LANGUAGE_VERSION: u16 = 1;
pub const SOURCE_LIMIT: usize = 64 * 1024;
pub const MODULE_SOURCE_LIMIT: usize = 256 * 1024;
pub const BYTECODE_LIMIT: usize = 128 * 1024;
pub const CALL_DEPTH_LIMIT: usize = 128;
pub const OPERAND_STACK_LIMIT: usize = 4096;
pub const VM_HEAP_LIMIT: usize = 512 * 1024;
pub const DEFAULT_INSTRUCTION_LIMIT: u64 = 10_000_000;
pub const DEFAULT_TIMEOUT_NS: u64 = 10_000_000_000;
pub const REPL_TIMEOUT_NS: u64 = 5_000_000_000;
pub const YIELD_INTERVAL: u64 = 4096;

pub fn compile(source: &str) -> Result<Chunk, CompileError> {
    if source.len() > SOURCE_LIMIT {
        return Err(CompileError::new(0, 0, "source exceeds 64 KiB"));
    }
    compiler::compile(source)
}

pub fn compile_with_prelude(prelude: &str, source: &str) -> Result<Chunk, CompileError> {
    if source.len() > SOURCE_LIMIT {
        return Err(CompileError::new(0, 0, "source exceeds 64 KiB"));
    }
    let mut combined =
        alloc::string::String::with_capacity(prelude.len().saturating_add(source.len()));
    combined.push_str(prelude);
    combined.push_str(source);
    compiler::compile(&combined)
}
