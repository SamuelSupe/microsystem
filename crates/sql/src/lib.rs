#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

pub const MAX_SQL_BYTES: usize = 4096;
pub const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;
pub const MAX_TABLES: usize = 32;
pub const MAX_COLUMNS: usize = 32;
pub const MAX_IDENTIFIER_BYTES: usize = 63;
pub const MAX_TEXT_BYTES: usize = 1024;
pub const MAX_RESULT_BYTES: usize = 4096;

const SNAPSHOT_MAGIC: &[u8; 8] = b"MSQLDB1\0";
const SNAPSHOT_VERSION: u16 = 1;
const SNAPSHOT_HEADER_BYTES: usize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Empty,
    Syntax,
    InvalidIdentifier,
    InvalidLiteral,
    Unsupported,
    InvalidType,
    TableNotFound,
    TableExists,
    ColumnNotFound,
    ColumnExists,
    Constraint,
    TypeMismatch,
    TooLarge,
    Corrupt,
    Version,
    NoSpace,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Empty => "empty statement",
            Self::Syntax => "syntax error",
            Self::InvalidIdentifier => "invalid identifier",
            Self::InvalidLiteral => "invalid literal",
            Self::Unsupported => "unsupported SQL feature",
            Self::InvalidType => "invalid column type",
            Self::TableNotFound => "table not found",
            Self::TableExists => "table already exists",
            Self::ColumnNotFound => "column not found",
            Self::ColumnExists => "column already exists",
            Self::Constraint => "constraint violation",
            Self::TypeMismatch => "type mismatch",
            Self::TooLarge => "database or value too large",
            Self::Corrupt => "corrupt database snapshot",
            Self::Version => "unsupported database snapshot version",
            Self::NoSpace => "result or snapshot exceeds the size limit",
        };
        f.write_str(text)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SqlType {
    Integer = 1,
    Text = 2,
    Bool = 3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Text(String),
    Bool(bool),
}

impl Value {
    pub fn sql_type(&self) -> Option<SqlType> {
        match self {
            Self::Null => None,
            Self::Integer(_) => Some(SqlType::Integer),
            Self::Text(_) => Some(SqlType::Text),
            Self::Bool(_) => Some(SqlType::Bool),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Column {
    pub name: String,
    pub ty: SqlType,
    pub primary_key: bool,
    pub not_null: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    pub ty: SqlType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Table {
    name: String,
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Database {
    tables: Vec<Table>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Execution {
    Command {
        affected_rows: u32,
    },
    Rows {
        columns: Vec<ColumnInfo>,
        rows: Vec<Vec<Value>>,
    },
}

impl Database {
    pub const fn new() -> Self {
        Self { tables: Vec::new() }
    }

    pub fn execute(&mut self, sql: &str) -> Result<Execution, Error> {
        if sql.len() > MAX_SQL_BYTES {
            return Err(Error::TooLarge);
        }
        let mut parser = Parser::new(sql)?;
        let statement = parser.parse_statement()?;
        if !parser.at_end() {
            return Err(Error::Syntax);
        }
        self.execute_statement(statement)
    }

    pub fn encode_snapshot(&self, output: &mut Vec<u8>) -> Result<(), Error> {
        output.clear();
        output.resize(SNAPSHOT_HEADER_BYTES, 0);
        let mut payload = Vec::new();
        put_u16(&mut payload, self.tables.len())?;
        for table in &self.tables {
            put_string(&mut payload, &table.name)?;
            put_u16(&mut payload, table.columns.len())?;
            for column in &table.columns {
                put_string(&mut payload, &column.name)?;
                payload.push(column.ty as u8);
                let mut flags = 0;
                if column.primary_key {
                    flags |= 1;
                }
                if column.not_null {
                    flags |= 2;
                }
                payload.push(flags);
                put_u16(&mut payload, 0)?;
            }
            put_u32(&mut payload, table.rows.len())?;
            for row in &table.rows {
                if row.len() != table.columns.len() {
                    return Err(Error::Corrupt);
                }
                for value in row {
                    encode_value(&mut payload, value)?;
                }
            }
        }
        if payload.len() > MAX_SNAPSHOT_BYTES - SNAPSHOT_HEADER_BYTES {
            return Err(Error::NoSpace);
        }
        output[..8].copy_from_slice(SNAPSHOT_MAGIC);
        output[8..10].copy_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
        output[10..12].copy_from_slice(&0u16.to_le_bytes());
        output[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        output[16..20].copy_from_slice(&crc32c(&payload).to_le_bytes());
        output.extend_from_slice(&payload);
        Ok(())
    }

    pub fn decode_snapshot(input: &[u8]) -> Result<Self, Error> {
        if input.len() < SNAPSHOT_HEADER_BYTES || &input[..8] != SNAPSHOT_MAGIC {
            return Err(Error::Corrupt);
        }
        let version = read_u16(input, 8)?;
        if version != SNAPSHOT_VERSION {
            return Err(Error::Version);
        }
        let payload_len = read_u32(input, 12)? as usize;
        let expected_end = SNAPSHOT_HEADER_BYTES
            .checked_add(payload_len)
            .ok_or(Error::Corrupt)?;
        if expected_end != input.len() || input.len() > MAX_SNAPSHOT_BYTES {
            return Err(Error::Corrupt);
        }
        let expected_crc = read_u32(input, 16)?;
        let payload = &input[SNAPSHOT_HEADER_BYTES..];
        if crc32c(payload) != expected_crc {
            return Err(Error::Corrupt);
        }
        let mut cursor = Cursor::new(payload);
        let table_count = cursor.u16()? as usize;
        if table_count > MAX_TABLES {
            return Err(Error::Corrupt);
        }
        let mut database = Self::new();
        for _ in 0..table_count {
            let name = cursor.string()?;
            validate_identifier(&name)?;
            let column_count = cursor.u16()? as usize;
            if column_count == 0 || column_count > MAX_COLUMNS {
                return Err(Error::Corrupt);
            }
            let mut columns = Vec::with_capacity(column_count);
            for _ in 0..column_count {
                let column_name = cursor.string()?;
                validate_identifier(&column_name)?;
                let ty = match cursor.byte()? {
                    1 => SqlType::Integer,
                    2 => SqlType::Text,
                    3 => SqlType::Bool,
                    _ => return Err(Error::Corrupt),
                };
                let flags = cursor.byte()?;
                let _ = cursor.u16()?;
                if flags & !3 != 0
                    || columns
                        .iter()
                        .any(|column: &Column| column.name == column_name)
                {
                    return Err(Error::Corrupt);
                }
                columns.push(Column {
                    name: column_name,
                    ty,
                    primary_key: flags & 1 != 0,
                    not_null: flags & 2 != 0 || flags & 1 != 0,
                });
            }
            if database.tables.iter().any(|table| table.name == name)
                || columns.iter().filter(|column| column.primary_key).count() > 1
            {
                return Err(Error::Corrupt);
            }
            let row_count = cursor.u32()? as usize;
            let mut rows = Vec::with_capacity(row_count.min(1024));
            for _ in 0..row_count {
                let mut row = Vec::with_capacity(column_count);
                for column in &columns {
                    let value = decode_value(&mut cursor)?;
                    validate_value(column, &value)?;
                    row.push(value);
                }
                validate_row(&columns, &rows, &row)?;
                rows.push(row);
            }
            database.tables.push(Table {
                name,
                columns,
                rows,
            });
        }
        if !cursor.at_end() {
            return Err(Error::Corrupt);
        }
        Ok(database)
    }

    fn execute_statement(&mut self, statement: Statement) -> Result<Execution, Error> {
        match statement {
            Statement::Create { name, columns } => {
                if self.tables.iter().any(|table| table.name == name) {
                    return Err(Error::TableExists);
                }
                if self.tables.len() == MAX_TABLES {
                    return Err(Error::TooLarge);
                }
                self.tables.push(Table {
                    name,
                    columns,
                    rows: Vec::new(),
                });
                Ok(Execution::Command { affected_rows: 0 })
            }
            Statement::Drop { name } => {
                let index = self.table_index(&name)?;
                self.tables.remove(index);
                Ok(Execution::Command { affected_rows: 0 })
            }
            Statement::Insert {
                table,
                columns,
                values,
            } => {
                let table = self.table_mut(&table)?;
                let mut row = vec![Value::Null; table.columns.len()];
                if let Some(column_names) = columns {
                    if column_names.len() != values.len()
                        || column_names
                            .iter()
                            .enumerate()
                            .any(|(index, name)| column_names[..index].contains(name))
                    {
                        return Err(Error::Constraint);
                    }
                    for (name, value) in column_names.iter().zip(values) {
                        let index = column_index(&table.columns, name)?;
                        row[index] = value;
                    }
                } else if values.len() != row.len() {
                    return Err(Error::Constraint);
                } else {
                    row = values;
                }
                validate_row(&table.columns, &table.rows, &row)?;
                table.rows.push(row);
                Ok(Execution::Command { affected_rows: 1 })
            }
            Statement::Select {
                table,
                columns,
                predicates,
            } => {
                let table = self.table(&table)?;
                let selected = selected_columns(&table.columns, columns.as_ref())?;
                validate_predicates(&table.columns, &predicates)?;
                let mut rows = Vec::new();
                for row in &table.rows {
                    if predicates_match(&table.columns, row, &predicates)? {
                        rows.push(selected.iter().map(|&index| row[index].clone()).collect());
                    }
                }
                let output_columns = selected
                    .iter()
                    .map(|&index| ColumnInfo {
                        name: table.columns[index].name.clone(),
                        ty: table.columns[index].ty,
                    })
                    .collect();
                Ok(Execution::Rows {
                    columns: output_columns,
                    rows,
                })
            }
            Statement::Update {
                table,
                assignments,
                predicates,
            } => {
                let table = self.table_mut(&table)?;
                for (name, value) in &assignments {
                    let index = column_index(&table.columns, name)?;
                    validate_value(&table.columns[index], value)?;
                }
                validate_predicates(&table.columns, &predicates)?;
                let mut updated = table.rows.clone();
                let mut affected = 0u32;
                for row in &mut updated {
                    if !predicates_match(&table.columns, row, &predicates)? {
                        continue;
                    }
                    for (name, value) in &assignments {
                        let index = column_index(&table.columns, name)?;
                        row[index] = value.clone();
                    }
                    validate_row_values(&table.columns, row)?;
                    affected = affected.saturating_add(1);
                }
                validate_unique_rows(&table.columns, &updated)?;
                table.rows = updated;
                Ok(Execution::Command {
                    affected_rows: affected,
                })
            }
            Statement::Delete { table, predicates } => {
                let table = self.table_mut(&table)?;
                let columns = table.columns.clone();
                validate_predicates(&columns, &predicates)?;
                let mut kept = Vec::with_capacity(table.rows.len());
                let mut affected = 0u32;
                for row in &table.rows {
                    if predicates_match(&columns, row, &predicates)? {
                        affected = affected.saturating_add(1);
                    } else {
                        kept.push(row.clone());
                    }
                }
                table.rows = kept;
                Ok(Execution::Command {
                    affected_rows: affected,
                })
            }
        }
    }

    fn table_index(&self, name: &str) -> Result<usize, Error> {
        self.tables
            .iter()
            .position(|table| table.name == name)
            .ok_or(Error::TableNotFound)
    }

    fn table(&self, name: &str) -> Result<&Table, Error> {
        let index = self.table_index(name)?;
        Ok(&self.tables[index])
    }

    fn table_mut(&mut self, name: &str) -> Result<&mut Table, Error> {
        let index = self.table_index(name)?;
        Ok(&mut self.tables[index])
    }
}

impl Default for Database {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
enum Statement {
    Create {
        name: String,
        columns: Vec<Column>,
    },
    Drop {
        name: String,
    },
    Insert {
        table: String,
        columns: Option<Vec<String>>,
        values: Vec<Value>,
    },
    Select {
        table: String,
        columns: Option<Vec<String>>,
        predicates: Vec<Predicate>,
    },
    Update {
        table: String,
        assignments: Vec<(String, Value)>,
        predicates: Vec<Predicate>,
    },
    Delete {
        table: String,
        predicates: Vec<Predicate>,
    },
}

#[derive(Clone, Copy, Debug)]
enum CompareOp {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    IsNull,
    IsNotNull,
}

#[derive(Clone, Debug)]
struct Predicate {
    column: String,
    op: CompareOp,
    value: Option<Value>,
}

#[derive(Clone, Debug)]
enum Token {
    Word(String),
    Number(i64),
    String(String),
    Symbol(u8),
    Eof,
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn new(input: &str) -> Result<Self, Error> {
        if input.trim().is_empty() {
            return Err(Error::Empty);
        }
        let mut tokens = Vec::new();
        let bytes = input.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                byte if byte.is_ascii_whitespace() => index += 1,
                b'(' | b')' | b',' | b';' | b'*' | b'=' | b'<' | b'>' | b'!' => {
                    tokens.push(Token::Symbol(bytes[index]));
                    index += 1;
                }
                b'\'' => {
                    index += 1;
                    let mut value = String::new();
                    let mut closed = false;
                    while index < bytes.len() {
                        if bytes[index] != b'\'' {
                            let start = index;
                            while index < bytes.len() && bytes[index] != b'\'' {
                                index += 1;
                            }
                            let piece = core::str::from_utf8(&bytes[start..index])
                                .map_err(|_| Error::InvalidLiteral)?;
                            value.push_str(piece);
                            if value.len() > MAX_TEXT_BYTES {
                                return Err(Error::TooLarge);
                            }
                            continue;
                        }
                        if index + 1 < bytes.len() && bytes[index + 1] == b'\'' {
                            value.push('\'');
                            index += 2;
                        } else {
                            index += 1;
                            closed = true;
                            break;
                        }
                    }
                    if !closed {
                        return Err(Error::InvalidLiteral);
                    }
                    tokens.push(Token::String(value));
                }
                b'0'..=b'9' | b'-' => {
                    let start = index;
                    if bytes[index] == b'-' {
                        index += 1;
                        if index == bytes.len() || !bytes[index].is_ascii_digit() {
                            return Err(Error::InvalidLiteral);
                        }
                    }
                    while index < bytes.len() && bytes[index].is_ascii_digit() {
                        index += 1;
                    }
                    let text = core::str::from_utf8(&bytes[start..index])
                        .map_err(|_| Error::InvalidLiteral)?;
                    let value = text.parse::<i64>().map_err(|_| Error::InvalidLiteral)?;
                    tokens.push(Token::Number(value));
                }
                byte if is_identifier_start(byte) => {
                    let start = index;
                    index += 1;
                    while index < bytes.len() && is_identifier_continue(bytes[index]) {
                        index += 1;
                    }
                    let word = core::str::from_utf8(&bytes[start..index])
                        .map_err(|_| Error::InvalidIdentifier)?;
                    if word.len() > MAX_IDENTIFIER_BYTES {
                        return Err(Error::TooLarge);
                    }
                    tokens.push(Token::Word(word.to_string()));
                }
                _ => return Err(Error::Syntax),
            }
        }
        tokens.push(Token::Eof);
        Ok(Self {
            tokens,
            position: 0,
        })
    }

    fn parse_statement(&mut self) -> Result<Statement, Error> {
        if self.consume_word("CREATE") {
            self.expect_word("TABLE")?;
            let name = self.identifier()?;
            self.expect_symbol(b'(')?;
            let mut columns = Vec::new();
            loop {
                if columns.len() == MAX_COLUMNS {
                    return Err(Error::TooLarge);
                }
                let column_name = self.identifier()?;
                let ty = self.column_type()?;
                let mut primary_key = false;
                let mut not_null = false;
                loop {
                    if self.consume_word("PRIMARY") {
                        self.expect_word("KEY")?;
                        if primary_key {
                            return Err(Error::Constraint);
                        }
                        primary_key = true;
                        not_null = true;
                    } else if self.consume_word("NOT") {
                        self.expect_word("NULL")?;
                        not_null = true;
                    } else {
                        break;
                    }
                }
                if columns
                    .iter()
                    .any(|column: &Column| column.name == column_name)
                {
                    return Err(Error::ColumnExists);
                }
                columns.push(Column {
                    name: column_name,
                    ty,
                    primary_key,
                    not_null,
                });
                if self.consume_symbol(b')') {
                    break;
                }
                self.expect_symbol(b',')?;
            }
            if columns.is_empty() {
                return Err(Error::Syntax);
            }
            if columns.iter().filter(|column| column.primary_key).count() > 1 {
                return Err(Error::Constraint);
            }
            Ok(Statement::Create { name, columns })
        } else if self.consume_word("DROP") {
            self.expect_word("TABLE")?;
            Ok(Statement::Drop {
                name: self.identifier()?,
            })
        } else if self.consume_word("INSERT") {
            self.expect_word("INTO")?;
            let table = self.identifier()?;
            let columns = if self.consume_symbol(b'(') {
                let mut names = Vec::new();
                loop {
                    names.push(self.identifier()?);
                    if self.consume_symbol(b')') {
                        break;
                    }
                    self.expect_symbol(b',')?;
                }
                Some(names)
            } else {
                None
            };
            self.expect_word("VALUES")?;
            self.expect_symbol(b'(')?;
            let mut values = Vec::new();
            loop {
                values.push(self.value()?);
                if self.consume_symbol(b')') {
                    break;
                }
                self.expect_symbol(b',')?;
            }
            Ok(Statement::Insert {
                table,
                columns,
                values,
            })
        } else if self.consume_word("SELECT") {
            let columns = if self.consume_symbol(b'*') {
                None
            } else {
                let mut names = Vec::new();
                loop {
                    names.push(self.identifier()?);
                    if !self.consume_symbol(b',') {
                        break;
                    }
                }
                Some(names)
            };
            self.expect_word("FROM")?;
            let table = self.identifier()?;
            let predicates = self.predicates()?;
            Ok(Statement::Select {
                table,
                columns,
                predicates,
            })
        } else if self.consume_word("UPDATE") {
            let table = self.identifier()?;
            self.expect_word("SET")?;
            let mut assignments = Vec::new();
            loop {
                let name = self.identifier()?;
                self.expect_symbol(b'=')?;
                let value = self.value()?;
                if assignments.iter().any(|(column, _)| column == &name) {
                    return Err(Error::Constraint);
                }
                assignments.push((name, value));
                if !self.consume_symbol(b',') {
                    break;
                }
            }
            Ok(Statement::Update {
                table,
                assignments,
                predicates: self.predicates()?,
            })
        } else if self.consume_word("DELETE") {
            self.expect_word("FROM")?;
            let table = self.identifier()?;
            Ok(Statement::Delete {
                table,
                predicates: self.predicates()?,
            })
        } else {
            Err(Error::Unsupported)
        }
    }

    fn predicates(&mut self) -> Result<Vec<Predicate>, Error> {
        if !self.consume_word("WHERE") {
            return Ok(Vec::new());
        }
        let mut predicates = Vec::new();
        loop {
            let column = self.identifier()?;
            let (op, value) = if self.consume_word("IS") {
                let not = self.consume_word("NOT");
                self.expect_word("NULL")?;
                (
                    if not {
                        CompareOp::IsNotNull
                    } else {
                        CompareOp::IsNull
                    },
                    None,
                )
            } else {
                let op = match self.symbol_operator()? {
                    b'=' => CompareOp::Equal,
                    b'!' => {
                        self.expect_symbol(b'=')?;
                        CompareOp::NotEqual
                    }
                    b'<' => {
                        if self.consume_symbol(b'=') {
                            CompareOp::LessEqual
                        } else if self.consume_symbol(b'>') {
                            CompareOp::NotEqual
                        } else {
                            CompareOp::Less
                        }
                    }
                    b'>' => {
                        if self.consume_symbol(b'=') {
                            CompareOp::GreaterEqual
                        } else {
                            CompareOp::Greater
                        }
                    }
                    _ => return Err(Error::Syntax),
                };
                (op, Some(self.value()?))
            };
            predicates.push(Predicate { column, op, value });
            if !self.consume_word("AND") {
                break;
            }
        }
        Ok(predicates)
    }

    fn column_type(&mut self) -> Result<SqlType, Error> {
        if self.consume_word("INTEGER") {
            Ok(SqlType::Integer)
        } else if self.consume_word("TEXT") {
            Ok(SqlType::Text)
        } else if self.consume_word("BOOL") || self.consume_word("BOOLEAN") {
            Ok(SqlType::Bool)
        } else {
            Err(Error::InvalidType)
        }
    }

    fn value(&mut self) -> Result<Value, Error> {
        match self.next() {
            Token::Number(value) => Ok(Value::Integer(value)),
            Token::String(value) => Ok(Value::Text(value)),
            Token::Word(word) if word.eq_ignore_ascii_case("TRUE") => Ok(Value::Bool(true)),
            Token::Word(word) if word.eq_ignore_ascii_case("FALSE") => Ok(Value::Bool(false)),
            Token::Word(word) if word.eq_ignore_ascii_case("NULL") => Ok(Value::Null),
            _ => Err(Error::InvalidLiteral),
        }
    }

    fn identifier(&mut self) -> Result<String, Error> {
        match self.next() {
            Token::Word(value) => {
                validate_identifier(&value)?;
                Ok(value)
            }
            _ => Err(Error::InvalidIdentifier),
        }
    }

    fn symbol_operator(&mut self) -> Result<u8, Error> {
        match self.next() {
            Token::Symbol(value) if matches!(value, b'=' | b'!' | b'<' | b'>') => Ok(value),
            _ => Err(Error::Syntax),
        }
    }

    fn expect_word(&mut self, expected: &str) -> Result<(), Error> {
        match self.next() {
            Token::Word(value) if value.eq_ignore_ascii_case(expected) => Ok(()),
            _ => Err(Error::Syntax),
        }
    }

    fn consume_word(&mut self, expected: &str) -> bool {
        match self.tokens.get(self.position) {
            Some(Token::Word(value)) if value.eq_ignore_ascii_case(expected) => {
                self.position += 1;
                true
            }
            _ => false,
        }
    }

    fn expect_symbol(&mut self, expected: u8) -> Result<(), Error> {
        if self.consume_symbol(expected) {
            Ok(())
        } else {
            Err(Error::Syntax)
        }
    }

    fn consume_symbol(&mut self, expected: u8) -> bool {
        match self.tokens.get(self.position) {
            Some(Token::Symbol(value)) if *value == expected => {
                self.position += 1;
                true
            }
            _ => false,
        }
    }

    fn next(&mut self) -> Token {
        let token = self.tokens[self.position].clone();
        self.position += 1;
        token
    }

    fn at_end(&self) -> bool {
        matches!(self.tokens.get(self.position), Some(Token::Eof))
            || matches!(self.tokens.get(self.position), Some(Token::Symbol(b';')))
                && matches!(self.tokens.get(self.position + 1), Some(Token::Eof))
    }
}

fn validate_identifier(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .as_bytes()
            .first()
            .is_some_and(|byte| is_identifier_start(*byte))
        || !value
            .as_bytes()
            .iter()
            .all(|byte| is_identifier_continue(*byte))
    {
        return Err(Error::InvalidIdentifier);
    }
    Ok(())
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

fn column_index(columns: &[Column], name: &str) -> Result<usize, Error> {
    columns
        .iter()
        .position(|column| column.name == name)
        .ok_or(Error::ColumnNotFound)
}

fn selected_columns(
    columns: &[Column],
    requested: Option<&Vec<String>>,
) -> Result<Vec<usize>, Error> {
    match requested {
        Some(names) => names
            .iter()
            .map(|name| column_index(columns, name))
            .collect(),
        None => Ok((0..columns.len()).collect()),
    }
}

fn validate_value(column: &Column, value: &Value) -> Result<(), Error> {
    if matches!(value, Value::Null) {
        return if column.not_null {
            Err(Error::Constraint)
        } else {
            Ok(())
        };
    }
    if value.sql_type() != Some(column.ty) {
        return Err(Error::TypeMismatch);
    }
    if let Value::Text(text) = value {
        if text.len() > MAX_TEXT_BYTES {
            return Err(Error::TooLarge);
        }
    }
    Ok(())
}

fn validate_row(columns: &[Column], rows: &[Vec<Value>], row: &[Value]) -> Result<(), Error> {
    validate_row_values(columns, row)?;
    validate_unique_row(columns, rows, row)
}

fn validate_row_values(columns: &[Column], row: &[Value]) -> Result<(), Error> {
    if row.len() != columns.len() {
        return Err(Error::Constraint);
    }
    for (column, value) in columns.iter().zip(row) {
        validate_value(column, value)?;
    }
    Ok(())
}

fn validate_unique_rows(columns: &[Column], rows: &[Vec<Value>]) -> Result<(), Error> {
    for index in 0..rows.len() {
        validate_unique_row(columns, &rows[..index], &rows[index])?;
    }
    Ok(())
}

fn validate_unique_row(
    columns: &[Column],
    existing: &[Vec<Value>],
    row: &[Value],
) -> Result<(), Error> {
    let Some(primary_key) = columns.iter().position(|column| column.primary_key) else {
        return Ok(());
    };
    if existing
        .iter()
        .any(|candidate| candidate.get(primary_key) == row.get(primary_key))
    {
        return Err(Error::Constraint);
    }
    Ok(())
}

fn predicates_match(
    columns: &[Column],
    row: &[Value],
    predicates: &[Predicate],
) -> Result<bool, Error> {
    for predicate in predicates {
        let index = column_index(columns, &predicate.column)?;
        let value = &row[index];
        let matches = match predicate.op {
            CompareOp::IsNull => matches!(value, Value::Null),
            CompareOp::IsNotNull => !matches!(value, Value::Null),
            op => {
                let expected = predicate.value.as_ref().ok_or(Error::InvalidLiteral)?;
                compare_values(value, expected, op)?
            }
        };
        if !matches {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_predicates(columns: &[Column], predicates: &[Predicate]) -> Result<(), Error> {
    for predicate in predicates {
        let index = column_index(columns, &predicate.column)?;
        let Some(value) = predicate.value.as_ref() else {
            continue;
        };
        if matches!(value, Value::Null) {
            return Err(Error::InvalidLiteral);
        }
        if value.sql_type() != Some(columns[index].ty) {
            return Err(Error::TypeMismatch);
        }
    }
    Ok(())
}

fn compare_values(left: &Value, right: &Value, op: CompareOp) -> Result<bool, Error> {
    if matches!(left, Value::Null) || matches!(right, Value::Null) {
        return Err(Error::InvalidLiteral);
    }
    let ordering = match (left, right) {
        (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
        (Value::Text(left), Value::Text(right)) => left.as_bytes().cmp(right.as_bytes()),
        (Value::Bool(left), Value::Bool(right)) => left.cmp(right),
        _ => return Err(Error::TypeMismatch),
    };
    Ok(match op {
        CompareOp::Equal => ordering == core::cmp::Ordering::Equal,
        CompareOp::NotEqual => ordering != core::cmp::Ordering::Equal,
        CompareOp::Less => ordering == core::cmp::Ordering::Less,
        CompareOp::LessEqual => ordering != core::cmp::Ordering::Greater,
        CompareOp::Greater => ordering == core::cmp::Ordering::Greater,
        CompareOp::GreaterEqual => ordering != core::cmp::Ordering::Less,
        CompareOp::IsNull | CompareOp::IsNotNull => false,
    })
}

fn put_u16(output: &mut Vec<u8>, value: usize) -> Result<(), Error> {
    let value = u16::try_from(value).map_err(|_| Error::TooLarge)?;
    output.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn put_u32(output: &mut Vec<u8>, value: usize) -> Result<(), Error> {
    let value = u32::try_from(value).map_err(|_| Error::TooLarge)?;
    output.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn put_string(output: &mut Vec<u8>, value: &str) -> Result<(), Error> {
    put_u16(output, value.len())?;
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn encode_value(output: &mut Vec<u8>, value: &Value) -> Result<(), Error> {
    match value {
        Value::Null => output.push(0),
        Value::Integer(value) => {
            output.push(1);
            output.extend_from_slice(&value.to_le_bytes());
        }
        Value::Text(value) => {
            if value.len() > MAX_TEXT_BYTES {
                return Err(Error::TooLarge);
            }
            output.push(2);
            put_u16(output, value.len())?;
            output.extend_from_slice(value.as_bytes());
        }
        Value::Bool(value) => {
            output.push(3);
            output.push(u8::from(*value));
        }
    }
    Ok(())
}

fn decode_value(cursor: &mut Cursor<'_>) -> Result<Value, Error> {
    match cursor.byte()? {
        0 => Ok(Value::Null),
        1 => Ok(Value::Integer(cursor.i64()?)),
        2 => Ok(Value::Text(cursor.string()?)),
        3 => match cursor.byte()? {
            0 => Ok(Value::Bool(false)),
            1 => Ok(Value::Bool(true)),
            _ => Err(Error::Corrupt),
        },
        _ => Err(Error::Corrupt),
    }
}

fn read_u16(input: &[u8], offset: usize) -> Result<u16, Error> {
    let bytes = input.get(offset..offset + 2).ok_or(Error::Corrupt)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(input: &[u8], offset: usize) -> Result<u32, Error> {
    let bytes = input.get(offset..offset + 4).ok_or(Error::Corrupt)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

struct Cursor<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    fn byte(&mut self) -> Result<u8, Error> {
        let byte = *self.input.get(self.offset).ok_or(Error::Corrupt)?;
        self.offset += 1;
        Ok(byte)
    }

    fn u16(&mut self) -> Result<u16, Error> {
        let start = self.offset;
        self.offset = self.offset.checked_add(2).ok_or(Error::Corrupt)?;
        let bytes = self.input.get(start..self.offset).ok_or(Error::Corrupt)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let start = self.offset;
        self.offset = self.offset.checked_add(4).ok_or(Error::Corrupt)?;
        let bytes = self.input.get(start..self.offset).ok_or(Error::Corrupt)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i64(&mut self) -> Result<i64, Error> {
        let start = self.offset;
        self.offset = self.offset.checked_add(8).ok_or(Error::Corrupt)?;
        let bytes = self.input.get(start..self.offset).ok_or(Error::Corrupt)?;
        Ok(i64::from_le_bytes(
            bytes.try_into().map_err(|_| Error::Corrupt)?,
        ))
    }

    fn string(&mut self) -> Result<String, Error> {
        let length = self.u16()? as usize;
        if length > MAX_TEXT_BYTES.max(MAX_IDENTIFIER_BYTES) {
            return Err(Error::Corrupt);
        }
        let start = self.offset;
        self.offset = self.offset.checked_add(length).ok_or(Error::Corrupt)?;
        let bytes = self.input.get(start..self.offset).ok_or(Error::Corrupt)?;
        let value = core::str::from_utf8(bytes).map_err(|_| Error::Corrupt)?;
        Ok(value.to_string())
    }

    fn at_end(&self) -> bool {
        self.offset == self.input.len()
    }
}

fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82f6_3b78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
