//! Runs every enabled source, merges candidates and ranks them.

use crate::config::Config;
use crate::parser;
use crate::sources::{
    Context, ExecutableSource, FileSource, GitSource, HistoryIndex, HistorySource, ProjectSource,
    SpecSource, SuggestionSource,
};
use crate::store::{parse_zsh_history, Record, Store};
use crate::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Ranked {
    pub text: String,
    pub score: f64,
    pub sources: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    /// Character offset, as used by ZLE (not a UTF-8 byte offset).
    pub cursor: usize,
}

pub struct Engine {
    config: Config,
    history: Arc<RwLock<HistoryIndex>>,
    sources: Vec<Box<dyn SuggestionSource>>,
    store: Option<Mutex<Store>>,
    /// Session id (shell pid) -> last command it ran.
    sessions: Mutex<HashMap<String, String>>,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Engine {
    /// Engine with every source, backed by the on-disk store.
    pub fn open(config: Config, store: Store) -> Result<Self> {
        let index = store.load()?;
        let mut engine = Self::with_index(config, index, true);
        engine.store = Some(Mutex::new(store));
        Ok(engine)
    }

    /// In-memory engine with only the history source (used by `bench`).
    pub fn history_only(config: Config) -> Self {
        Self::with_index(config, HistoryIndex::default(), false)
    }

    fn with_index(config: Config, index: HistoryIndex, context_sources: bool) -> Self {
        let history = Arc::new(RwLock::new(index));
        let mut history_source = HistorySource::new(history.clone(), config.history.clone());
        if !context_sources {
            history_source = history_source.without_dir_checks();
        }
        let mut sources: Vec<Box<dyn SuggestionSource>> = vec![Box::new(history_source)];
        if context_sources {
            sources.push(Box::new(GitSource::default()));
            sources.push(Box::new(ProjectSource::default()));
            sources.push(Box::new(SpecSource));
            sources.push(Box::new(FileSource));
            sources.push(Box::new(ExecutableSource::default()));
        }
        sources.retain(|s| config.enabled(s.name()));
        Self {
            config,
            history,
            sources,
            store: None,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn history_len(&self) -> usize {
        self.history.read().unwrap().len()
    }

    pub fn max_suggestions(&self) -> usize {
        self.config.max_suggestions
    }

    pub fn suggest(&self, buffer: &str, cwd: &Path, session: &str, limit: usize) -> Vec<Ranked> {
        self.suggest_at(buffer, cwd, session, limit, now())
    }

    /// `suggest` with an explicit clock, so `bench` can replay old history.
    pub fn suggest_at(
        &self,
        buffer: &str,
        cwd: &Path,
        session: &str,
        limit: usize,
        now: i64,
    ) -> Vec<Ranked> {
        self.query(buffer, cwd, session, limit, now, false)
    }

    /// Complete the word at a UTF-8 byte cursor, retaining everything after it.
    pub fn complete(
        &self,
        buffer: &str,
        cursor: usize,
        cwd: &Path,
        session: &str,
        limit: usize,
    ) -> Vec<Completion> {
        if !buffer.is_char_boundary(cursor) {
            return Vec::new();
        }
        let left = &buffer[..cursor];
        let start = parser::parse(left).current().start;
        let end = parser::word_end(buffer, start, cursor);
        let tail = &buffer[end..];
        self.query(
            left,
            cwd,
            session,
            if tail.is_empty() { limit } else { usize::MAX },
            now(),
            true,
        )
        .into_iter()
        .filter(|r| tail.is_empty() || !r.sources.contains(&"history"))
        .take(limit)
        .map(|r| {
            let mut text = r.text;
            if tail.starts_with(char::is_whitespace) && text.ends_with(' ') {
                text.pop();
            }
            let cursor = text.chars().count();
            text.push_str(tail);
            Completion { text, cursor }
        })
        .collect()
    }

    fn query(
        &self,
        buffer: &str,
        cwd: &Path,
        session: &str,
        limit: usize,
        now: i64,
        fuzzy: bool,
    ) -> Vec<Ranked> {
        // An empty prompt gets a next-command prediction, from history only.
        if !buffer.is_empty() && buffer.trim().is_empty() {
            return Vec::new();
        }
        let ctx = Context {
            buffer: buffer.to_string(),
            fuzzy,
            cwd: cwd.to_path_buf(),
            prev_command: self.sessions.lock().unwrap().get(session).cloned(),
            now,
            line: parser::parse(buffer),
        };

        let mut merged: HashMap<String, Ranked> = HashMap::new();
        for source in &self.sources {
            if buffer.is_empty() && !fuzzy && source.name() != "history" {
                continue;
            }
            let weight = self.config.weight(source.name());
            for mut s in source.suggest(&ctx) {
                if let Some(text) = self.rewrite(&ctx, &s.text) {
                    s.text = text;
                }
                if self.sources.iter().any(|src| src.rejects(&ctx, &s.text)) {
                    continue;
                }
                if s.text == buffer || (!fuzzy && !s.text.starts_with(buffer)) {
                    continue;
                }
                let p = (s.score * weight).clamp(0.0, 0.999);
                let r = merged.entry(s.text.clone()).or_insert_with(|| Ranked {
                    text: s.text,
                    score: 0.0,
                    sources: Vec::new(),
                });
                // Noisy-OR: agreement between sources raises confidence.
                r.score = 1.0 - (1.0 - r.score) * (1.0 - p);
                if !r.sources.contains(&s.source) {
                    r.sources.push(s.source);
                }
            }
        }

        let mut ranked: Vec<Ranked> = merged.into_values().collect();
        // A history line extending a context completion (`git log` ->
        // `git log --oneline`) is the more specific answer, so it inherits
        // the support of the best such prefix.
        let context: Vec<(String, f64)> = ranked
            .iter()
            .filter(|r| !r.sources.contains(&"history"))
            .map(|r| (r.text.clone(), r.score))
            .collect();
        for r in ranked.iter_mut().filter(|r| r.sources.contains(&"history")) {
            let support = context
                .iter()
                .filter(|(t, _)| r.text.len() > t.len() && r.text.starts_with(t.as_str()))
                .map(|(_, s)| *s)
                .fold(0.0, f64::max);
            r.score = 1.0 - (1.0 - r.score) * (1.0 - support);
        }
        ranked.sort_by(|a, b| {
            (!a.text.starts_with(buffer))
                .cmp(&!b.text.starts_with(buffer))
                .then_with(|| {
                    b.score
                        .total_cmp(&a.score)
                        .then_with(|| a.text.len().cmp(&b.text.len()))
                        .then_with(|| a.text.cmp(&b.text))
                })
        });
        ranked.truncate(limit);
        ranked
    }

    /// Let every source adapt a candidate; keeps it only if it still
    /// extends what was typed.
    fn rewrite(&self, ctx: &Context, text: &str) -> Option<String> {
        self.sources
            .iter()
            .find_map(|s| s.rewrite(ctx, text))
            .filter(|t| t.len() > ctx.buffer.len() && t.starts_with(&ctx.buffer))
    }

    /// Pretend `session` just ran `command`, without recording it.
    pub fn set_previous(&self, session: &str, command: &str) {
        self.sessions
            .lock()
            .unwrap()
            .insert(session.to_string(), command.to_string());
    }

    pub fn record(&self, session: &str, cwd: Option<&str>, command: &str, status: i32, ts: i64) {
        let command = command.trim_end_matches('\n');
        if command.trim().is_empty() || command.starts_with(' ') {
            return;
        }
        let prev = self
            .sessions
            .lock()
            .unwrap()
            .insert(session.to_string(), command.to_string());
        let success = status == 0;
        let seq = self
            .history
            .write()
            .unwrap()
            .record(command, cwd, ts, success, prev.as_deref());
        if let Some(store) = &self.store {
            let r = Record {
                command,
                dir: cwd,
                ts,
                success,
                prev: prev.as_deref(),
                seq,
            };
            if let Err(e) = store.lock().unwrap().record(&r) {
                eprintln!("zghost: failed to persist command: {e}");
            }
        }
    }

    /// One-time import of an existing Zsh history file. Returns how many
    /// commands were imported, or `None` if an import already happened.
    pub fn import_histfile(&self, path: &Path) -> Result<Option<usize>> {
        let Some(store) = &self.store else {
            return Ok(None);
        };
        let mut store = store.lock().unwrap();
        if store.meta("imported_histfile")?.is_some() {
            return Ok(None);
        }
        let entries = parse_zsh_history(&std::fs::read(path)?);
        let base = self.history.read().unwrap().seq();
        let mut records = Vec::with_capacity(entries.len());
        let mut prev: Option<&str> = None;
        for (ts, cmd) in &entries {
            if cmd.starts_with(' ') {
                continue;
            }
            records.push(Record {
                command: cmd,
                dir: None,
                ts: *ts,
                success: true,
                prev,
                seq: base + records.len() as u64 + 1,
            });
            prev = Some(cmd);
        }
        store.record_many(&records)?;
        store.set_meta("imported_histfile", &path.to_string_lossy())?;
        *self.history.write().unwrap() = store.load()?;
        Ok(Some(records.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_sources_and_tracks_sessions() {
        let engine = Engine::history_only(Config::default());
        let t = now();
        engine.record("1", Some("/r"), "cargo build --release", 0, t);
        engine.record("1", Some("/r"), " secret-token", 0, t);
        engine.record("1", Some("/r"), "cargo test", 0, t);

        let s = engine.suggest("cargo b", Path::new("/r"), "1", 5);
        assert_eq!(s[0].text, "cargo build --release");
        assert!(engine.suggest("secret", Path::new("/r"), "1", 5).is_empty());
        assert!(engine.suggest("  ", Path::new("/r"), "1", 5).is_empty());
    }

    #[test]
    fn imports_once() {
        let dir = tempfile::tempdir().unwrap();
        let hist = dir.path().join("hist");
        std::fs::write(&hist, ": 1:0;ls -la\n: 2:0;git log --oneline\n").unwrap();
        let engine = Engine::open(
            Config::default(),
            Store::open(&dir.path().join("db")).unwrap(),
        )
        .unwrap();
        assert_eq!(engine.import_histfile(&hist).unwrap(), Some(2));
        assert_eq!(engine.import_histfile(&hist).unwrap(), None);
        assert_eq!(
            engine.suggest("git l", dir.path(), "x", 1)[0].text,
            "git log --oneline"
        );
    }
}
