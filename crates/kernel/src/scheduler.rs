use microsystem_abi::Status;

pub const MAX_THREADS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadState {
    Empty,
    Ready,
    Running,
    Blocked,
    Exited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Thread {
    pub id: u32,
    pub task: u32,
    pub state: ThreadState,
    pub cpu: Option<u8>,
    pub quantum_ticks: u32,
}

const EMPTY_THREAD: Thread = Thread {
    id: 0,
    task: 0,
    state: ThreadState::Empty,
    cpu: None,
    quantum_ticks: 0,
};

pub struct Scheduler {
    threads: [Thread; MAX_THREADS],
    queue: [u32; MAX_THREADS],
    head: usize,
    len: usize,
    next_id: u32,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            threads: [EMPTY_THREAD; MAX_THREADS],
            queue: [0; MAX_THREADS],
            head: 0,
            len: 0,
            next_id: 1,
        }
    }

    pub fn create(&mut self, task: u32) -> Result<u32, Status> {
        let index = self
            .threads
            .iter()
            .position(|thread| thread.state == ThreadState::Empty)
            .ok_or(Status::NoMemory)?;
        if self.len == MAX_THREADS {
            return Err(Status::Busy);
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.threads[index] = Thread {
            id,
            task,
            state: ThreadState::Ready,
            cpu: None,
            quantum_ticks: 0,
        };
        self.enqueue(id)?;
        Ok(id)
    }

    pub fn schedule(&mut self, cpu: u8) -> Option<u32> {
        while self.len > 0 {
            let id = self.dequeue()?;
            if let Some(thread) = self.find_mut(id) {
                if thread.state == ThreadState::Ready {
                    thread.state = ThreadState::Running;
                    thread.cpu = Some(cpu);
                    thread.quantum_ticks = 0;
                    return Some(id);
                }
            }
        }
        None
    }

    pub fn preempt(&mut self, id: u32) -> Result<(), Status> {
        if self.find_mut(id).ok_or(Status::NotFound)?.state != ThreadState::Running {
            return Err(Status::Invalid);
        }
        self.enqueue(id)?;
        let thread = self.find_mut(id).ok_or(Status::NotFound)?;
        thread.state = ThreadState::Ready;
        thread.cpu = None;
        Ok(())
    }

    pub fn block(&mut self, id: u32) -> Result<(), Status> {
        let state = self.find_mut(id).ok_or(Status::NotFound)?.state;
        if !matches!(state, ThreadState::Ready | ThreadState::Running) {
            return Err(Status::Invalid);
        }
        if state == ThreadState::Ready {
            self.remove_queued(id);
        }
        let thread = self.find_mut(id).ok_or(Status::NotFound)?;
        thread.state = ThreadState::Blocked;
        thread.cpu = None;
        Ok(())
    }

    pub fn wake(&mut self, id: u32) -> Result<(), Status> {
        if self.find_mut(id).ok_or(Status::NotFound)?.state != ThreadState::Blocked {
            return Err(Status::Invalid);
        }
        self.enqueue(id)?;
        let thread = self.find_mut(id).ok_or(Status::NotFound)?;
        thread.state = ThreadState::Ready;
        Ok(())
    }

    pub fn thread(&self, id: u32) -> Option<&Thread> {
        self.threads
            .iter()
            .find(|thread| thread.id == id && thread.state != ThreadState::Empty)
    }

    fn find_mut(&mut self, id: u32) -> Option<&mut Thread> {
        self.threads
            .iter_mut()
            .find(|thread| thread.id == id && thread.state != ThreadState::Empty)
    }
    fn enqueue(&mut self, id: u32) -> Result<(), Status> {
        if self.len == MAX_THREADS {
            return Err(Status::Busy);
        }
        self.queue[(self.head + self.len) % MAX_THREADS] = id;
        self.len += 1;
        Ok(())
    }
    fn dequeue(&mut self) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        let id = self.queue[self.head];
        self.head = (self.head + 1) % MAX_THREADS;
        self.len -= 1;
        Some(id)
    }

    fn remove_queued(&mut self, id: u32) {
        let mut retained = 0;
        for read in 0..self.len {
            let queued = self.queue[(self.head + read) % MAX_THREADS];
            if queued != id {
                self.queue[(self.head + retained) % MAX_THREADS] = queued;
                retained += 1;
            }
        }
        self.len = retained;
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}
