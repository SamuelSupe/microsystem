use microsystem_abi::{Message, Status};

/// One asynchronous call per task. A completed reply remains owned by the
/// caller until acknowledged; a second call cannot overwrite it.
#[derive(Clone, Copy)]
pub enum ReplySlot {
    Idle,
    Pending(u32),
    Ready(Message),
    Failed(Status),
}

impl ReplySlot {
    pub fn begin(&mut self, server: u32) -> Result<(), Status> {
        if !matches!(self, Self::Idle) {
            return Err(Status::Busy);
        }
        *self = Self::Pending(server);
        Ok(())
    }

    pub fn waiting_for(&self, server: u32) -> bool {
        matches!(self, Self::Pending(expected) if *expected == server)
    }

    pub fn complete(&mut self, server: u32, reply: Message) -> Result<(), Status> {
        if !self.waiting_for(server) {
            return Err(Status::AccessDenied);
        }
        *self = Self::Ready(reply);
        Ok(())
    }

    pub fn result(&self) -> Result<Message, Status> {
        match self {
            Self::Idle => Err(Status::NotFound),
            Self::Pending(_) => Err(Status::Busy),
            Self::Ready(message) => Ok(*message),
            Self::Failed(status) => Err(*status),
        }
    }
}

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
