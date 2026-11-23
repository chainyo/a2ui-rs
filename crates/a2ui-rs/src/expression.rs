//! Parses A2UI v1.0 interpolation templates.

use serde_json::{Number, Value, json};
use thiserror::Error;

use crate::protocol::{FunctionCall, JsonObject};

const MAX_DEPTH: usize = 100;

/// A literal, data binding, or local function call in a template.
#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    /// A string, number, or boolean.
    Literal(Value),
    /// A relative or absolute JSON Pointer.
    Binding(String),
    /// A function call with named arguments.
    Call(FunctionCall),
}

impl Expression {
    pub(crate) fn into_value(self) -> Value {
        match self {
            Self::Literal(value) => value,
            Self::Binding(path) => json!({"@path": path}),
            Self::Call(call) => json!({"@call": call.name, "args": call.args}),
        }
    }
}

/// Parses a string with `${...}` expressions into parts.
///
/// # Errors
/// Returns an error if an expression is invalid or exceeds the nesting limit.
pub fn parse_template(input: &str) -> Result<Vec<Expression>, ParseError> {
    let mut parser = Parser { rest: input };
    let mut parts = Vec::new();
    let mut literal = String::new();
    while !parser.rest.is_empty() {
        if parser.consume(r"\${") {
            literal.push_str("${");
        } else if parser.consume("${") {
            match parser.interpolation(1)? {
                Expression::Literal(Value::Null) => {}
                Expression::Literal(Value::String(value)) => literal.push_str(&value),
                expression => {
                    flush(&mut literal, &mut parts);
                    parts.push(expression);
                }
            }
        } else if let Some(ch) = parser.next() {
            literal.push(ch);
        }
    }
    flush(&mut literal, &mut parts);
    Ok(parts)
}

fn flush(literal: &mut String, parts: &mut Vec<Expression>) {
    if !literal.is_empty() {
        parts.push(Expression::Literal(Value::String(std::mem::take(literal))));
    }
}

/// An invalid expression or template.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ParseError {
    /// The interpolation has no closing brace.
    #[error("Unclosed interpolation")]
    UnclosedInterpolation,
    /// The expression has extra characters.
    #[error("Unexpected characters in expression")]
    UnexpectedCharacters,
    /// The argument lacks a name or colon.
    #[error("Expected ':' after argument name")]
    ArgumentName,
    /// The call has no closing parenthesis or comma.
    #[error("Expected ')' or ',' after function arguments")]
    FunctionArguments,
    /// The same argument appears twice.
    #[error("Duplicate function argument")]
    DuplicateArgument,
    /// The quoted string has no closing quote.
    #[error("Unclosed quoted string")]
    UnclosedString,
    /// The number has invalid syntax.
    #[error("Invalid number literal")]
    InvalidNumber,
    /// The number exceeds the supported range.
    #[error("Number literal out of range")]
    NumberRange,
    /// The expression exceeds the nesting limit.
    #[error("Max recursion depth exceeded")]
    Depth,
}

struct Parser<'a> {
    rest: &'a str,
}

impl<'a> Parser<'a> {
    fn consume(&mut self, prefix: &str) -> bool {
        if let Some(rest) = self.rest.strip_prefix(prefix) {
            self.rest = rest;
            true
        } else {
            false
        }
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.rest.chars().next()?;
        self.rest = &self.rest[ch.len_utf8()..];
        Some(ch)
    }

    fn take(&mut self, predicate: impl Fn(char) -> bool) -> &'a str {
        let end = self
            .rest
            .find(|ch| !predicate(ch))
            .unwrap_or(self.rest.len());
        let (token, rest) = self.rest.split_at(end);
        self.rest = rest;
        token
    }

    fn whitespace(&mut self) {
        self.rest = self.rest.trim_start();
    }

    fn interpolation(&mut self, depth: usize) -> Result<Expression, ParseError> {
        let expression = self.expression(depth)?;
        self.whitespace();
        if self.consume("}") {
            Ok(expression)
        } else if self.rest.is_empty() {
            Err(ParseError::UnclosedInterpolation)
        } else {
            Err(ParseError::UnexpectedCharacters)
        }
    }

    fn expression(&mut self, depth: usize) -> Result<Expression, ParseError> {
        if depth > MAX_DEPTH {
            return Err(ParseError::Depth);
        }
        self.whitespace();
        if self.consume("${") {
            return self.interpolation(depth + 1);
        }
        if let Some(quote @ ('\'' | '"')) = self.rest.chars().next() {
            self.next();
            return self
                .quoted(quote)
                .map(|value| Expression::Literal(Value::String(value)));
        }
        if starts_number(self.rest) {
            return self
                .number()
                .map(|value| Expression::Literal(Value::Number(value)));
        }

        let system = self.consume("@");
        let token = self.take(|ch| {
            unicode_ident::is_xid_continue(ch) || matches!(ch, '_' | '/' | '-' | '.' | '~')
        });
        if token.is_empty() {
            return Err(if self.rest.is_empty() {
                ParseError::UnclosedInterpolation
            } else {
                ParseError::UnexpectedCharacters
            });
        }
        self.whitespace();
        if self.consume("(") {
            if !is_identifier(token) {
                return Err(ParseError::UnexpectedCharacters);
            }
            let name = if system {
                format!("@{token}")
            } else {
                token.to_owned()
            };
            return self.call(name, depth);
        }
        if system {
            return Err(ParseError::UnexpectedCharacters);
        }
        Ok(match token {
            "true" => Expression::Literal(Value::Bool(true)),
            "false" => Expression::Literal(Value::Bool(false)),
            "null" => Expression::Literal(Value::Null),
            path => Expression::Binding(path.to_owned()),
        })
    }

    fn quoted(&mut self, quote: char) -> Result<String, ParseError> {
        let mut value = String::new();
        while let Some(ch) = self.next() {
            if ch == quote {
                return Ok(value);
            }
            let ch = if ch == '\\' {
                match self.next().ok_or(ParseError::UnclosedString)? {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    ch => ch,
                }
            } else {
                ch
            };
            value.push(ch);
        }
        Err(ParseError::UnclosedString)
    }

    fn call(&mut self, name: String, depth: usize) -> Result<Expression, ParseError> {
        let mut args = JsonObject::new();
        loop {
            self.whitespace();
            if self.consume(")") {
                break;
            }
            let argument = self.take(|ch| ch == '_' || unicode_ident::is_xid_continue(ch));
            self.whitespace();
            if !is_identifier(argument) || !self.consume(":") {
                return Err(ParseError::ArgumentName);
            }
            let mut value = self.expression(depth + 1)?.into_value();
            if value.is_null() {
                value = Value::String(String::new());
            }
            if args.insert(argument.to_owned(), value).is_some() {
                return Err(ParseError::DuplicateArgument);
            }
            self.whitespace();
            if self.consume(")") {
                break;
            }
            if !self.consume(",") {
                return Err(ParseError::FunctionArguments);
            }
        }
        Ok(Expression::Call(FunctionCall {
            name,
            catalog_id: None,
            args: Some(args),
        }))
    }

    fn number(&mut self) -> Result<Number, ParseError> {
        let source = self.rest;
        if !self.consume("-") {
            self.consume("+");
        }
        self.take(|ch| ch.is_ascii_digit());
        if self.consume(".") {
            self.take(|ch| ch.is_ascii_digit());
        }
        if (self.consume("e") || self.consume("E")) && {
            if !self.consume("-") {
                self.consume("+");
            }
            self.take(|ch| ch.is_ascii_digit()).is_empty()
        } {
            return Err(ParseError::InvalidNumber);
        }
        if self.rest.starts_with('.') {
            return Err(ParseError::InvalidNumber);
        }
        let number = &source[..source.len() - self.rest.len()];
        if let Ok(integer) = number.parse::<i64>() {
            return Ok(integer.into());
        }
        let number: f64 = number.parse().map_err(|_| ParseError::InvalidNumber)?;
        Number::from_f64(number).ok_or(ParseError::NumberRange)
    }
}

fn starts_number(input: &str) -> bool {
    let input = input.strip_prefix(['+', '-']).unwrap_or(input);
    let input = input.strip_prefix('.').unwrap_or(input);
    input.starts_with(|ch: char| ch.is_ascii_digit())
}

pub(crate) fn is_identifier(input: &str) -> bool {
    let mut chars = input.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || unicode_ident::is_xid_start(ch))
        && chars.all(unicode_ident::is_xid_continue)
}
