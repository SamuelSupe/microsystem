use alloc::string::ToString;
use alloc::vec::Vec;

use crate::compiler::{Chunk, Op};
use crate::{
    CALL_DEPTH_LIMIT, DEFAULT_INSTRUCTION_LIMIT, DEFAULT_TIMEOUT_NS, ErrorValue,
    OPERAND_STACK_LIMIT, VM_HEAP_LIMIT, Value, YIELD_INTERVAL,
};

pub trait Host {
    fn resolve(&mut self, _name: &str) -> Option<Value> {
        None
    }

    fn call(&mut self, name: &str, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue>;

    fn now_ns(&mut self) -> u64;

    fn yield_now(&mut self) {}

    fn interrupted(&mut self) -> bool {
        false
    }

    fn gc_roots(&self) -> Vec<Value> {
        Vec::new()
    }
}

impl<H: Host + ?Sized> Host for &mut H {
    fn resolve(&mut self, name: &str) -> Option<Value> {
        (**self).resolve(name)
    }
    fn call(&mut self, name: &str, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        (**self).call(name, arguments)
    }
    fn now_ns(&mut self) -> u64 {
        (**self).now_ns()
    }
    fn yield_now(&mut self) {
        (**self).yield_now()
    }
    fn interrupted(&mut self) -> bool {
        (**self).interrupted()
    }

    fn gc_roots(&self) -> Vec<Value> {
        (**self).gc_roots()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub instructions: u64,
    pub timeout_ns: u64,
    pub stack: usize,
    pub call_depth: usize,
    pub heap_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            instructions: DEFAULT_INSTRUCTION_LIMIT,
            timeout_ns: DEFAULT_TIMEOUT_NS,
            stack: OPERAND_STACK_LIMIT,
            call_depth: CALL_DEPTH_LIMIT,
            heap_bytes: VM_HEAP_LIMIT,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum VmError {
    Runtime(ErrorValue),
    InstructionLimit,
    Timeout,
    Interrupted,
    StackOverflow,
    CallDepth,
    HeapLimit,
    InvalidBytecode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VmOutcome {
    pub values: Vec<Value>,
    pub instructions: u64,
}

#[derive(Clone)]
struct Closure {
    function: u16,
    captured: Vec<(u16, Value)>,
}

struct Frame {
    function: Option<u16>,
    closure: Option<u16>,
    ip: usize,
    environment: Vec<(u16, Value)>,
    stack_base: usize,
    expected_returns: u8,
    protected: bool,
}

pub struct Vm<'a, H> {
    chunk: &'a Chunk,
    host: H,
    limits: Limits,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    closures: Vec<Option<Closure>>,
    instructions: u64,
    start_ns: u64,
    heap_bytes: usize,
}

impl<'a, H: Host> Vm<'a, H> {
    pub fn new(chunk: &'a Chunk, mut host: H) -> Self {
        let start_ns = host.now_ns();
        Self {
            chunk,
            host,
            limits: Limits::default(),
            stack: Vec::new(),
            frames: alloc::vec![Frame {
                function: None,
                closure: None,
                ip: 0,
                environment: Vec::new(),
                stack_base: 0,
                expected_returns: u8::MAX,
                protected: false,
            }],
            closures: Vec::new(),
            instructions: 0,
            start_ns,
            heap_bytes: 0,
        }
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    pub fn run(mut self) -> Result<VmOutcome, VmError> {
        validate_chunk(self.chunk)?;
        loop {
            self.check_limits()?;
            let Some(frame) = self.frames.last_mut() else {
                return Err(VmError::InvalidBytecode);
            };
            let code = match frame.function {
                Some(index) => self
                    .chunk
                    .functions
                    .get(index as usize)
                    .map(|value| &value.code),
                None => Some(&self.chunk.code),
            }
            .ok_or(VmError::InvalidBytecode)?;
            let operation = code.get(frame.ip).cloned().unwrap_or(Op::Return(0));
            frame.ip += 1;
            self.instructions += 1;
            let result = match operation {
                Op::Return(count) => self.finish_frame(count),
                operation => self.execute_non_return(operation).map(|()| None),
            };
            match result {
                Ok(Some(outcome)) => return Ok(outcome),
                Ok(None) => {}
                Err(VmError::Runtime(error)) => {
                    if !self.catch_protected(error.clone())? {
                        return Err(VmError::Runtime(error));
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn finish_frame(&mut self, count: u8) -> Result<Option<VmOutcome>, VmError> {
        let frame = self.frames.pop().ok_or(VmError::InvalidBytecode)?;
        let count = count as usize;
        if self.stack.len() < frame.stack_base + count {
            return Err(VmError::InvalidBytecode);
        }
        let mut values = self.stack.split_off(self.stack.len() - count);
        self.stack.truncate(frame.stack_base);
        if let Some(handle) = frame.closure {
            let closure = self
                .closures
                .get_mut(handle as usize)
                .and_then(Option::as_mut)
                .ok_or(VmError::InvalidBytecode)?;
            closure.captured = frame.environment;
        }
        if self.frames.is_empty() {
            return Ok(Some(VmOutcome {
                values,
                instructions: self.instructions,
            }));
        }
        let expected = frame.expected_returns as usize - frame.protected as usize;
        values.truncate(expected);
        while values.len() < expected {
            values.push(Value::Nil);
        }
        if frame.protected {
            values.insert(0, Value::Bool(true));
        }
        for value in values {
            self.push(value)?;
        }
        Ok(None)
    }

    fn catch_protected(&mut self, error: ErrorValue) -> Result<bool, VmError> {
        let Some(index) = self.frames.iter().rposition(|frame| frame.protected) else {
            return Ok(false);
        };
        let frame = self.frames[index].stack_base;
        let expected = self.frames[index].expected_returns as usize;
        self.frames.truncate(index);
        self.stack.truncate(frame);
        let mut values = alloc::vec![Value::Bool(false), error_table(error)];
        values.truncate(expected);
        while values.len() < expected {
            values.push(Value::Nil);
        }
        for value in values {
            self.push(value)?;
        }
        Ok(true)
    }

    fn execute_non_return(&mut self, operation: Op) -> Result<(), VmError> {
        match operation {
            Op::Constant(index) => self.push(
                self.chunk
                    .constants
                    .get(index as usize)
                    .cloned()
                    .ok_or(VmError::InvalidBytecode)?,
            )?,
            Op::Nil => self.push(Value::Nil)?,
            Op::True => self.push(Value::Bool(true))?,
            Op::False => self.push(Value::Bool(false))?,
            Op::Load(name) => {
                let value = self.load(name)?;
                self.push(value)?;
            }
            Op::Define(name) => {
                let value = self.pop()?;
                self.define(name, value);
            }
            Op::Store(name) => {
                let value = self.pop()?;
                if !self.store(name, value) {
                    return Err(runtime("name", "assignment to undeclared variable"));
                }
            }
            Op::NewTable => self.push(Value::Table(Vec::new()))?,
            Op::Index => {
                let key = self.pop()?;
                let target = self.pop()?;
                self.push(index(&target, &key)?)?;
            }
            Op::SetIndex => {
                let value = self.pop()?;
                let key = self.pop()?;
                let mut target = self.pop()?;
                target.table_set(key, value).map_err(VmError::Runtime)?;
                self.push(target)?;
            }
            Op::MakeClosure(function) => {
                if self.chunk.functions.get(function as usize).is_none() {
                    return Err(VmError::InvalidBytecode);
                }
                let captured = self.frames.last().unwrap().environment.clone();
                let handle = self
                    .closures
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(self.closures.len());
                let handle = u16::try_from(handle).map_err(|_| VmError::HeapLimit)?;
                let closure = Closure { function, captured };
                if let Some(slot) = self.closures.get_mut(handle as usize) {
                    *slot = Some(closure);
                } else {
                    self.closures.push(Some(closure));
                }
                if let Err(error) = self.push(Value::Function(handle)) {
                    self.closures[handle as usize] = None;
                    return Err(error);
                }
            }
            Op::Add => self.numeric_binary(arithmetic_add)?,
            Op::Subtract => self.numeric_binary(arithmetic_subtract)?,
            Op::Multiply => self.numeric_binary(arithmetic_multiply)?,
            Op::Divide => self.numeric_binary(arithmetic_divide)?,
            Op::IntegerDivide => self.numeric_binary(arithmetic_integer_divide)?,
            Op::Remainder => self.numeric_binary(arithmetic_remainder)?,
            Op::Negate => {
                let value = self.pop()?;
                self.push(match value {
                    Value::Integer(value) => Value::Integer(
                        value
                            .checked_neg()
                            .ok_or_else(|| runtime("overflow", "integer overflow"))?,
                    ),
                    Value::Float(value) => Value::Float(-value),
                    _ => return Err(runtime("type", "unary '-' requires number")),
                })?;
            }
            Op::Not => {
                let value = self.pop()?;
                self.push(Value::Bool(!value.truthy()))?;
            }
            Op::Equal => self.compare(|a, b| a == b)?,
            Op::NotEqual => self.compare(|a, b| a != b)?,
            Op::Less => self.order(|o| o.is_lt())?,
            Op::LessEqual => self.order(|o| o.is_le())?,
            Op::Greater => self.order(|o| o.is_gt())?,
            Op::GreaterEqual => self.order(|o| o.is_ge())?,
            Op::And => {
                let r = self.pop()?;
                let l = self.pop()?;
                self.push(if l.truthy() { r } else { l })?;
            }
            Op::Or => {
                let r = self.pop()?;
                let l = self.pop()?;
                self.push(if l.truthy() { l } else { r })?;
            }
            Op::Dup => {
                let value = self.stack.last().cloned().ok_or(VmError::InvalidBytecode)?;
                self.push(value)?;
            }
            Op::Swap => {
                let n = self.stack.len();
                if n < 2 {
                    return Err(VmError::InvalidBytecode);
                }
                self.stack.swap(n - 1, n - 2);
            }
            Op::Pop => {
                self.pop()?;
            }
            Op::Call { arguments, returns } => self.call(arguments, returns)?,
            Op::Jump(target) => self.jump(target)?,
            Op::JumpIfFalse(target) => {
                let v = self.pop()?;
                if !v.truthy() {
                    self.jump(target)?;
                }
            }
            Op::Return(_) => unreachable!(),
        }
        Ok(())
    }

    fn call(&mut self, arguments: u8, returns: u8) -> Result<(), VmError> {
        let count = arguments as usize;
        if self.stack.len() < count + 1 {
            return Err(VmError::InvalidBytecode);
        }
        let args = self.stack.split_off(self.stack.len() - count);
        let callee = self.pop()?;
        if matches!(&callee, Value::Native(name) if name == "pcall") {
            let Some(target) = args.first().cloned() else {
                return Err(runtime("argument", "pcall requires a function"));
            };
            let protected_arguments = &args[1..];
            return match target {
                Value::Native(name) => {
                    let values = match self.builtin_or_host(&name, protected_arguments) {
                        Ok(mut values) => {
                            values.insert(0, Value::Bool(true));
                            values
                        }
                        Err(VmError::Runtime(error)) => {
                            alloc::vec![Value::Bool(false), error_table(error)]
                        }
                        Err(error) => return Err(error),
                    };
                    self.push_call_results(values, returns)
                }
                Value::Function(handle) => {
                    self.push_function_frame(handle, protected_arguments, returns, true)
                }
                _ => self.push_call_results(
                    alloc::vec![
                        Value::Bool(false),
                        error_table(ErrorValue::new("type", "pcall target is not callable"))
                    ],
                    returns,
                ),
            };
        }
        match callee {
            Value::Native(name) => {
                let values = self.builtin_or_host(&name, &args)?;
                self.push_call_results(values, returns)?;
            }
            Value::Function(handle) => self.push_function_frame(handle, &args, returns, false)?,
            _ => return Err(runtime("type", "attempt to call non-function")),
        }
        Ok(())
    }

    fn push_call_results(&mut self, mut values: Vec<Value>, returns: u8) -> Result<(), VmError> {
        values.truncate(returns as usize);
        while values.len() < returns as usize {
            values.push(Value::Nil);
        }
        for value in values {
            self.push(value)?;
        }
        Ok(())
    }

    fn push_function_frame(
        &mut self,
        handle: u16,
        arguments: &[Value],
        returns: u8,
        protected: bool,
    ) -> Result<(), VmError> {
        if self.frames.len() >= self.limits.call_depth {
            return Err(VmError::CallDepth);
        }
        if protected && returns == 0 {
            return Err(runtime("argument", "pcall result must be observed"));
        }
        let closure = self
            .closures
            .get(handle as usize)
            .and_then(Option::as_ref)
            .cloned()
            .ok_or(VmError::InvalidBytecode)?;
        let proto = self
            .chunk
            .functions
            .get(closure.function as usize)
            .ok_or(VmError::InvalidBytecode)?;
        let mut environment = closure.captured;
        for (index, parameter) in proto.parameters.iter().enumerate() {
            set_environment(
                &mut environment,
                *parameter,
                arguments.get(index).cloned().unwrap_or(Value::Nil),
            );
        }
        self.frames.push(Frame {
            function: Some(closure.function),
            closure: Some(handle),
            ip: 0,
            environment,
            stack_base: self.stack.len(),
            expected_returns: returns,
            protected,
        });
        Ok(())
    }

    fn builtin_or_host(&mut self, name: &str, arguments: &[Value]) -> Result<Vec<Value>, VmError> {
        match name {
            "type" => Ok(alloc::vec![Value::String(
                arguments
                    .first()
                    .unwrap_or(&Value::Nil)
                    .type_name()
                    .to_string()
            )]),
            "tostring" => Ok(alloc::vec![Value::String(
                arguments.first().unwrap_or(&Value::Nil).display()
            )]),
            "assert" => {
                if arguments.first().is_some_and(Value::truthy) {
                    Ok(arguments.to_vec())
                } else {
                    Err(runtime(
                        "assert",
                        arguments
                            .get(1)
                            .map(Value::display)
                            .as_deref()
                            .unwrap_or("assertion failed"),
                    ))
                }
            }
            "error" => Err(runtime(
                "script",
                arguments
                    .first()
                    .map(Value::display)
                    .as_deref()
                    .unwrap_or("error"),
            )),
            _ => self.host.call(name, arguments).map_err(VmError::Runtime),
        }
    }

    fn load(&mut self, name: u16) -> Result<Value, VmError> {
        for frame in self.frames.iter().rev() {
            if let Some((_, value)) = frame
                .environment
                .iter()
                .rev()
                .find(|(candidate, _)| *candidate == name)
            {
                return Ok(value.clone());
            }
        }
        let text = self
            .chunk
            .names
            .get(name as usize)
            .ok_or(VmError::InvalidBytecode)?;
        if matches!(
            text.as_str(),
            "type" | "tostring" | "assert" | "error" | "pcall" | "require"
        ) {
            return Ok(Value::Native(text.clone()));
        }
        self.host
            .resolve(text)
            .or(Some(Value::Native(text.clone())))
            .ok_or_else(|| runtime("name", "undefined variable"))
    }

    fn define(&mut self, name: u16, value: Value) {
        set_environment(
            &mut self.frames.last_mut().unwrap().environment,
            name,
            value,
        );
    }
    fn store(&mut self, name: u16, value: Value) -> bool {
        for frame in self.frames.iter_mut().rev() {
            if let Some((_, slot)) = frame
                .environment
                .iter_mut()
                .rev()
                .find(|(candidate, _)| *candidate == name)
            {
                *slot = value;
                return true;
            }
        }
        false
    }
    fn jump(&mut self, target: usize) -> Result<(), VmError> {
        let frame = self.frames.last_mut().ok_or(VmError::InvalidBytecode)?;
        let length = match frame.function {
            Some(i) => self.chunk.functions.get(i as usize).map(|f| f.code.len()),
            None => Some(self.chunk.code.len()),
        }
        .ok_or(VmError::InvalidBytecode)?;
        if target > length {
            return Err(VmError::InvalidBytecode);
        }
        frame.ip = target;
        Ok(())
    }
    fn push(&mut self, value: Value) -> Result<(), VmError> {
        if self.stack.len() >= self.limits.stack {
            return Err(VmError::StackOverflow);
        }
        self.stack.push(value);
        if let Err(error) = self.collect_garbage() {
            self.stack.pop();
            return Err(error);
        }
        Ok(())
    }
    fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::InvalidBytecode)
    }
    fn numeric_binary(
        &mut self,
        operation: fn(Value, Value) -> Result<Value, VmError>,
    ) -> Result<(), VmError> {
        let right = self.pop()?;
        let left = self.pop()?;
        let value = operation(left, right)?;
        self.push(value)
    }
    fn compare(&mut self, comparison: impl FnOnce(&Value, &Value) -> bool) -> Result<(), VmError> {
        let right = self.pop()?;
        let left = self.pop()?;
        self.push(Value::Bool(comparison(&left, &right)))
    }
    fn order(
        &mut self,
        comparison: impl FnOnce(core::cmp::Ordering) -> bool,
    ) -> Result<(), VmError> {
        let right = self.pop()?;
        let left = self.pop()?;
        let ordering = compare_order(&left, &right)?;
        self.push(Value::Bool(comparison(ordering)))
    }
    fn check_limits(&mut self) -> Result<(), VmError> {
        if self.instructions >= self.limits.instructions {
            return Err(VmError::InstructionLimit);
        }
        if self.host.now_ns().saturating_sub(self.start_ns) >= self.limits.timeout_ns {
            return Err(VmError::Timeout);
        }
        if self.host.interrupted() {
            return Err(VmError::Interrupted);
        }
        if self.instructions != 0 && self.instructions % YIELD_INTERVAL == 0 {
            self.collect_garbage()?;
            self.host.yield_now();
        }
        Ok(())
    }

    fn collect_garbage(&mut self) -> Result<(), VmError> {
        let host_roots = self.host.gc_roots();
        let mut marked = alloc::vec![false; self.closures.len()];
        for value in &self.stack {
            mark_closures(value, &self.closures, &mut marked);
        }
        for frame in &self.frames {
            for (_, value) in &frame.environment {
                mark_closures(value, &self.closures, &mut marked);
            }
        }
        for value in &host_roots {
            mark_closures(value, &self.closures, &mut marked);
        }
        for (index, closure) in self.closures.iter_mut().enumerate() {
            if !marked[index] {
                *closure = None;
            }
        }
        let roots = self.stack.iter().map(value_heap).sum::<usize>()
            + self
                .frames
                .iter()
                .flat_map(|frame| frame.environment.iter())
                .map(|(_, value)| value_heap(value))
                .sum::<usize>()
            + host_roots.iter().map(value_heap).sum::<usize>();
        let closures = self
            .closures
            .iter()
            .filter_map(Option::as_ref)
            .map(|closure| {
                closure
                    .captured
                    .iter()
                    .map(|(_, value)| value_heap(value))
                    .sum::<usize>()
            })
            .sum::<usize>();
        self.heap_bytes = roots.saturating_add(closures);
        if self.heap_bytes > self.limits.heap_bytes {
            Err(VmError::HeapLimit)
        } else {
            Ok(())
        }
    }
}

fn set_environment(environment: &mut Vec<(u16, Value)>, name: u16, value: Value) {
    if let Some((_, slot)) = environment
        .iter_mut()
        .rev()
        .find(|(candidate, _)| *candidate == name)
    {
        *slot = value
    } else {
        environment.push((name, value))
    }
}
fn index(target: &Value, key: &Value) -> Result<Value, VmError> {
    match target {
        Value::Table(entries) => Ok(entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.clone())
            .unwrap_or(Value::Nil)),
        Value::String(value) => {
            let Value::Integer(index) = key else {
                return Err(runtime("type", "string index must be integer"));
            };
            if *index < 1 {
                return Ok(Value::Nil);
            }
            Ok(value
                .chars()
                .nth(*index as usize - 1)
                .map(|v| Value::String(v.to_string()))
                .unwrap_or(Value::Nil))
        }
        Value::Bytes(value) => {
            let Value::Integer(index) = key else {
                return Err(runtime("type", "bytes index must be integer"));
            };
            if *index < 1 {
                return Ok(Value::Nil);
            }
            Ok(value
                .get(*index as usize - 1)
                .map(|v| Value::Integer(*v as i64))
                .unwrap_or(Value::Nil))
        }
        _ => Err(runtime("type", "value is not indexable")),
    }
}
fn value_heap(value: &Value) -> usize {
    match value {
        Value::String(v) => v.len(),
        Value::Bytes(v) => v.len(),
        Value::Table(v) => {
            v.len() * core::mem::size_of::<(Value, Value)>()
                + v.iter()
                    .map(|(key, value)| value_heap(key) + value_heap(value))
                    .sum::<usize>()
        }
        _ => 0,
    }
}

fn mark_closures(value: &Value, closures: &[Option<Closure>], marked: &mut [bool]) {
    match value {
        Value::Function(handle) => {
            let index = *handle as usize;
            if index >= marked.len() || marked[index] {
                return;
            }
            marked[index] = true;
            if let Some(closure) = closures.get(index).and_then(Option::as_ref) {
                for (_, value) in &closure.captured {
                    mark_closures(value, closures, marked);
                }
            }
        }
        Value::Table(entries) => {
            for (key, value) in entries {
                mark_closures(key, closures, marked);
                mark_closures(value, closures, marked);
            }
        }
        _ => {}
    }
}

fn validate_chunk(chunk: &Chunk) -> Result<(), VmError> {
    fn validate_code(chunk: &Chunk, code: &[Op]) -> Result<(), VmError> {
        for operation in code {
            match *operation {
                Op::Constant(index) if index as usize >= chunk.constants.len() => {
                    return Err(VmError::InvalidBytecode);
                }
                Op::Load(index) | Op::Define(index) | Op::Store(index)
                    if index as usize >= chunk.names.len() =>
                {
                    return Err(VmError::InvalidBytecode);
                }
                Op::MakeClosure(index) if index as usize >= chunk.functions.len() => {
                    return Err(VmError::InvalidBytecode);
                }
                Op::Jump(target) | Op::JumpIfFalse(target) if target > code.len() => {
                    return Err(VmError::InvalidBytecode);
                }
                _ => {}
            }
        }
        Ok(())
    }
    validate_code(chunk, &chunk.code)?;
    for function in &chunk.functions {
        if function
            .parameters
            .iter()
            .any(|index| *index as usize >= chunk.names.len())
        {
            return Err(VmError::InvalidBytecode);
        }
        validate_code(chunk, &function.code)?;
    }
    Ok(())
}
fn runtime(kind: &str, message: &str) -> VmError {
    VmError::Runtime(ErrorValue::new(kind, message))
}
fn error_table(error: ErrorValue) -> Value {
    Value::Table(alloc::vec![
        (Value::String("kind".to_string()), Value::String(error.kind)),
        (
            Value::String("message".to_string()),
            Value::String(error.message)
        ),
        (
            Value::String("operation".to_string()),
            Value::String(error.operation)
        ),
        (
            Value::String("code".to_string()),
            Value::Integer(error.code)
        ),
    ])
}

fn numeric_pair(left: Value, right: Value) -> Result<(f64, f64, bool), VmError> {
    match (left, right) {
        (Value::Integer(a), Value::Integer(b)) => Ok((a as f64, b as f64, true)),
        (Value::Integer(a), Value::Float(b)) => Ok((a as f64, b, false)),
        (Value::Float(a), Value::Integer(b)) => Ok((a, b as f64, false)),
        (Value::Float(a), Value::Float(b)) => Ok((a, b, false)),
        _ => Err(runtime("type", "arithmetic requires numbers")),
    }
}
fn arithmetic_add(left: Value, right: Value) -> Result<Value, VmError> {
    if let (Value::String(mut a), Value::String(b)) = (left.clone(), right.clone()) {
        a.push_str(&b);
        return Ok(Value::String(a));
    }
    match (left, right) {
        (Value::Integer(a), Value::Integer(b)) => a
            .checked_add(b)
            .map(Value::Integer)
            .ok_or_else(|| runtime("overflow", "integer overflow")),
        (a, b) => {
            let (a, b, _) = numeric_pair(a, b)?;
            Ok(Value::Float(a + b))
        }
    }
}
fn arithmetic_subtract(left: Value, right: Value) -> Result<Value, VmError> {
    match (left, right) {
        (Value::Integer(a), Value::Integer(b)) => a
            .checked_sub(b)
            .map(Value::Integer)
            .ok_or_else(|| runtime("overflow", "integer overflow")),
        (a, b) => {
            let (a, b, _) = numeric_pair(a, b)?;
            Ok(Value::Float(a - b))
        }
    }
}
fn arithmetic_multiply(left: Value, right: Value) -> Result<Value, VmError> {
    match (left, right) {
        (Value::Integer(a), Value::Integer(b)) => a
            .checked_mul(b)
            .map(Value::Integer)
            .ok_or_else(|| runtime("overflow", "integer overflow")),
        (a, b) => {
            let (a, b, _) = numeric_pair(a, b)?;
            Ok(Value::Float(a * b))
        }
    }
}
fn arithmetic_divide(left: Value, right: Value) -> Result<Value, VmError> {
    let (a, b, _) = numeric_pair(left, right)?;
    if b == 0.0 {
        return Err(runtime("divide", "division by zero"));
    }
    Ok(Value::Float(a / b))
}
fn arithmetic_integer_divide(left: Value, right: Value) -> Result<Value, VmError> {
    let (Value::Integer(a), Value::Integer(b)) = (left, right) else {
        return Err(runtime("type", "'//' requires integers"));
    };
    if b == 0 {
        return Err(runtime("divide", "division by zero"));
    }
    a.checked_div(b)
        .map(Value::Integer)
        .ok_or_else(|| runtime("overflow", "integer overflow"))
}
fn arithmetic_remainder(left: Value, right: Value) -> Result<Value, VmError> {
    let (Value::Integer(a), Value::Integer(b)) = (left, right) else {
        return Err(runtime("type", "'%' requires integers"));
    };
    if b == 0 {
        return Err(runtime("divide", "division by zero"));
    }
    a.checked_rem(b)
        .map(Value::Integer)
        .ok_or_else(|| runtime("overflow", "integer overflow"))
}
fn compare_order(left: &Value, right: &Value) -> Result<core::cmp::Ordering, VmError> {
    match (left, right) {
        (Value::Integer(a), Value::Integer(b)) => Ok(a.cmp(b)),
        (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
        (a, b) => {
            let (a, b, _) = numeric_pair(a.clone(), b.clone())?;
            a.partial_cmp(&b)
                .ok_or_else(|| runtime("number", "unordered comparison"))
        }
    }
}
