use microsystem_abi::{Message, Status};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Envelope {
    pub sender: u32,
    pub message: Message,
}

pub struct EndpointQueue<const N: usize> {
    entries: [Option<Envelope>; N],
    head: usize,
    len: usize,
}

impl<const N: usize> EndpointQueue<N> {
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            head: 0,
            len: 0,
        }
    }

    pub fn send(&mut self, envelope: Envelope) -> Result<(), Status> {
        if self.len == N || N == 0 {
            return Err(Status::Busy);
        }
        let index = (self.head + self.len) % N;
        self.entries[index] = Some(envelope);
        self.len += 1;
        Ok(())
    }

    pub fn receive(&mut self) -> Option<Envelope> {
        if self.len == 0 || N == 0 {
            return None;
        }
        let envelope = self.entries[self.head].take();
        self.head = (self.head + 1) % N;
        self.len -= 1;
        envelope
    }

    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> Default for EndpointQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}
