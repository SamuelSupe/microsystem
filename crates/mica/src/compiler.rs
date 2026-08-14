use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::lexer::{Token, TokenKind, lex};
use crate::{BYTECODE_LIMIT, Value};

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Constant(u16),
    Nil,
    True,
    False,
    Load(u16),
    Define(u16),
    Store(u16),
    NewTable,
    Index,
    SetIndex,
    MakeClosure(u16),
    Add,
    Subtract,
    Multiply,
    Divide,
    IntegerDivide,
    Remainder,
    Negate,
    Not,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    Dup,
    Swap,
    Pop,
    Call { arguments: u8, returns: u8 },
    Jump(usize),
    JumpIfFalse(usize),
    Return(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionProto {
    pub parameters: Vec<u16>,
    pub code: Vec<Op>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub constants: Vec<Value>,
    pub names: Vec<String>,
    pub functions: Vec<FunctionProto>,
    pub code: Vec<Op>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl CompileError {
    pub fn new(line: usize, column: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            column,
            message: message.into(),
        }
    }
}

pub fn compile(source: &str) -> Result<Chunk, CompileError> {
    let tokens =
        lex(source).map_err(|error| CompileError::new(error.line, error.column, error.message))?;
    let mut compiler = Compiler {
        tokens,
        cursor: 0,
        constants: Vec::new(),
        names: Vec::new(),
        functions: Vec::new(),
        code: Vec::new(),
        breaks: Vec::new(),
        scopes: alloc::vec![Vec::new()],
    };
    compiler.block_until(|kind| matches!(kind, TokenKind::Eof))?;
    compiler.emit(Op::Return(0));
    let chunk = Chunk {
        constants: compiler.constants,
        names: compiler.names,
        functions: compiler.functions,
        code: compiler.code,
    };
    let bytes = chunk.code.len() * core::mem::size_of::<Op>()
        + chunk
            .functions
            .iter()
            .map(|function| function.code.len() * core::mem::size_of::<Op>())
            .sum::<usize>()
        + chunk.constants.iter().map(value_size).sum::<usize>()
        + chunk.names.iter().map(String::len).sum::<usize>();
    if bytes > BYTECODE_LIMIT {
        Err(CompileError::new(0, 0, "bytecode exceeds 128 KiB"))
    } else {
        Ok(chunk)
    }
}

fn value_size(value: &Value) -> usize {
    match value {
        Value::String(value) => value.len(),
        Value::Bytes(value) => value.len(),
        Value::Table(entries) => entries.len() * core::mem::size_of::<(Value, Value)>(),
        _ => core::mem::size_of::<Value>(),
    }
}

struct Compiler {
    tokens: Vec<Token>,
    cursor: usize,
    constants: Vec<Value>,
    names: Vec<String>,
    functions: Vec<FunctionProto>,
    code: Vec<Op>,
    breaks: Vec<Vec<usize>>,
    scopes: Vec<Vec<u16>>,
}

impl Compiler {
    fn block_until(&mut self, end: impl Fn(&TokenKind) -> bool) -> Result<(), CompileError> {
        while !end(self.kind()) {
            if matches!(self.kind(), TokenKind::Eof) {
                return Err(self.error("unterminated block"));
            }
            self.statement()?;
            self.take_simple(TokenKind::Semicolon);
        }
        Ok(())
    }

    fn statement(&mut self) -> Result<(), CompileError> {
        match self.kind() {
            TokenKind::Local => self.local_statement(),
            TokenKind::Function => self.function_statement(),
            TokenKind::If => self.if_statement(),
            TokenKind::While => self.while_statement(),
            TokenKind::For => self.for_statement(),
            TokenKind::Break => self.break_statement(),
            TokenKind::Return => self.return_statement(),
            TokenKind::Identifier(_)
                if matches!(self.peek_kind(1), Some(TokenKind::Equal | TokenKind::Comma)) =>
            {
                self.assignment_statement()
            }
            _ => {
                self.expression(1)?;
                self.emit(Op::Pop);
                Ok(())
            }
        }
    }

    fn local_statement(&mut self) -> Result<(), CompileError> {
        self.advance();
        let mut names = Vec::new();
        loop {
            names.push(self.consume_identifier("expected local name")?);
            if !self.take_simple(TokenKind::Comma) {
                break;
            }
        }
        if self.take_simple(TokenKind::Equal) {
            self.expression(names.len().min(u8::MAX as usize) as u8)?;
        } else {
            for _ in &names {
                self.emit(Op::Nil);
            }
        }
        for name in names.into_iter().rev() {
            let index = self.name(&name);
            self.scopes.last_mut().unwrap().push(index);
            self.emit(Op::Define(index));
        }
        Ok(())
    }

    fn assignment_statement(&mut self) -> Result<(), CompileError> {
        let mut names = Vec::new();
        loop {
            names.push(self.consume_identifier("expected assignment name")?);
            if !self.take_simple(TokenKind::Comma) {
                break;
            }
        }
        self.consume_simple(TokenKind::Equal, "expected '='")?;
        self.expression(names.len().min(u8::MAX as usize) as u8)?;
        for name in names.into_iter().rev() {
            let index = self.name(&name);
            if !self.is_declared(index) {
                return Err(self.error_at_previous("assignment to undeclared variable"));
            }
            self.emit(Op::Store(index));
        }
        Ok(())
    }

    fn function_statement(&mut self) -> Result<(), CompileError> {
        self.advance();
        let name = self.consume_identifier("expected function name")?;
        let name_index = self.name(&name);
        self.scopes.last_mut().unwrap().push(name_index);
        let function = self.function_body()?;
        self.emit(Op::MakeClosure(function));
        self.emit(Op::Define(name_index));
        Ok(())
    }

    fn function_body(&mut self) -> Result<u16, CompileError> {
        self.consume_simple(TokenKind::LeftParen, "expected '('")?;
        let mut parameters = Vec::new();
        if !matches!(self.kind(), TokenKind::RightParen) {
            loop {
                let parameter = self.consume_identifier("expected parameter")?;
                parameters.push(self.name(&parameter));
                if !self.take_simple(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.consume_simple(TokenKind::RightParen, "expected ')'")?;
        let outer_code = core::mem::take(&mut self.code);
        let outer_breaks = core::mem::take(&mut self.breaks);
        self.scopes.push(parameters.clone());
        self.block_until(|kind| matches!(kind, TokenKind::End))?;
        self.consume_simple(TokenKind::End, "expected end")?;
        self.emit(Op::Return(0));
        let code = core::mem::replace(&mut self.code, outer_code);
        self.breaks = outer_breaks;
        self.scopes.pop();
        let index =
            u16::try_from(self.functions.len()).map_err(|_| self.error("too many functions"))?;
        self.functions.push(FunctionProto { parameters, code });
        Ok(index)
    }

    fn if_statement(&mut self) -> Result<(), CompileError> {
        self.advance();
        let mut end_jumps = Vec::new();
        loop {
            self.expression(1)?;
            self.consume_simple(TokenKind::Then, "expected then")?;
            let false_jump = self.emit(Op::JumpIfFalse(usize::MAX));
            self.scopes.push(Vec::new());
            self.block_until(|kind| {
                matches!(kind, TokenKind::Else | TokenKind::ElseIf | TokenKind::End)
            })?;
            self.scopes.pop();
            end_jumps.push(self.emit(Op::Jump(usize::MAX)));
            self.patch(false_jump, self.code.len());
            if self.take_simple(TokenKind::ElseIf) {
                continue;
            }
            if self.take_simple(TokenKind::Else) {
                self.scopes.push(Vec::new());
                self.block_until(|kind| matches!(kind, TokenKind::End))?;
                self.scopes.pop();
            }
            break;
        }
        self.consume_simple(TokenKind::End, "expected end")?;
        let end = self.code.len();
        for jump in end_jumps {
            self.patch(jump, end);
        }
        Ok(())
    }

    fn while_statement(&mut self) -> Result<(), CompileError> {
        self.advance();
        let start = self.code.len();
        self.expression(1)?;
        self.consume_simple(TokenKind::Do, "expected do")?;
        let exit = self.emit(Op::JumpIfFalse(usize::MAX));
        self.breaks.push(Vec::new());
        self.scopes.push(Vec::new());
        self.block_until(|kind| matches!(kind, TokenKind::End))?;
        self.scopes.pop();
        self.consume_simple(TokenKind::End, "expected end")?;
        self.emit(Op::Jump(start));
        let target = self.code.len();
        self.patch(exit, target);
        for jump in self.breaks.pop().unwrap() {
            self.patch(jump, target);
        }
        Ok(())
    }

    fn for_statement(&mut self) -> Result<(), CompileError> {
        self.advance();
        let name = self.consume_identifier("expected loop variable")?;
        if !matches!(self.kind(), TokenKind::Equal) {
            return self.iterator_for_statement(name);
        }
        self.advance();
        self.expression(1)?;
        let variable = self.name(&name);
        self.scopes.push(alloc::vec![variable]);
        self.emit(Op::Define(variable));
        self.consume_simple(TokenKind::Comma, "expected ','")?;
        let limit_name = self.name(&alloc::format!("$limit{}", self.code.len()));
        self.expression(1)?;
        self.emit(Op::Define(limit_name));
        let step_name = self.name(&alloc::format!("$step{}", self.code.len()));
        if self.take_simple(TokenKind::Comma) {
            self.expression(1)?;
        } else {
            self.emit_constant(Value::Integer(1))?;
        }
        self.emit(Op::Define(step_name));
        self.consume_simple(TokenKind::Do, "expected do")?;
        let start = self.code.len();
        self.emit(Op::Load(step_name));
        self.emit_constant(Value::Integer(0))?;
        self.emit(Op::GreaterEqual);
        let negative = self.emit(Op::JumpIfFalse(usize::MAX));
        self.emit(Op::Load(variable));
        self.emit(Op::Load(limit_name));
        self.emit(Op::LessEqual);
        let condition_ready = self.emit(Op::Jump(usize::MAX));
        self.patch(negative, self.code.len());
        self.emit(Op::Load(variable));
        self.emit(Op::Load(limit_name));
        self.emit(Op::GreaterEqual);
        self.patch(condition_ready, self.code.len());
        let exit = self.emit(Op::JumpIfFalse(usize::MAX));
        self.breaks.push(Vec::new());
        self.block_until(|kind| matches!(kind, TokenKind::End))?;
        self.consume_simple(TokenKind::End, "expected end")?;
        self.emit(Op::Load(variable));
        self.emit(Op::Load(step_name));
        self.emit(Op::Add);
        self.emit(Op::Store(variable));
        self.emit(Op::Jump(start));
        let target = self.code.len();
        self.patch(exit, target);
        for jump in self.breaks.pop().unwrap() {
            self.patch(jump, target);
        }
        self.scopes.pop();
        Ok(())
    }

    fn iterator_for_statement(&mut self, first: String) -> Result<(), CompileError> {
        let second = if self.take_simple(TokenKind::Comma) {
            Some(self.consume_identifier("expected second loop variable")?)
        } else {
            None
        };
        self.consume_simple(TokenKind::In, "expected in")?;
        self.expression(1)?;
        let table_name = self.name(&alloc::format!("$table{}", self.code.len()));
        let index_name = self.name(&alloc::format!("$index{}", self.code.len()));
        let pair_name = self.name(&alloc::format!("$pair{}", self.code.len()));
        let first_name = self.name(&first);
        let second_name = second.as_ref().map(|name| self.name(name));
        self.scopes
            .push(alloc::vec![table_name, index_name, pair_name, first_name]);
        if let Some(name) = second_name {
            self.scopes.last_mut().unwrap().push(name);
        }
        self.emit(Op::Define(table_name));
        self.emit_constant(Value::Integer(0))?;
        self.emit(Op::Define(index_name));
        self.consume_simple(TokenKind::Do, "expected do")?;
        let start = self.code.len();
        self.emit(Op::Load(index_name));
        self.emit_constant(Value::Integer(1))?;
        self.emit(Op::Add);
        self.emit(Op::Store(index_name));
        let next_name = self.name("table.next");
        self.emit(Op::Load(next_name));
        self.emit(Op::Load(table_name));
        self.emit(Op::Load(index_name));
        self.emit(Op::Call {
            arguments: 2,
            returns: 1,
        });
        self.emit(Op::Define(pair_name));
        self.emit(Op::Load(pair_name));
        self.emit(Op::Nil);
        self.emit(Op::Equal);
        let body = self.emit(Op::JumpIfFalse(usize::MAX));
        let exit = self.emit(Op::Jump(usize::MAX));
        self.patch(body, self.code.len());
        self.emit(Op::Load(pair_name));
        self.emit_constant(Value::Integer(1))?;
        self.emit(Op::Index);
        self.emit(Op::Define(first_name));
        if let Some(name) = second_name {
            self.emit(Op::Load(pair_name));
            self.emit_constant(Value::Integer(2))?;
            self.emit(Op::Index);
            self.emit(Op::Define(name));
        }
        self.breaks.push(Vec::new());
        self.block_until(|kind| matches!(kind, TokenKind::End))?;
        self.consume_simple(TokenKind::End, "expected end")?;
        self.emit(Op::Jump(start));
        let target = self.code.len();
        self.patch(exit, target);
        for jump in self.breaks.pop().unwrap() {
            self.patch(jump, target);
        }
        self.scopes.pop();
        Ok(())
    }

    fn break_statement(&mut self) -> Result<(), CompileError> {
        self.advance();
        if self.breaks.is_empty() {
            return Err(self.error_at_previous("break outside loop"));
        }
        let jump = self.emit(Op::Jump(usize::MAX));
        self.breaks.last_mut().unwrap().push(jump);
        Ok(())
    }

    fn return_statement(&mut self) -> Result<(), CompileError> {
        self.advance();
        if matches!(
            self.kind(),
            TokenKind::End
                | TokenKind::Else
                | TokenKind::ElseIf
                | TokenKind::Eof
                | TokenKind::Semicolon
        ) {
            self.emit(Op::Return(0));
            return Ok(());
        }
        let mut count = 0u8;
        loop {
            self.expression(1)?;
            count = count
                .checked_add(1)
                .ok_or_else(|| self.error("too many return values"))?;
            if !self.take_simple(TokenKind::Comma) {
                break;
            }
        }
        self.emit(Op::Return(count));
        Ok(())
    }

    fn expression(&mut self, returns: u8) -> Result<(), CompileError> {
        self.binary(0, returns)
    }

    fn binary(&mut self, minimum: u8, returns: u8) -> Result<(), CompileError> {
        self.unary(returns)?;
        while let Some((precedence, op)) = binary_op(self.kind()) {
            if precedence < minimum {
                break;
            }
            self.advance();
            if matches!(op, Op::And | Op::Or) {
                self.emit(Op::Dup);
                if matches!(op, Op::Or) {
                    self.emit(Op::Not);
                }
                let short_circuit = self.emit(Op::JumpIfFalse(usize::MAX));
                self.emit(Op::Pop);
                self.binary(precedence + 1, 1)?;
                self.patch(short_circuit, self.code.len());
                continue;
            }
            self.binary(precedence + 1, 1)?;
            self.emit(op);
        }
        Ok(())
    }

    fn unary(&mut self, returns: u8) -> Result<(), CompileError> {
        if self.take_simple(TokenKind::Minus) {
            self.unary(1)?;
            self.emit(Op::Negate);
            return Ok(());
        }
        if self.take_simple(TokenKind::Not) {
            self.unary(1)?;
            self.emit(Op::Not);
            return Ok(());
        }
        self.primary()?;
        self.postfix(returns)
    }

    fn primary(&mut self) -> Result<(), CompileError> {
        let token = self.advance().clone();
        match token.kind {
            TokenKind::Integer(value) => self.emit_constant(Value::Integer(value)),
            TokenKind::Float(value) => self.emit_constant(Value::Float(value)),
            TokenKind::String(value) => self.emit_constant(Value::String(value)),
            TokenKind::True => {
                self.emit(Op::True);
                Ok(())
            }
            TokenKind::False => {
                self.emit(Op::False);
                Ok(())
            }
            TokenKind::Nil => {
                self.emit(Op::Nil);
                Ok(())
            }
            TokenKind::Identifier(name) => {
                let index = self.name(&name);
                self.emit(Op::Load(index));
                Ok(())
            }
            TokenKind::LeftParen => {
                self.expression(1)?;
                self.consume_simple(TokenKind::RightParen, "expected ')'")
            }
            TokenKind::LeftBrace => self.table(),
            TokenKind::Function => {
                let function = self.function_body()?;
                self.emit(Op::MakeClosure(function));
                Ok(())
            }
            _ => Err(CompileError::new(
                token.line,
                token.column,
                "expected expression",
            )),
        }
    }

    fn table(&mut self) -> Result<(), CompileError> {
        self.emit(Op::NewTable);
        let mut array_index = 1i64;
        while !self.take_simple(TokenKind::RightBrace) {
            if let TokenKind::Identifier(name) = self.kind().clone()
                && matches!(self.peek_kind(1), Some(TokenKind::Equal))
            {
                self.advance();
                self.advance();
                self.emit_constant(Value::String(name))?;
                self.expression(1)?;
            } else if self.take_simple(TokenKind::LeftBracket) {
                self.expression(1)?;
                self.consume_simple(TokenKind::RightBracket, "expected ']'")?;
                self.consume_simple(TokenKind::Equal, "expected '='")?;
                self.expression(1)?;
            } else {
                self.emit_constant(Value::Integer(array_index))?;
                array_index += 1;
                self.expression(1)?;
            }
            self.emit(Op::SetIndex);
            if self.take_simple(TokenKind::RightBrace) {
                break;
            }
            self.consume_simple(TokenKind::Comma, "expected ',' or '}'")?;
        }
        Ok(())
    }

    fn postfix(&mut self, returns: u8) -> Result<(), CompileError> {
        loop {
            if self.take_simple(TokenKind::LeftBracket) {
                self.expression(1)?;
                self.consume_simple(TokenKind::RightBracket, "expected ']'")?;
                self.emit(Op::Index);
            } else if self.take_simple(TokenKind::Dot) {
                let name = self.consume_identifier("expected field name")?;
                self.emit_constant(Value::String(name))?;
                self.emit(Op::Index);
            } else if self.take_simple(TokenKind::Colon) {
                let name = self.consume_identifier("expected method name")?;
                self.emit(Op::Dup);
                self.emit_constant(Value::String(name))?;
                self.emit(Op::Index);
                self.emit(Op::Swap);
                let arguments = self.arguments(1)?;
                self.emit(Op::Call { arguments, returns });
            } else if self.take_simple(TokenKind::LeftParen) {
                let arguments = self.arguments_after_open(0)?;
                self.emit(Op::Call { arguments, returns });
            } else if self.take_simple(TokenKind::LeftBrace) {
                self.table()?;
                self.emit(Op::Call {
                    arguments: 1,
                    returns,
                });
            } else {
                break;
            }
        }
        Ok(())
    }

    fn arguments(&mut self, initial: u8) -> Result<u8, CompileError> {
        self.consume_simple(TokenKind::LeftParen, "expected '('")?;
        self.arguments_after_open(initial)
    }

    fn arguments_after_open(&mut self, mut count: u8) -> Result<u8, CompileError> {
        if !matches!(self.kind(), TokenKind::RightParen) {
            loop {
                self.expression(1)?;
                count = count
                    .checked_add(1)
                    .ok_or_else(|| self.error("too many arguments"))?;
                if !self.take_simple(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.consume_simple(TokenKind::RightParen, "expected ')'")?;
        Ok(count)
    }

    fn emit_constant(&mut self, value: Value) -> Result<(), CompileError> {
        let index =
            u16::try_from(self.constants.len()).map_err(|_| self.error("too many constants"))?;
        self.constants.push(value);
        self.emit(Op::Constant(index));
        Ok(())
    }

    fn name(&mut self, name: &str) -> u16 {
        if let Some(index) = self.names.iter().position(|candidate| candidate == name) {
            index as u16
        } else {
            let index = self.names.len() as u16;
            self.names.push(name.to_string());
            index
        }
    }

    fn is_declared(&self, name: u16) -> bool {
        self.scopes.iter().rev().any(|scope| scope.contains(&name))
    }

    fn emit(&mut self, op: Op) -> usize {
        let index = self.code.len();
        self.code.push(op);
        index
    }

    fn patch(&mut self, index: usize, target: usize) {
        match &mut self.code[index] {
            Op::Jump(value) | Op::JumpIfFalse(value) => *value = target,
            _ => unreachable!(),
        }
    }

    fn kind(&self) -> &TokenKind {
        &self.tokens[self.cursor].kind
    }

    fn peek_kind(&self, offset: usize) -> Option<&TokenKind> {
        self.tokens
            .get(self.cursor + offset)
            .map(|token| &token.kind)
    }

    fn advance(&mut self) -> &Token {
        let index = self.cursor;
        if !matches!(self.tokens[index].kind, TokenKind::Eof) {
            self.cursor += 1;
        }
        &self.tokens[index]
    }

    fn take_simple(&mut self, expected: TokenKind) -> bool {
        if core::mem::discriminant(self.kind()) == core::mem::discriminant(&expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn consume_simple(&mut self, expected: TokenKind, message: &str) -> Result<(), CompileError> {
        if self.take_simple(expected) {
            Ok(())
        } else {
            Err(self.error(message))
        }
    }

    fn consume_identifier(&mut self, message: &str) -> Result<String, CompileError> {
        let token = self.advance().clone();
        match token.kind {
            TokenKind::Identifier(name) => Ok(name),
            _ => Err(CompileError::new(token.line, token.column, message)),
        }
    }

    fn error(&self, message: &str) -> CompileError {
        let token = &self.tokens[self.cursor];
        CompileError::new(token.line, token.column, message)
    }

    fn error_at_previous(&self, message: &str) -> CompileError {
        let token = &self.tokens[self.cursor.saturating_sub(1)];
        CompileError::new(token.line, token.column, message)
    }
}

fn binary_op(kind: &TokenKind) -> Option<(u8, Op)> {
    Some(match kind {
        TokenKind::Or => (1, Op::Or),
        TokenKind::And => (2, Op::And),
        TokenKind::EqualEqual => (3, Op::Equal),
        TokenKind::NotEqual => (3, Op::NotEqual),
        TokenKind::Less => (4, Op::Less),
        TokenKind::LessEqual => (4, Op::LessEqual),
        TokenKind::Greater => (4, Op::Greater),
        TokenKind::GreaterEqual => (4, Op::GreaterEqual),
        TokenKind::Plus => (5, Op::Add),
        TokenKind::Minus => (5, Op::Subtract),
        TokenKind::Star => (6, Op::Multiply),
        TokenKind::Slash => (6, Op::Divide),
        TokenKind::SlashSlash => (6, Op::IntegerDivide),
        TokenKind::Percent => (6, Op::Remainder),
        _ => return None,
    })
}
