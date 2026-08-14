#![no_std]

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Ping = 0,
    Write = 1,
    Read = 2,
}

pub const INLINE_BYTES: usize = 32;

pub struct ByteRing<const N: usize> {
    bytes: [u8; N],
    head: usize,
    len: usize,
    dropped: u64,
}

impl<const N: usize> ByteRing<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }
    pub fn push(&mut self, byte: u8) -> bool {
        if self.len == N || N == 0 {
            self.dropped += 1;
            return false;
        }
        self.bytes[(self.head + self.len) % N] = byte;
        self.len += 1;
        true
    }
    pub fn pop(&mut self) -> Option<u8> {
        if self.len == 0 || N == 0 {
            return None;
        }
        let byte = self.bytes[self.head];
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Some(byte)
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl<const N: usize> Default for ByteRing<N> {
    fn default() -> Self {
        Self::new()
    }
}
