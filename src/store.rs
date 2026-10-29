//! SQLite persistence for aggregated history, plus Zsh history import.

use crate::sources::{Entry, HistoryIndex};
use crate::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Store {
    conn: Connection,
}

pub struct Record<'a> {
    pub command: &'a str,
    pub dir: Option<&'a str>,
    pub ts: i64,
    pub success: bool,
    pub prev: Option<&'a str>,
    /// Position in the command stream (see `Entry::last_seq`).
    pub seq: u64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS commands (
                 cmd TEXT PRIMARY KEY, count INTEGER NOT NULL, failures INTEGER NOT NULL, last_used INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS dir_usage (
                 cmd TEXT NOT NULL, dir TEXT NOT NULL, count INTEGER NOT NULL, PRIMARY KEY (cmd, dir));
             CREATE TABLE IF NOT EXISTS transitions (
                 prev TEXT NOT NULL, next TEXT NOT NULL, count INTEGER NOT NULL, PRIMARY KEY (prev, next));
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        let has_seq = conn
            .prepare("SELECT 1 FROM pragma_table_info('commands') WHERE name = 'last_seq'")?
            .exists([])?;
        if !has_seq {
            conn.execute_batch(
                "ALTER TABLE commands ADD COLUMN last_seq INTEGER NOT NULL DEFAULT 0;
                 UPDATE commands SET last_seq = rowid;",
            )?;
        }
        Ok(Self { conn })
    }

    pub fn record(&mut self, r: &Record) -> Result<()> {
        self.record_many(std::slice::from_ref(r))
    }

    pub fn record_many(&mut self, records: &[Record]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut cmd = tx.prepare_cached(
                "INSERT INTO commands (cmd, count, failures, last_used, last_seq)
                 VALUES (?1, 1, ?2, ?3, ?4)
                 ON CONFLICT(cmd) DO UPDATE SET count = count + 1, failures = failures + ?2,
                     last_used = max(last_used, ?3), last_seq = max(last_seq, ?4)",
            )?;
            let mut dir = tx.prepare_cached(
                "INSERT INTO dir_usage (cmd, dir, count) VALUES (?1, ?2, 1)
                 ON CONFLICT(cmd, dir) DO UPDATE SET count = count + 1",
            )?;
            let mut trans = tx.prepare_cached(
                "INSERT INTO transitions (prev, next, count) VALUES (?1, ?2, 1)
                 ON CONFLICT(prev, next) DO UPDATE SET count = count + 1",
            )?;
            for r in records {
                cmd.execute(params![r.command, !r.success as i64, r.ts, r.seq as i64])?;
                if let Some(d) = r.dir {
                    dir.execute(params![r.command, d])?;
                }
                if let Some(p) = r.prev.filter(|p| *p != r.command) {
                    trans.execute(params![p, r.command])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load(&self) -> Result<HistoryIndex> {
        let mut index = HistoryIndex::default();
        let mut stmt = self
            .conn
            .prepare("SELECT cmd, count, failures, last_used, last_seq FROM commands")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Entry {
                    count: r.get(1)?,
                    failures: r.get(2)?,
                    last_used: r.get(3)?,
                    last_seq: r.get::<_, i64>(4)? as u64,
                    dirs: Default::default(),
                },
            ))
        })?;
        for row in rows {
            let (cmd, entry) = row?;
            index.load_entry(cmd, entry);
        }
        let mut stmt = self.conn.prepare("SELECT cmd, dir, count FROM dir_usage")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))? {
            let (cmd, dir, count) = row?;
            index.load_dir(&cmd, dir, count);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT prev, next, count FROM transitions")?;
        for row in stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
            let (prev, next, count) = row?;
            index.load_transition(prev, next, count);
        }
        Ok(index)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
            [key, value],
        )?;
        Ok(())
    }
}

/// Parse a Zsh history file (plain or `EXTENDED_HISTORY`) into
/// `(timestamp, command)` pairs, oldest first.
pub fn parse_zsh_history(bytes: &[u8]) -> Vec<(i64, String)> {
    // Zsh "metafies" bytes >= 0x83: they are stored as 0x83, byte ^ 0x20.
    let mut raw = Vec::with_capacity(bytes.len());
    let mut it = bytes.iter();
    while let Some(&b) = it.next() {
        if b == 0x83 {
            if let Some(&n) = it.next() {
                raw.push(n ^ 0x20);
            }
        } else {
            raw.push(b);
        }
    }
    let text = String::from_utf8_lossy(&raw);

    let mut out = Vec::new();
    let mut pending: Option<(i64, String)> = None;
    for line in text.lines() {
        let (ts, mut cmd) = match pending.take() {
            Some((ts, mut acc)) => {
                acc.push('\n');
                acc.push_str(line);
                (ts, acc)
            }
            None => parse_header(line),
        };
        if cmd.ends_with('\\') {
            cmd.pop();
            pending = Some((ts, cmd));
        } else if !cmd.trim().is_empty() {
            out.push((ts, cmd));
        }
    }
    out
}

fn parse_header(line: &str) -> (i64, String) {
    if let Some(rest) = line.strip_prefix(": ") {
        if let Some((meta, cmd)) = rest.split_once(';') {
            if let Some(ts) = meta.split(':').next().and_then(|t| t.trim().parse().ok()) {
                return (ts, cmd.to_string());
            }
        }
    }
    (0, line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extended_and_multiline_history() {
        let file = b": 1700000000:0;git status\n: 1700000005:2;for f in *; do\\\necho $f\\\ndone\n: 1700000009:0;ls\nplain\n";
        let h = parse_zsh_history(file);
        assert_eq!(
            h,
            [
                (1_700_000_000, "git status".to_string()),
                (1_700_000_005, "for f in *; do\necho $f\ndone".to_string()),
                (1_700_000_009, "ls".to_string()),
                (0, "plain".to_string()),
            ]
        );
    }

    #[test]
    fn unmetafies_non_ascii() {
        // "é" is C3 A9; zsh writes A9 as 0x83 0x89.
        let h = parse_zsh_history(&[b'e', b'c', b'h', b'o', b' ', 0xC3, 0x83, 0x89, b'\n']);
        assert_eq!(h[0].1, "echo é");
    }

    #[test]
    fn store_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("h.db")).unwrap();
        store
            .record_many(&[
                Record {
                    command: "make",
                    dir: Some("/p"),
                    ts: 10,
                    success: true,
                    prev: None,
                    seq: 1,
                },
                Record {
                    command: "make test",
                    dir: Some("/p"),
                    ts: 11,
                    success: false,
                    prev: Some("make"),
                    seq: 2,
                },
                Record {
                    command: "make",
                    dir: Some("/p"),
                    ts: 12,
                    success: true,
                    prev: Some("make test"),
                    seq: 3,
                },
            ])
            .unwrap();
        let index = store.load().unwrap();
        assert_eq!(index.len(), 2);
        assert_eq!(index.seq(), 3);
        store.set_meta("k", "v").unwrap();
        assert_eq!(store.meta("k").unwrap().as_deref(), Some("v"));
    }

    #[test]
    fn migrates_databases_without_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE commands (
                     cmd TEXT PRIMARY KEY, count INTEGER NOT NULL, failures INTEGER NOT NULL, last_used INTEGER NOT NULL);
                 INSERT INTO commands VALUES ('ls', 1, 0, 5), ('pwd', 1, 0, 5);",
            )
            .unwrap();
        let index = Store::open(&path).unwrap().load().unwrap();
        assert_eq!(index.len(), 2);
        assert_eq!(index.seq(), 2);
    }
}
