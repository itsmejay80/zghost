//! A small, allocation-light shell lexer.
//!
//! It only needs to answer "which simple command is being typed, and which
//! word is under the cursor", so it handles quoting, escapes and command
//! separators but makes no attempt at full Zsh grammar.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// Text exactly as typed, including quotes and backslashes.
    pub raw: String,
    /// Text after quote removal.
    pub value: String,
    /// Byte offset of the word in the buffer.
    pub start: usize,
}

#[derive(Debug, Clone)]
pub struct ParsedLine {
    /// Words of the last simple command. The final word is the one being
    /// typed; it is empty when the buffer ends in whitespace.
    pub words: Vec<Word>,
    /// Index of the command word, after skipping `sudo`, `FOO=bar`, etc.
    pub command: Option<usize>,
}

const PREFIX_COMMANDS: &[&str] = &[
    "sudo",
    "doas",
    "time",
    "command",
    "builtin",
    "exec",
    "nohup",
    "env",
    "noglob",
    "nocorrect",
];

pub fn parse(buffer: &str) -> ParsedLine {
    let mut words: Vec<Word> = Vec::new();
    let mut cur: Option<Word> = None;
    let mut quote: Option<char> = None;
    let mut chars = buffer.char_indices();

    while let Some((i, c)) = chars.next() {
        if let Some(q) = quote {
            let w = open(&mut cur, i);
            w.raw.push(c);
            if c == q {
                quote = None;
            } else if q == '"' && c == '\\' {
                if let Some((_, n)) = chars.next() {
                    w.raw.push(n);
                    w.value.push(n);
                }
            } else {
                w.value.push(c);
            }
            continue;
        }
        match c {
            ' ' | '\t' => {
                if let Some(w) = cur.take() {
                    words.push(w);
                }
            }
            '\n' | '|' | '&' | ';' | '(' | ')' | '{' | '}' => {
                cur = None;
                words.clear();
            }
            '\\' => {
                let w = open(&mut cur, i);
                w.raw.push(c);
                if let Some((_, n)) = chars.next() {
                    w.raw.push(n);
                    w.value.push(n);
                }
            }
            '\'' | '"' => {
                open(&mut cur, i).raw.push(c);
                quote = Some(c);
            }
            _ => {
                let w = open(&mut cur, i);
                w.raw.push(c);
                w.value.push(c);
            }
        }
    }
    words.push(cur.unwrap_or(Word {
        raw: String::new(),
        value: String::new(),
        start: buffer.len(),
    }));

    let mut command = None;
    let mut after_prefix = false;
    for (i, w) in words.iter().enumerate() {
        let v = w.value.as_str();
        let is_last = i == words.len() - 1;
        if !is_last
            && (is_assignment(v)
                || PREFIX_COMMANDS.contains(&v)
                || (after_prefix && v.starts_with('-')))
        {
            after_prefix = after_prefix || PREFIX_COMMANDS.contains(&v);
            continue;
        }
        command = Some(i);
        break;
    }
    ParsedLine { words, command }
}

fn open(cur: &mut Option<Word>, start: usize) -> &mut Word {
    cur.get_or_insert_with(|| Word {
        raw: String::new(),
        value: String::new(),
        start,
    })
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

/// End of the cursor's shell word, accounting for quoting and escapes.
pub fn word_end(buffer: &str, start: usize, cursor: usize) -> usize {
    let mut quote = None;
    let mut escaped = false;
    for (offset, c) in buffer[start..].char_indices() {
        let i = start + offset;
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
        } else if i >= cursor && (c.is_whitespace() || "|&;(){}<>".contains(c)) {
            return i;
        }
    }
    buffer.len()
}

impl ParsedLine {
    pub fn current(&self) -> &Word {
        self.words.last().expect("parse always yields a word")
    }

    fn current_index(&self) -> usize {
        self.words.len() - 1
    }

    /// The command word, once the user has moved past it.
    pub fn command(&self) -> Option<&str> {
        self.command
            .filter(|&i| i < self.current_index())
            .map(|i| self.words[i].value.as_str())
    }

    pub fn typing_command(&self) -> bool {
        self.command == Some(self.current_index())
    }

    /// Non-flag arguments between the command and the current word.
    pub fn positionals(&self) -> Vec<&str> {
        match self.command {
            Some(i) if i < self.current_index() => crate::sources::specs::arguments(self).0,
            _ => Vec::new(),
        }
    }

    /// Byte offset where the last simple command begins.
    pub fn segment_start(&self) -> usize {
        self.words[0].start
    }
}

/// Backslash-escape a value so it can be inserted as a single shell word.
pub fn shell_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if " \t\n'\"\\$`&|;()<>*?[]{}!#".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(p: &ParsedLine) -> Vec<&str> {
        p.words.iter().map(|w| w.value.as_str()).collect()
    }

    #[test]
    fn splits_words_and_tracks_current() {
        let p = parse("git checkout fe");
        assert_eq!(values(&p), ["git", "checkout", "fe"]);
        assert_eq!(p.command(), Some("git"));
        assert_eq!(p.positionals(), ["checkout"]);
        assert_eq!(p.current().start, 13);
    }

    #[test]
    fn trailing_space_starts_empty_word() {
        let p = parse("git ");
        assert_eq!(values(&p), ["git", ""]);
        assert_eq!(p.current().start, 4);
    }

    #[test]
    fn last_segment_only() {
        let p = parse("cd src && npm run b");
        assert_eq!(values(&p), ["npm", "run", "b"]);
        assert_eq!(p.segment_start(), 10);
        let p = parse("cat x | gr");
        assert!(p.typing_command());
    }

    #[test]
    fn skips_prefixes_and_assignments() {
        let p = parse("RUST_LOG=debug sudo -E cargo b");
        assert_eq!(p.command(), Some("cargo"));
        let p = parse("sudo ");
        assert!(p.typing_command());
    }

    #[test]
    fn quotes_and_escapes() {
        let p = parse(r#"echo "a b" c\ d 'e"#);
        assert_eq!(values(&p), ["echo", "a b", "c d", "e"]);
        assert_eq!(p.current().raw, "'e");
    }

    #[test]
    fn escape_round_trips_through_parse() {
        let v = "my file (1).txt";
        let p = parse(&format!("ls {}", shell_escape(v)));
        assert_eq!(p.current().value, v);
    }
}
