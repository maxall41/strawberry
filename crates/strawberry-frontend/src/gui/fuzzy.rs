use std::ops::Range;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

pub struct Match {
    /// Match score
    pub score: u32,
    /// Byte ranges of the matched characters in the label, ascending and non-overlapping.
    pub runs: Vec<Range<usize>>,
}

pub struct CommandMatcher {
    matcher: Matcher,
    pattern: Pattern,
    indices: Vec<u32>,
    haystack: Vec<char>,
}

impl CommandMatcher {
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            pattern: Pattern::parse("", CaseMatching::Ignore, Normalization::Smart),
            indices: Vec::new(),
            haystack: Vec::new(),
        }
    }

    /// Re-parse the query once per keystroke. Called externally.
    pub fn set_query(&mut self, query: &str) {
        self.pattern
            .reparse(query, CaseMatching::Smart, Normalization::Smart);
    }

    /// Match the Pattern against the given label. Returns Match with score.
    pub fn match_candidate(&mut self, label: &str) -> Option<Match> {
        self.indices.clear();
        let haystack = Utf32Str::new(label, &mut self.haystack);
        let score = self
            .pattern
            .indices(haystack, &mut self.matcher, &mut self.indices)?;

        self.indices.sort_unstable();
        self.indices.dedup();

        Some(Match {
            score,
            runs: byte_runs(label, &self.indices),
        })
    }
}

impl Default for CommandMatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// nucleo returns character indicies; convert them into a byte range, as gpui styled text expects byte ranges for character highlights, not character indicies.
fn byte_runs(text: &str, indices: &[u32]) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    let mut chars = text.char_indices();
    let mut position = 0usize;

    for &index in indices {
        let index = index as usize;
        let (offset, ch) = loop {
            let Some(next) = chars.next() else {
                return runs;
            };
            let current = position;
            position += 1;
            if current == index {
                break next;
            }
        };

        let end = offset + ch.len_utf8();
        match runs.last_mut() {
            Some(run) if run.end == offset => run.end = end,
            _ => runs.push(offset..end),
        }
    }

    runs
}
