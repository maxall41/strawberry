use std::ops::Range;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

pub struct Match {
    /// Byte ranges of the matched characters in the label, ascending and non-overlapping.
    pub runs: Vec<Range<usize>>,
}

pub struct FuzzyMatcher {
    matcher: Matcher,
    pattern: Pattern,
    indices: Vec<u32>,
    haystack: Vec<char>,
}

impl FuzzyMatcher {
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

    /// Match the pattern against a label and return its highlight ranges.
    pub fn match_candidate(&mut self, label: &str) -> Option<Match> {
        self.indices.clear();
        let haystack = Utf32Str::new(label, &mut self.haystack);
        self.pattern
            .indices(haystack, &mut self.matcher, &mut self.indices)?;

        self.indices.sort_unstable();
        self.indices.dedup();

        Some(Match {
            runs: byte_runs(label, &self.indices),
        })
    }
}

impl Default for FuzzyMatcher {
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
        let Ok(index) = usize::try_from(index) else {
            return runs;
        };
        let (offset, ch) = loop {
            let Some(next) = chars.next() else {
                return runs;
            };
            let current = position;
            position = position.saturating_add(1);
            if current == index {
                break next;
            }
        };

        let end = offset.saturating_add(ch.len_utf8());
        match runs.last_mut() {
            Some(run) if run.end == offset => run.end = end,
            _ => runs.push(offset..end),
        }
    }

    runs
}
