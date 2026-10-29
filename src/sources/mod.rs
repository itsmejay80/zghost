//! Suggestion sources. Each one looks at the line being typed and proposes
//! complete buffers; the engine merges and ranks them.

mod cache;
mod git;
mod history;
mod path;
mod project;
pub(crate) mod specs;

pub use git::GitSource;
pub use history::{Entry, HistoryIndex, HistorySource};
pub use path::{ExecutableSource, FileSource};
pub use project::ProjectSource;
pub use specs::SpecSource;

use crate::parser::{shell_escape, ParsedLine};
use std::path::PathBuf;

pub struct Context {
    pub buffer: String,
    /// Explicit completions may replace the current word; ghost text may only append.
    pub fuzzy: bool,
    pub cwd: PathBuf,
    /// The command this shell session ran last, if known.
    pub prev_command: Option<String>,
    /// Unix time in seconds.
    pub now: i64,
    pub line: ParsedLine,
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    /// The full buffer after accepting. Inline candidates always extend the buffer.
    pub text: String,
    /// Confidence in `0.0..1.0`.
    pub score: f64,
    pub source: &'static str,
}

pub trait SuggestionSource: Send + Sync {
    fn name(&self) -> &'static str;
    fn suggest(&self, ctx: &Context) -> Vec<Suggestion>;

    /// Adapt another source's full-line candidate to the current context
    /// (e.g. swap a remembered branch for the current one).
    fn rewrite(&self, _ctx: &Context, _text: &str) -> Option<String> {
        None
    }

    /// Veto another source's full-line candidate that can't work here
    /// (e.g. checking out a branch this repo doesn't have).
    fn rejects(&self, _ctx: &Context, _text: &str) -> bool {
        false
    }
}

impl Context {
    /// Complete the word under the cursor to `value` (escaped) plus `suffix`.
    /// Menu completion can replace a fuzzy-matched word; inline completion only appends.
    pub fn complete_word(&self, value: &str, suffix: &str) -> Option<String> {
        let raw = &self.line.current().raw;
        let mut word = if raw.starts_with('\'') {
            format!("'{}'", value.replace('\'', "'\\''"))
        } else if raw.starts_with('"') {
            format!(
                "\"{}\"",
                value
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('$', "\\$")
                    .replace('`', "\\`")
            )
        } else {
            shell_escape(value)
        };
        word.push_str(suffix);
        if word.len() > raw.len() && word.starts_with(raw.as_str()) {
            Some(format!("{}{}", self.buffer, &word[raw.len()..]))
        } else if self.fuzzy && matches_word(&self.line.current().value, value) {
            Some(format!(
                "{}{}",
                &self.buffer[..self.line.current().start],
                word
            ))
        } else {
            None
        }
    }

    /// Helper for sources that rank a list of word completions in order.
    pub fn ranked_words<'a>(
        &self,
        values: impl IntoIterator<Item = &'a str>,
        suffix: &str,
        top: f64,
        source: &'static str,
    ) -> Vec<Suggestion> {
        let mut candidates: Vec<_> = values
            .into_iter()
            .filter_map(|v| self.complete_word(v, suffix))
            .collect();
        candidates.sort_by_key(|s| !s.starts_with(&self.buffer));
        candidates
            .into_iter()
            .take(100)
            .enumerate()
            .map(|(rank, text)| Suggestion {
                text,
                score: (top - 0.01 * rank as f64).max(top / 2.0),
                source,
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) fn test_context(buffer: &str, cwd: &std::path::Path) -> Context {
    Context {
        buffer: buffer.to_string(),
        fuzzy: false,
        cwd: cwd.to_path_buf(),
        prev_command: None,
        now: 1_700_000_000,
        line: crate::parser::parse(buffer),
    }
}

/// Case-insensitive subsequence matching, plus a single typo for words of 3+ chars.
pub(crate) fn matches_word(query: &str, value: &str) -> bool {
    if query.chars().count() > 256 || value.chars().count() > 1024 {
        return value.starts_with(query);
    }
    let q: Vec<_> = query.to_lowercase().chars().collect();
    let v: Vec<_> = value.to_lowercase().chars().collect();
    let mut i = 0;
    for c in &v {
        if q.get(i) == Some(c) {
            i += 1;
        }
    }
    if i == q.len() {
        return true;
    }
    if q.len() < 3 {
        return false;
    }
    // Compare against prefixes too: `chek` can discover `checkout`.
    let mut row: Vec<usize> = (0..=v.len()).collect();
    let mut next = vec![0; v.len() + 1];
    for (i, a) in q.iter().enumerate() {
        next[0] = i + 1;
        for (j, b) in v.iter().enumerate() {
            next[j + 1] = (row[j + 1] + 1)
                .min(next[j] + 1)
                .min(row[j] + usize::from(a != b));
        }
        std::mem::swap(&mut row, &mut next);
    }
    row.iter()
        .skip(q.len().saturating_sub(1))
        .take(3)
        .any(|d| *d <= 1)
}

#[cfg(test)]
mod matching_tests {
    use super::matches_word;

    #[test]
    fn fuzzy_matching_retains_subsequences_typos_and_unicode() {
        for (query, value, expected) in [
            ("", "checkout", true),
            ("gco", "git-checkout", true),
            ("GCO", "git-checkout", true),
            ("chek", "checkout", true),
            ("chexk", "checkout", true),
            ("cxexk", "checkout", false),
            ("abx", "abc", true),
            ("abxx", "abc", false),
            ("ax", "abc", false),
            ("CAFÉ", "café", true),
            ("İ", "i\u{307}", true),
            ("abc", "", false),
        ] {
            assert_eq!(
                matches_word(query, value),
                expected,
                "{query:?} vs {value:?}"
            );
        }
    }
}
