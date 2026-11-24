use std::fmt::Write;

use super::Error;

const WORD_BOUNDARY: &str =
    "(?:(?<=[A-Za-z0-9_])(?![A-Za-z0-9_])|(?<![A-Za-z0-9_])(?=[A-Za-z0-9_]))";
const MAX_SCALAR: u32 = 0x10_FFFF;
const SCALAR_RANGES: [(u32, u32); 2] = [(0, 0xD7FF), (0xE000, MAX_SCALAR)];

#[derive(Default)]
pub(super) struct CharSet(Vec<(u32, u32)>);

impl CharSet {
    pub(super) fn from_ranges(ranges: &[(u32, u32)]) -> Self {
        let mut set = Self::default();
        for &(start, end) in ranges {
            for (low, high) in SCALAR_RANGES {
                let clipped = (start.max(low), end.min(high));
                if clipped.0 <= clipped.1 {
                    set.0.push(clipped);
                }
            }
        }
        set
    }

    pub(super) fn single(character: char) -> Self {
        Self(vec![(u32::from(character), u32::from(character))])
    }

    pub(super) fn extend(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    pub(super) fn complement(mut self) -> Self {
        self.0.sort_unstable();
        let mut gaps = Vec::new();
        let mut next = 0;
        for (start, end) in self.0 {
            if next < start {
                gaps.push((next, start - 1));
            }
            next = next.max(end + 1);
        }
        if next <= MAX_SCALAR {
            gaps.push((next, MAX_SCALAR));
        }
        Self::from_ranges(&gaps)
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.0.iter().any(|&(start, end)| {
            other
                .0
                .iter()
                .any(|&(other_start, other_end)| start <= other_end && other_start <= end)
        })
    }

    fn translate(&self, output: &mut String) {
        if self.0.is_empty() {
            output.push_str("[^\\x{0}-\\x{10FFFF}]");
            return;
        }
        output.push('[');
        for &(start, end) in &self.0 {
            let _ = write!(output, "\\x{{{start:X}}}");
            if start != end {
                let _ = write!(output, "-\\x{{{end:X}}}");
            }
        }
        output.push(']');
    }
}

#[derive(Clone, Copy)]
pub(super) enum Anchor {
    Start,
    End,
    Word,
    NonWord,
}

pub(super) enum Node {
    Empty,
    Char(CharSet),
    Anchor(Anchor),
    Lookahead {
        positive: bool,
        node: Box<Self>,
    },
    Concat(Vec<Self>),
    Alternation(Vec<Self>),
    Repeat {
        node: Box<Self>,
        min: u32,
        max: Option<u32>,
        lazy: bool,
    },
}

impl Node {
    pub(super) fn sequence(mut items: Vec<Self>) -> Self {
        match items.len() {
            0 => Self::Empty,
            1 => items.pop().unwrap_or(Self::Empty),
            _ => Self::Concat(items),
        }
    }

    fn children(&self) -> &[Self] {
        match self {
            Self::Lookahead { node, .. } | Self::Repeat { node, .. } => std::slice::from_ref(node),
            Self::Concat(items) | Self::Alternation(items) => items,
            _ => &[],
        }
    }

    fn nullable(&self) -> bool {
        match self {
            Self::Char(_) => false,
            Self::Alternation(items) => items.iter().any(Self::nullable),
            Self::Concat(items) => items.iter().all(Self::nullable),
            Self::Repeat { node, min, .. } => *min == 0 || node.nullable(),
            _ => true,
        }
    }

    fn first(&self, set: &mut CharSet) {
        match self {
            Self::Char(chars) => set.0.extend_from_slice(&chars.0),
            Self::Concat(items) => {
                for item in items {
                    item.first(set);
                    if !item.nullable() {
                        break;
                    }
                }
            }
            Self::Alternation(items) => items.iter().for_each(|item| item.first(set)),
            Self::Repeat { node, max, .. } if *max != Some(0) => node.first(set),
            _ => {}
        }
    }

    fn consumed(&self, set: &mut CharSet) {
        match self {
            Self::Char(chars) => set.0.extend_from_slice(&chars.0),
            Self::Repeat { max: Some(0), .. } | Self::Lookahead { .. } => {}
            _ => self.children().iter().for_each(|child| child.consumed(set)),
        }
    }

    fn has_unbounded(&self) -> bool {
        matches!(self, Self::Repeat { max: None, .. })
            || self.children().iter().any(Self::has_unbounded)
    }

    pub(super) fn validate(&self) -> Result<(), Error> {
        if let Self::Repeat { node, max, .. } = self {
            if max.is_none_or(|max| max > 1) && node.has_unbounded() {
                return Err(Error::NestedQuantifier);
            }
            if max.is_none()
                && let Self::Alternation(branches) = node.as_ref()
            {
                let mut seen = CharSet::default();
                for branch in branches {
                    let mut first = CharSet::default();
                    branch.first(&mut first);
                    if seen.overlaps(&first) {
                        return Err(Error::OverlappingAlternation);
                    }
                    seen.extend(first);
                }
            }
        }
        if let Self::Concat(items) = self {
            for (index, item) in items.iter().enumerate() {
                let Self::Repeat {
                    node, max: None, ..
                } = item
                else {
                    continue;
                };
                let mut left = CharSet::default();
                node.consumed(&mut left);
                for next in &items[index + 1..] {
                    if let Self::Repeat {
                        node, max: None, ..
                    } = next
                    {
                        let mut right = CharSet::default();
                        node.consumed(&mut right);
                        if left.overlaps(&right) {
                            return Err(Error::AdjacentOverlap);
                        }
                    }
                    if !next.nullable() {
                        break;
                    }
                }
            }
        }
        self.children().iter().try_for_each(Self::validate)
    }

    pub(super) fn translate(&self, output: &mut String) {
        match self {
            Self::Empty => {}
            Self::Char(characters) => characters.translate(output),
            Self::Anchor(Anchor::Start) => output.push_str("\\A"),
            Self::Anchor(Anchor::End) => output.push_str("\\z"),
            Self::Anchor(Anchor::Word) => output.push_str(WORD_BOUNDARY),
            Self::Anchor(Anchor::NonWord) => {
                output.push_str("(?!");
                output.push_str(WORD_BOUNDARY);
                output.push(')');
            }
            Self::Lookahead { positive, node } => {
                output.push_str(if *positive { "(?=" } else { "(?!" });
                node.translate(output);
                output.push(')');
            }
            Self::Concat(items) => items.iter().for_each(|node| node.translate(output)),
            Self::Alternation(branches) => {
                output.push_str("(?:");
                for (index, branch) in branches.iter().enumerate() {
                    if index > 0 {
                        output.push('|');
                    }
                    branch.translate(output);
                }
                output.push(')');
            }
            Self::Repeat {
                node,
                min,
                max,
                lazy,
            } => {
                output.push_str("(?:");
                node.translate(output);
                output.push(')');
                match max {
                    Some(max) if min == max => {
                        let _ = write!(output, "{{{min}}}");
                    }
                    Some(max) => {
                        let _ = write!(output, "{{{min},{max}}}");
                    }
                    None => {
                        let _ = write!(output, "{{{min},}}");
                    }
                }
                if *lazy {
                    output.push('?');
                }
            }
        }
    }
}
