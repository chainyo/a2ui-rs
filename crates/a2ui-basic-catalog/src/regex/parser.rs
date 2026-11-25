use std::iter::Peekable;
use std::str::Chars;

use super::Error;
use super::syntax::{Anchor, CharSet, Node};

const WHITESPACE: &[(u32, u32)] = &[
    (0x09, 0x0D),
    (0x20, 0x20),
    (0xA0, 0xA0),
    (0x1680, 0x1680),
    (0x2000, 0x200A),
    (0x2028, 0x2029),
    (0x202F, 0x202F),
    (0x205F, 0x205F),
    (0x3000, 0x3000),
    (0xFEFF, 0xFEFF),
];

const WORD: &[(u32, u32)] = &[(48, 57), (65, 90), (95, 95), (97, 122)];
const DIGIT: &[(u32, u32)] = &[(48, 57)];
const DOT_EXCLUDED: &[(u32, u32)] = &[(10, 10), (13, 13), (0x2028, 0x2029)];

pub(super) struct Parser<'a>(Peekable<Chars<'a>>);

enum ClassAtom {
    Single(char),
    Set(CharSet),
}

impl Parser<'_> {
    pub(super) fn parse(pattern: &str) -> Result<Node, Error> {
        let mut parser = Parser(pattern.chars().peekable());
        let node = parser.alternation()?;
        if parser.peek().is_some() {
            return Err(Error::InvalidSyntax);
        }
        Ok(node)
    }

    fn peek(&mut self) -> Option<char> {
        self.0.peek().copied()
    }

    fn eat(&mut self, c: char) -> bool {
        self.0.next_if_eq(&c).is_some()
    }

    fn expect(&mut self, c: char) -> Result<(), Error> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(Error::InvalidSyntax)
        }
    }

    fn alternation(&mut self) -> Result<Node, Error> {
        let first = self.sequence()?;
        if !self.eat('|') {
            return Ok(first);
        }
        let mut branches = vec![first, self.sequence()?];
        while self.eat('|') {
            branches.push(self.sequence()?);
        }
        Ok(Node::Alternation(branches))
    }

    fn sequence(&mut self) -> Result<Node, Error> {
        let mut items = Vec::new();
        while self.peek().is_some_and(|c| !matches!(c, ')' | '|')) {
            let (atom, assertion) = self.atom()?;
            let item = if matches!(self.peek(), Some('*' | '+' | '?' | '{')) {
                if assertion {
                    return Err(Error::InvalidSyntax);
                }
                self.quantify(atom)?
            } else {
                atom
            };
            items.push(item);
        }
        Ok(Node::sequence(items))
    }

    fn atom(&mut self) -> Result<(Node, bool), Error> {
        let c = self.0.next().ok_or(Error::InvalidSyntax)?;
        let atom = match c {
            '^' => return Ok((Node::Anchor(Anchor::Start), true)),
            '$' => return Ok((Node::Anchor(Anchor::End), true)),
            '.' => Node::Char(CharSet::from_ranges(DOT_EXCLUDED).complement()),
            '[' => Node::Char(self.character_class()?),
            '(' => return self.group(),
            '\\' => match self.peek() {
                Some('b' | 'B') => {
                    let anchor = if self.eat('b') {
                        Anchor::Word
                    } else {
                        self.0.next();
                        Anchor::NonWord
                    };
                    return Ok((Node::Anchor(anchor), true));
                }
                _ => Node::Char(self.escape(false)?.into_set()),
            },
            '*' | '+' | '?' | '{' | '}' | ']' => return Err(Error::InvalidSyntax),
            c => Node::Char(CharSet::single(c)),
        };
        Ok((atom, false))
    }

    fn group(&mut self) -> Result<(Node, bool), Error> {
        let look = if self.eat('?') {
            match self.0.next() {
                Some(':') => None,
                Some('=') => Some(true),
                Some('!') => Some(false),
                _ => return Err(Error::UnsupportedSyntax),
            }
        } else {
            None
        };
        let node = self.alternation()?;
        self.expect(')')?;
        Ok(match look {
            Some(positive) => (
                Node::Lookahead {
                    positive,
                    node: Box::new(node),
                },
                true,
            ),
            None => (node, false),
        })
    }

    fn quantify(&mut self, node: Node) -> Result<Node, Error> {
        let (min, max) = match self.0.next() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => {
                let min = self.decimal()?.ok_or(Error::InvalidSyntax)?;
                let max = if self.eat(',') {
                    self.decimal()?
                } else {
                    Some(min)
                };
                self.expect('}')?;
                if max.is_some_and(|max| max < min) {
                    return Err(Error::InvalidSyntax);
                }
                (min, max)
            }
            _ => return Err(Error::InvalidSyntax),
        };
        Ok(Node::Repeat {
            node: Box::new(node),
            min,
            max,
            lazy: self.eat('?'),
        })
    }

    fn decimal(&mut self) -> Result<Option<u32>, Error> {
        let mut value: Option<u32> = None;
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(10)) {
            self.0.next();
            value = Some(
                value
                    .unwrap_or(0)
                    .checked_mul(10)
                    .and_then(|value| value.checked_add(digit))
                    .ok_or(Error::InvalidSyntax)?,
            );
        }
        Ok(value)
    }

    fn character_class(&mut self) -> Result<CharSet, Error> {
        let negated = self.eat('^');
        let mut set = CharSet::default();
        while !self.eat(']') {
            let left = self.class_atom()?;
            let is_range = self.peek() == Some('-') && self.0.clone().nth(1) != Some(']');
            if is_range {
                self.0.next();
                let right = self.class_atom()?;
                let (ClassAtom::Single(start), ClassAtom::Single(end)) = (left, right) else {
                    return Err(Error::InvalidSyntax);
                };
                if start > end {
                    return Err(Error::InvalidSyntax);
                }
                set.extend(CharSet::from_ranges(&[(u32::from(start), u32::from(end))]));
            } else {
                set.extend(left.into_set());
            }
        }
        Ok(if negated { set.complement() } else { set })
    }

    fn class_atom(&mut self) -> Result<ClassAtom, Error> {
        match self.0.next() {
            Some('\\') => self.escape(true),
            Some(']') | None => Err(Error::InvalidSyntax),
            Some(c) => Ok(ClassAtom::Single(c)),
        }
    }

    fn escape(&mut self, in_class: bool) -> Result<ClassAtom, Error> {
        let c = self.0.next().ok_or(Error::InvalidSyntax)?;
        let class = match c.to_ascii_lowercase() {
            'd' => Some(DIGIT),
            's' => Some(WHITESPACE),
            'w' => Some(WORD),
            _ => None,
        };
        if let Some(class) = class {
            let set = CharSet::from_ranges(class);
            return Ok(ClassAtom::Set(if c.is_ascii_uppercase() {
                set.complement()
            } else {
                set
            }));
        }
        let literal = match c {
            '0' if !self.peek().is_some_and(|c| c.is_ascii_digit()) => '\0',
            'b' if in_class => '\u{8}',
            't' => '\t',
            'n' => '\n',
            'v' => '\u{B}',
            'f' => '\u{C}',
            'r' => '\r',
            'x' => self.hex(2)?,
            'u' if self.eat('{') => self.braced_hex()?,
            'u' => self.hex(4)?,
            '-' if in_class => c,
            c if "^$\\.*+?()[]{}|/".contains(c) => c,
            _ => return Err(Error::UnsupportedSyntax),
        };
        Ok(ClassAtom::Single(literal))
    }

    fn hex(&mut self, count: usize) -> Result<char, Error> {
        let mut value = 0;
        for _ in 0..count {
            value = value * 16
                + self
                    .0
                    .next()
                    .and_then(|c| c.to_digit(16))
                    .ok_or(Error::InvalidSyntax)?;
        }
        code_point(value)
    }

    fn braced_hex(&mut self) -> Result<char, Error> {
        let mut value: Option<u32> = None;
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(16)) {
            self.0.next();
            value = Some(
                value
                    .unwrap_or(0)
                    .checked_mul(16)
                    .and_then(|value| value.checked_add(digit))
                    .ok_or(Error::InvalidSyntax)?,
            );
        }
        self.expect('}')?;
        code_point(value.ok_or(Error::InvalidSyntax)?)
    }
}

fn code_point(value: u32) -> Result<char, Error> {
    if (0xD800..=0xDFFF).contains(&value) {
        return Err(Error::UnsupportedSyntax);
    }
    char::from_u32(value).ok_or(Error::InvalidSyntax)
}

impl ClassAtom {
    fn into_set(self) -> CharSet {
        match self {
            Self::Single(c) => CharSet::single(c),
            Self::Set(set) => set,
        }
    }
}
