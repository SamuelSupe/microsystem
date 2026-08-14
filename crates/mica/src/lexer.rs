use alloc::string::{String, ToString};

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Eof,
    Identifier(String),
    Integer(i64),
    Float(f64),
    String(String),
    Local,
    Function,
    If,
    Then,
    Else,
    ElseIf,
    End,
    While,
    Do,
    For,
    In,
    Break,
    Return,
    True,
    False,
    Nil,
    And,
    Or,
    Not,
    Plus,
    Minus,
    Star,
    Slash,
    SlashSlash,
    Percent,
    Equal,
    EqualEqual,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Dot,
    Colon,
    Semicolon,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug)]
pub struct LexError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

pub fn lex(source: &str) -> Result<alloc::vec::Vec<Token>, LexError> {
    Lexer::new(source).all()
}

struct Lexer<'a> {
    source: &'a [u8],
    cursor: usize,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            cursor: 0,
            line: 1,
            column: 1,
        }
    }

    fn all(mut self) -> Result<alloc::vec::Vec<Token>, LexError> {
        let mut tokens = alloc::vec::Vec::new();
        loop {
            let token = self.next()?;
            let eof = token.kind == TokenKind::Eof;
            tokens.push(token);
            if eof {
                return Ok(tokens);
            }
        }
    }

    fn next(&mut self) -> Result<Token, LexError> {
        self.skip_space_and_comments();
        let line = self.line;
        let column = self.column;
        let Some(byte) = self.peek() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                line,
                column,
            });
        };
        if byte.is_ascii_alphabetic() || byte == b'_' {
            return Ok(Token {
                kind: self.identifier(),
                line,
                column,
            });
        }
        if byte.is_ascii_digit() {
            return self.number(line, column);
        }
        if matches!(byte, b'\'' | b'"') {
            return self.string(line, column);
        }
        self.advance();
        let kind = match byte {
            b'+' => TokenKind::Plus,
            b'-' => TokenKind::Minus,
            b'*' => TokenKind::Star,
            b'/' if self.take(b'/') => TokenKind::SlashSlash,
            b'/' => TokenKind::Slash,
            b'%' => TokenKind::Percent,
            b'=' if self.take(b'=') => TokenKind::EqualEqual,
            b'=' => TokenKind::Equal,
            b'~' if self.take(b'=') => TokenKind::NotEqual,
            b'<' if self.take(b'=') => TokenKind::LessEqual,
            b'<' => TokenKind::Less,
            b'>' if self.take(b'=') => TokenKind::GreaterEqual,
            b'>' => TokenKind::Greater,
            b'(' => TokenKind::LeftParen,
            b')' => TokenKind::RightParen,
            b'{' => TokenKind::LeftBrace,
            b'}' => TokenKind::RightBrace,
            b'[' => TokenKind::LeftBracket,
            b']' => TokenKind::RightBracket,
            b',' => TokenKind::Comma,
            b'.' => TokenKind::Dot,
            b':' => TokenKind::Colon,
            b';' => TokenKind::Semicolon,
            _ => return Err(self.error(line, column, "unexpected character")),
        };
        Ok(Token { kind, line, column })
    }

    fn skip_space_and_comments(&mut self) {
        loop {
            while self.peek().is_some_and(|value| value.is_ascii_whitespace()) {
                self.advance();
            }
            if self.peek() == Some(b'-') && self.peek_next() == Some(b'-') {
                while self.peek().is_some_and(|value| value != b'\n') {
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    fn identifier(&mut self) -> TokenKind {
        let start = self.cursor;
        while self
            .peek()
            .is_some_and(|value| value.is_ascii_alphanumeric() || value == b'_')
        {
            self.advance();
        }
        let value = core::str::from_utf8(&self.source[start..self.cursor]).unwrap_or("");
        match value {
            "local" => TokenKind::Local,
            "function" => TokenKind::Function,
            "if" => TokenKind::If,
            "then" => TokenKind::Then,
            "else" => TokenKind::Else,
            "elseif" => TokenKind::ElseIf,
            "end" => TokenKind::End,
            "while" => TokenKind::While,
            "do" => TokenKind::Do,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "break" => TokenKind::Break,
            "return" => TokenKind::Return,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "nil" => TokenKind::Nil,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => TokenKind::Not,
            _ => TokenKind::Identifier(value.to_string()),
        }
    }

    fn number(&mut self, line: usize, column: usize) -> Result<Token, LexError> {
        let start = self.cursor;
        if self.peek() == Some(b'0') && matches!(self.peek_next(), Some(b'x' | b'X')) {
            self.advance();
            self.advance();
            let digits = self.cursor;
            while self.peek().is_some_and(|value| value.is_ascii_hexdigit()) {
                self.advance();
            }
            if digits == self.cursor {
                return Err(self.error(line, column, "invalid hexadecimal integer"));
            }
            let text = core::str::from_utf8(&self.source[digits..self.cursor]).unwrap_or("");
            let value = i64::from_str_radix(text, 16)
                .map_err(|_| self.error(line, column, "integer out of range"))?;
            return Ok(Token {
                kind: TokenKind::Integer(value),
                line,
                column,
            });
        }
        while self.peek().is_some_and(|value| value.is_ascii_digit()) {
            self.advance();
        }
        let mut float = false;
        if self.peek() == Some(b'.') && self.peek_next().is_some_and(|value| value.is_ascii_digit())
        {
            float = true;
            self.advance();
            while self.peek().is_some_and(|value| value.is_ascii_digit()) {
                self.advance();
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            float = true;
            self.advance();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.advance();
            }
            while self.peek().is_some_and(|value| value.is_ascii_digit()) {
                self.advance();
            }
        }
        let text = core::str::from_utf8(&self.source[start..self.cursor]).unwrap_or("");
        let kind = if float {
            TokenKind::Float(
                text.parse()
                    .map_err(|_| self.error(line, column, "invalid number"))?,
            )
        } else {
            TokenKind::Integer(
                text.parse()
                    .map_err(|_| self.error(line, column, "integer out of range"))?,
            )
        };
        Ok(Token { kind, line, column })
    }

    fn string(&mut self, line: usize, column: usize) -> Result<Token, LexError> {
        let quote = self.peek().unwrap();
        self.advance();
        let mut value = String::new();
        let mut segment = self.cursor;
        while let Some(byte) = self.peek() {
            if byte == quote {
                value.push_str(core::str::from_utf8(&self.source[segment..self.cursor]).unwrap());
                self.advance();
                return Ok(Token {
                    kind: TokenKind::String(value),
                    line,
                    column,
                });
            }
            if byte == b'\n' {
                return Err(self.error(line, column, "unterminated string"));
            }
            self.advance();
            if byte != b'\\' {
                continue;
            }
            value.push_str(core::str::from_utf8(&self.source[segment..self.cursor - 1]).unwrap());
            let escaped = self
                .peek()
                .ok_or_else(|| self.error(line, column, "unterminated escape"))?;
            self.advance();
            match escaped {
                b'n' => value.push('\n'),
                b'r' => value.push('\r'),
                b't' => value.push('\t'),
                b'\\' => value.push('\\'),
                b'\'' => value.push('\''),
                b'"' => value.push('"'),
                _ => return Err(self.error(self.line, self.column, "unsupported escape")),
            }
            segment = self.cursor;
        }
        Err(self.error(line, column, "unterminated string"))
    }

    fn take(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.cursor).copied()
    }

    fn peek_next(&self) -> Option<u8> {
        self.source.get(self.cursor + 1).copied()
    }

    fn advance(&mut self) {
        if let Some(byte) = self.source.get(self.cursor) {
            self.cursor += 1;
            if *byte == b'\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
    }

    fn error(&self, line: usize, column: usize, message: &str) -> LexError {
        LexError {
            line,
            column,
            message: message.to_string(),
        }
    }
}
