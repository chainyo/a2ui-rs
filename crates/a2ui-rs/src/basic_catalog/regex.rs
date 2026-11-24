use fancy_regex::RegexBuilder;

mod parser;
mod syntax;

use parser::Parser;

const MAX_PATTERN: usize = 256;
const MAX_VALUE: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
    #[error("PATTERN_TOO_LONG")]
    PatternTooLong,
    #[error("UNSUPPORTED_SYNTAX")]
    UnsupportedSyntax,
    #[error("INVALID_SYNTAX")]
    InvalidSyntax,
    #[error("NESTED_QUANTIFIER")]
    NestedQuantifier,
    #[error("OVERLAPPING_ALTERNATION")]
    OverlappingAlternation,
    #[error("ADJACENT_OVERLAP")]
    AdjacentOverlap,
    #[error("Regex compilation failed: {0}")]
    Compile(Box<fancy_regex::Error>),
    #[error("Regex matching failed: {0}")]
    Match(Box<fancy_regex::Error>),
}

pub(super) fn matches(pattern: &str, value: &str) -> Result<bool, Error> {
    if pattern.chars().take(MAX_PATTERN + 1).count() > MAX_PATTERN {
        return Err(Error::PatternTooLong);
    }
    let tree = Parser::parse(pattern)?;
    tree.validate()?;
    if value.chars().take(MAX_VALUE + 1).count() > MAX_VALUE {
        return Ok(false);
    }
    let mut translated = String::new();
    tree.translate(&mut translated);
    RegexBuilder::new(&translated)
        .backtrack_limit(1_000_000)
        .delegate_size_limit(1 << 20)
        .build()
        .map_err(|error| Error::Compile(Box::new(error)))?
        .is_match(value)
        .map_err(|error| Error::Match(Box::new(error)))
}
