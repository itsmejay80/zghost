use super::{Context, Suggestion, SuggestionSource};
use crate::config::HistoryConfig;
use crate::parser;
use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

#[derive(Debug, Default, Clone)]
pub struct Entry {
    pub count: u32,
    pub failures: u32,
    pub last_used: i64,
    /// Position in the command stream when last used (0 = unknown). Imported
    /// history often shares one timestamp, so order is the better recency.
    pub last_seq: u64,
    /// Directory -> times used there.
    pub dirs: HashMap<String, u32>,
}

/// The part of a command that predicts what comes next: the command word
/// plus its subcommand. `git commit -m "fix"` -> `git commit`, `ls -la` -> `ls`.
pub fn shape(cmd: &str) -> String {
    let line = parser::parse(cmd);
    let Some(i) = line.command else {
        return String::new();
    };
    let mut out = line.words[i].value.clone();
    if let Some(sub) = line.words.get(i + 1).map(|w| w.value.as_str()) {
        let subcommand_like = sub.starts_with(|c: char| c.is_ascii_lowercase())
            && sub
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'));
        if subcommand_like {
            out.push(' ');
            out.push_str(sub);
        }
    }
    out
}

/// In-memory command history, ordered so prefix lookups are a range scan.
#[derive(Debug, Default)]
pub struct HistoryIndex {
    entries: BTreeMap<String, Entry>,
    /// prev command -> next command -> count.
    transitions: HashMap<String, HashMap<String, u32>>,
    /// Same, keyed by `shape`, so `git commit -m "anything"` teaches that
    /// `git push` usually follows any commit.
    shapes: HashMap<String, HashMap<String, u32>>,
    /// Command -> its shape, cached.
    shape_of: HashMap<String, String>,
    /// Commands recorded so far; the next one gets `seq + 1`.
    seq: u64,
}

/// What the previous command says about the next one, looked up once per query.
struct Sequence<'a> {
    exact: Option<&'a HashMap<String, u32>>,
    exact_total: u32,
    shapes: Option<&'a HashMap<String, u32>>,
    shape_total: u32,
}

impl HistoryIndex {
    pub fn record(
        &mut self,
        cmd: &str,
        dir: Option<&str>,
        ts: i64,
        success: bool,
        prev: Option<&str>,
    ) -> u64 {
        self.seq += 1;
        let seq = self.seq;
        self.shape_of(cmd);
        let e = self.entries.entry(cmd.to_string()).or_default();
        e.count += 1;
        e.last_used = e.last_used.max(ts);
        e.last_seq = seq;
        if !success {
            e.failures += 1;
        }
        if let Some(d) = dir {
            *e.dirs.entry(d.to_string()).or_default() += 1;
        }
        if let Some(p) = prev.filter(|p| *p != cmd) {
            self.load_transition(p.to_string(), cmd.to_string(), 1);
        }
        seq
    }

    /// Restore an aggregated row (used when loading from the store).
    pub fn load_entry(&mut self, cmd: String, entry: Entry) {
        self.seq = self.seq.max(entry.last_seq);
        self.shape_of(&cmd);
        self.entries.insert(cmd, entry);
    }

    fn shape_of(&mut self, cmd: &str) -> String {
        if let Some(s) = self.shape_of.get(cmd) {
            return s.clone();
        }
        let s = shape(cmd);
        self.shape_of.insert(cmd.to_string(), s.clone());
        s
    }

    pub fn load_dir(&mut self, cmd: &str, dir: String, count: u32) {
        if let Some(e) = self.entries.get_mut(cmd) {
            e.dirs.insert(dir, count);
        }
    }

    pub fn load_transition(&mut self, prev: String, next: String, count: u32) {
        let (ps, ns) = (self.shape_of(&prev), self.shape_of(&next));
        if !ps.is_empty() && !ns.is_empty() {
            *self.shapes.entry(ps).or_default().entry(ns).or_default() += count;
        }
        *self
            .transitions
            .entry(prev)
            .or_default()
            .entry(next)
            .or_default() += count;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn with_prefix<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = (&'a String, &'a Entry)> {
        self.entries
            .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
            .take_while(move |(k, _)| k.starts_with(prefix))
    }

    fn sequence(&self, prev: Option<&str>) -> Sequence<'_> {
        let exact = prev.and_then(|p| self.transitions.get(p));
        let shapes = prev.and_then(|p| self.shapes.get(&shape(p)));
        Sequence {
            exact,
            exact_total: exact.map(|m| m.values().sum()).unwrap_or(0),
            shapes,
            shape_total: shapes.map(|m| m.values().sum()).unwrap_or(0),
        }
    }

    /// Share of times the exact previous line was followed by `cmd`.
    fn exact_probability(&self, cmd: &str, seq: &Sequence) -> f64 {
        let n = seq.exact.and_then(|m| m.get(cmd)).copied().unwrap_or(0);
        n as f64 / (seq.exact_total as f64 + 1.0)
    }

    /// How strongly the previous command predicts `cmd`, in `0.0..1.0`:
    /// the share of times `cmd`'s shape followed the previous shape.
    fn shape_probability(&self, cmd: &str, seq: &Sequence) -> f64 {
        let n = seq
            .shapes
            .zip(self.shape_of.get(cmd))
            .and_then(|(m, s)| m.get(s))
            .copied()
            .unwrap_or(0);
        // +1 smoothing so a single observation isn't taken as certainty.
        n as f64 / (seq.shape_total as f64 + 1.0)
    }

    /// Unbounded relevance score; higher is better.
    fn raw_score(
        &self,
        cmd: &str,
        e: &Entry,
        ctx: &Context,
        seq: &Sequence,
        cfg: &HistoryConfig,
    ) -> f64 {
        let age_secs = (ctx.now - e.last_used).max(0) as f64;
        let recency = 0.5f64.powf(age_secs / 86_400.0 / cfg.half_life_days);
        let short = 0.5f64.powf(age_secs / 60.0 / cfg.short_half_life_minutes);
        let mut score = cfg.frequency_weight * (1.0 + e.count as f64).ln()
            + cfg.recency_weight * recency
            + cfg.short_recency_weight * short;
        if e.last_seq > 0 {
            let ago = self.seq.saturating_sub(e.last_seq) as f64;
            score += cfg.order_recency_weight * 0.5f64.powf(ago / cfg.order_half_life);
        }

        if let Some(n) = ctx.cwd.to_str().and_then(|d| e.dirs.get(d)) {
            score += cfg.directory_boost * (1.0 + (1.0 + *n as f64).ln());
        }
        if let Some(n) = seq.exact.and_then(|next| next.get(cmd)) {
            score += cfg.sequence_boost * (1.0 + *n as f64).ln();
        }
        score += cfg.shape_sequence_boost * self.shape_probability(cmd, seq);
        if e.failures * 2 > e.count {
            score *= cfg.failure_penalty;
        }
        score
    }
}

pub struct HistorySource {
    index: Arc<RwLock<HistoryIndex>>,
    config: HistoryConfig,
    /// Drop `cd` lines whose target is missing. Off for `bench`, where the
    /// working directory isn't known.
    check_dirs: bool,
}

impl HistorySource {
    pub fn new(index: Arc<RwLock<HistoryIndex>>, config: HistoryConfig) -> Self {
        Self {
            index,
            config,
            check_dirs: true,
        }
    }

    pub fn without_dir_checks(mut self) -> Self {
        self.check_dirs = false;
        self
    }
}

const MAX_CANDIDATES: usize = 50;

impl SuggestionSource for HistorySource {
    fn name(&self) -> &'static str {
        "history"
    }

    fn suggest(&self, ctx: &Context) -> Vec<Suggestion> {
        let index = self.index.read().unwrap();
        let seq = index.sequence(ctx.prev_command.as_deref());
        if ctx.buffer.is_empty() {
            return self.predict_next(&index, &seq, ctx);
        }
        if ctx.buffer.trim().is_empty() {
            return Vec::new();
        }
        let mut out: Vec<Suggestion> = Vec::new();
        let mut push = |prefix: &str, discount: f64| {
            for (cmd, e) in index.with_prefix(prefix) {
                if cmd.len() == prefix.len() {
                    continue;
                }
                let raw = index.raw_score(cmd, e, ctx, &seq, &self.config) * discount;
                out.push(Suggestion {
                    text: format!("{}{}", ctx.buffer, &cmd[prefix.len()..]),
                    score: raw / (raw + 2.0),
                    source: "history",
                });
            }
        };

        push(&ctx.buffer, 1.0);
        // `cd api && git pu` should still find `git push` on its own.
        let seg = ctx.line.segment_start();
        if seg > 0 && seg < ctx.buffer.len() {
            push(&ctx.buffer[seg..], 0.8);
        }

        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        out.into_iter()
            .filter(|s| !self.check_dirs || cd_target_exists(&s.text, &ctx.cwd))
            .take(MAX_CANDIDATES)
            .collect()
    }
}

impl HistorySource {
    /// Empty prompt: suggest the next command, Warp style, but only when the
    /// previous command makes it a confident guess.
    fn predict_next(&self, index: &HistoryIndex, seq: &Sequence, ctx: &Context) -> Vec<Suggestion> {
        let min_obs = self.config.next_min_observations;
        let min_p = self.config.next_min_probability;
        if seq.shape_total < min_obs {
            return Vec::new();
        }
        // Either the exact previous line or its shape must point here.
        let confident = |cmd: &str| {
            (seq.exact_total >= min_obs && index.exact_probability(cmd, seq) >= min_p)
                || index.shape_probability(cmd, seq) >= min_p
        };
        let mut out: Vec<Suggestion> = index
            .entries
            .iter()
            .filter(|(cmd, _)| confident(cmd))
            .map(|(cmd, e)| {
                let raw = index.raw_score(cmd, e, ctx, seq, &self.config);
                Suggestion {
                    text: cmd.clone(),
                    score: raw / (raw + 2.0),
                    source: "history",
                }
            })
            .collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        out.into_iter()
            .filter(|s| !self.check_dirs || cd_target_exists(&s.text, &ctx.cwd))
            .take(MAX_CANDIDATES)
            .collect()
    }
}

/// A `cd` line from history is only useful if its target exists from the
/// current directory (`cd src/api` typed in `~` is noise inside a project).
fn cd_target_exists(text: &str, cwd: &Path) -> bool {
    let line = parser::parse(text);
    if !matches!(line.command(), Some("cd" | "pushd")) {
        return true;
    }
    // `cd a b` is almost always a typo like `cd git checkout dev`.
    if !line.positionals().is_empty() {
        return false;
    }
    let target = line.current().value.as_str();
    if target.is_empty() || target == "-" || target.contains('$') {
        return true;
    }
    let path = match target.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            home.join(rest.trim_start_matches('/'))
        }
        Some(_) => return true, // ~user or named directory: can't resolve here.
        None => cwd.join(target),
    };
    path.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_context;

    fn source(index: HistoryIndex) -> HistorySource {
        HistorySource::new(Arc::new(RwLock::new(index)), HistoryConfig::default())
    }

    fn top(src: &HistorySource, buffer: &str, cwd: &str, prev: Option<&str>) -> Option<String> {
        let mut ctx = test_context(buffer, Path::new(cwd));
        ctx.prev_command = prev.map(String::from);
        src.suggest(&ctx).into_iter().next().map(|s| s.text)
    }

    #[test]
    fn prefers_frequent_and_recent() {
        let mut idx = HistoryIndex::default();
        let now = 1_700_000_000;
        idx.record("git status", None, now - 100 * 86_400, true, None);
        for _ in 0..5 {
            idx.record("git stash", None, now - 60, true, None);
        }
        assert_eq!(
            top(&source(idx), "git st", "/", None).as_deref(),
            Some("git stash")
        );
    }

    #[test]
    fn directory_and_sequence_boosts() {
        let now = 1_700_000_000;
        let mut idx = HistoryIndex::default();
        idx.record("npm run dev", Some("/web"), now, true, None);
        idx.record("npm run build", Some("/api"), now, true, None);
        idx.record("npm run build", Some("/api"), now, true, None);
        let src = source(idx);
        assert_eq!(
            top(&src, "npm r", "/web", None).as_deref(),
            Some("npm run dev")
        );
        assert_eq!(
            top(&src, "npm r", "/api", None).as_deref(),
            Some("npm run build")
        );

        let mut idx = HistoryIndex::default();
        idx.record("git push", None, now, true, None);
        idx.record("git pull", None, now, true, None);
        idx.record("git commit -m wip", None, now, true, None);
        idx.record("git push", None, now, true, Some("git commit -m wip"));
        idx.record("git pull", None, now, true, None);
        let src = source(idx);
        assert_eq!(
            top(&src, "git pu", "/", Some("git commit -m wip")).as_deref(),
            Some("git push")
        );
    }

    #[test]
    fn penalises_failing_commands() {
        let now = 1_700_000_000;
        let mut idx = HistoryIndex::default();
        for _ in 0..3 {
            idx.record("cargo tset", None, now, false, None);
        }
        idx.record("cargo test", None, now, true, None);
        assert_eq!(
            top(&source(idx), "cargo t", "/", None).as_deref(),
            Some("cargo test")
        );
    }

    #[test]
    fn drops_cd_into_missing_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("api")).unwrap();
        let mut idx = HistoryIndex::default();
        let now = 1_700_000_000;
        for _ in 0..5 {
            idx.record("cd Desktop/Company/project", None, now, true, None);
        }
        idx.record("cd api", None, now - 86_400, true, None);
        idx.record("cd ~", None, now - 30 * 86_400, true, None);
        idx.record("cd git checkout dev", None, now, true, None);
        let src = source(idx);
        let cwd = dir.path().to_str().unwrap();
        assert_eq!(top(&src, "cd ", cwd, None).as_deref(), Some("cd api"));
        assert_eq!(top(&src, "cd D", cwd, None), None);
        assert_eq!(top(&src, "cd g", cwd, None), None);
        assert_eq!(top(&src, "cd ~", cwd, None), None); // nothing longer than the buffer
    }

    #[test]
    fn shapes() {
        assert_eq!(shape("git commit -m \"fix login\""), "git commit");
        assert_eq!(shape("sudo git push origin main"), "git push");
        assert_eq!(shape("ls -la"), "ls");
        assert_eq!(shape("python manage.py runserver"), "python");
        assert_eq!(shape("cd api && npm run dev"), "npm run");
    }

    #[test]
    fn learns_what_follows_a_kind_of_command() {
        let now = 1_700_000_000;
        let mut idx = HistoryIndex::default();
        // Every commit message is different, but a push always follows.
        for i in 0..5 {
            let commit = format!("git commit -m 'change {i}'");
            idx.record("git add .", None, now, true, None);
            idx.record(&commit, None, now, true, Some("git add ."));
            idx.record("git push", None, now, true, Some(&commit));
        }
        for _ in 0..10 {
            idx.record("git status", None, now, true, None);
        }
        let src = source(idx);
        let after = Some("git commit -m 'something new'");
        assert_eq!(top(&src, "git", "/", after).as_deref(), Some("git push"));
        assert_eq!(top(&src, "", "/", after).as_deref(), Some("git push"));
        assert_eq!(
            top(&src, "git", "/", Some("git add src/")).as_deref(),
            Some("git commit -m 'change 4'")
        );
        // No confident guess for an empty prompt without a known pattern.
        assert_eq!(top(&src, "", "/", Some("make")), None);
        assert_eq!(top(&src, "", "/", None), None);
    }

    #[test]
    fn predicts_from_the_exact_previous_line() {
        // After `git checkout dev` you pull, though checkouts in general are
        // followed by many different things.
        let now = 1_700_000_000;
        let mut idx = HistoryIndex::default();
        for i in 0..4 {
            idx.record("git pull", None, now, true, Some("git checkout dev"));
            for next in ["ls", "make", "cursor ."] {
                idx.record(next, None, now, true, Some(&format!("git checkout b{i}")));
            }
        }
        let src = source(idx);
        assert_eq!(
            top(&src, "", "/", Some("git checkout dev")).as_deref(),
            Some("git pull")
        );
        assert_eq!(top(&src, "", "/", Some("git checkout b9")), None);
    }

    #[test]
    fn prefers_what_was_run_a_few_commands_ago() {
        // Imported history often shares one timestamp; order still counts.
        let now = 1_700_000_000;
        let mut idx = HistoryIndex::default();
        for _ in 0..3 {
            idx.record("docker compose up", None, now, true, None);
        }
        for i in 0..100 {
            idx.record(&format!("echo {i}"), None, now, true, None);
        }
        idx.record("docker compose down", None, now, true, None);
        idx.record("ls", None, now, true, None);
        assert_eq!(
            top(&source(idx), "docker c", "/", None).as_deref(),
            Some("docker compose down")
        );
    }

    #[test]
    fn completes_last_segment() {
        let mut idx = HistoryIndex::default();
        idx.record("git push origin main", None, 1_700_000_000, true, None);
        assert_eq!(
            top(&source(idx), "cd api && git pu", "/", None).as_deref(),
            Some("cd api && git push origin main")
        );
    }
}
