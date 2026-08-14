use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Table(Vec<(Value, Value)>),
    Function(u16),
    Native(String),
}

impl Value {
    pub fn truthy(&self) -> bool {
        !matches!(self, Self::Nil | Self::Bool(false))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Nil => "nil",
            Self::Bool(_) => "boolean",
            Self::Integer(_) => "integer",
            Self::Float(_) => "number",
            Self::String(_) => "string",
            Self::Bytes(_) => "bytes",
            Self::Table(_) => "table",
            Self::Function(_) | Self::Native(_) => "function",
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Nil => "nil".to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Integer(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::String(value) => value.clone(),
            Self::Bytes(value) => {
                let mut output = String::from("bytes(");
                output.push_str(&value.len().to_string());
                output.push(')');
                output
            }
            Self::Table(_) => "table".to_string(),
            Self::Function(_) | Self::Native(_) => "function".to_string(),
        }
    }

    pub fn table_get(&self, key: &Value) -> Option<&Value> {
        let Self::Table(entries) = self else {
            return None;
        };
        entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    pub fn table_set(&mut self, key: Value, value: Value) -> Result<(), ErrorValue> {
        let Self::Table(entries) = self else {
            return Err(ErrorValue::new("type", "table assignment requires table"));
        };
        if let Some((_, existing)) = entries.iter_mut().find(|(candidate, _)| candidate == &key) {
            *existing = value;
        } else {
            entries.push((key, value));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorValue {
    pub kind: String,
    pub message: String,
    pub operation: String,
    pub code: i64,
}

impl ErrorValue {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            operation: String::new(),
            code: 0,
        }
    }

    pub fn operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = operation.into();
        self
    }

    pub fn code(mut self, code: i64) -> Self {
        self.code = code;
        self
    }
}

impl fmt::Display for ErrorValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.operation.is_empty() {
            formatter.write_str(&self.message)
        } else {
            write!(formatter, "{}: {}", self.operation, self.message)
        }
    }
}
