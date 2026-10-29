use super::cache::Cache;
use super::{Context, Suggestion, SuggestionSource};
use crate::parser::{self, shell_escape};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(3);

#[derive(Clone, Default)]
struct Refs {
    local: Vec<String>,
    remote: Vec<String>,
    remotes: Vec<String>,
    tags: Vec<String>,
    current: String,
}

#[derive(Default)]
pub struct GitSource {
    cache: Mutex<Cache<(Instant, Arc<Refs>)>>,
}

impl GitSource {
    fn refs(&self, cwd: &Path) -> Arc<Refs> {
        if let Some((at, refs)) = self.cache.lock().unwrap().get(cwd) {
            if at.elapsed() < TTL {
                return refs.clone();
            }
        }
        let refs = Arc::new(load_refs(cwd));
        self.cache
            .lock()
            .unwrap()
            .insert(cwd.to_path_buf(), (Instant::now(), refs.clone()));
        refs
    }
}

fn git(cwd: &Path, args: &[&str]) -> Vec<String> {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn load_refs(cwd: &Path) -> Refs {
    let fmt = [
        "for-each-ref",
        "--sort=-committerdate",
        "--format=%(refname:short)",
    ];
    let local = git(cwd, &[&fmt[..], &["refs/heads"]].concat());
    let remote = git(cwd, &[&fmt[..], &["refs/remotes"]].concat())
        .into_iter()
        .filter_map(|r| r.split_once('/').map(|(_, b)| b.to_string()))
        .filter(|b| b != "HEAD" && !b.is_empty())
        .collect();
    Refs {
        local,
        remote,
        remotes: git(cwd, &["remote"]),
        tags: git(
            cwd,
            &[
                "for-each-ref",
                "--sort=-creatordate",
                "--format=%(refname:short)",
                "refs/tags",
            ],
        ),
        current: git(cwd, &["branch", "--show-current"])
            .pop()
            .unwrap_or_default(),
    }
}

impl SuggestionSource for GitSource {
    fn name(&self) -> &'static str {
        "git"
    }

    fn suggest(&self, ctx: &Context) -> Vec<Suggestion> {
        if ctx.line.command() != Some("git") || ctx.line.current().raw.starts_with('-') {
            return Vec::new();
        }
        let pos = ctx.line.positionals();
        if super::specs::arguments(&ctx.line).1.is_some() {
            return Vec::new();
        }
        let words: Vec<String> = match (pos.first().copied(), pos.len()) {
            (
                Some(
                    "checkout" | "switch" | "merge" | "rebase" | "branch" | "diff" | "log"
                    | "cherry-pick" | "reset",
                ),
                1,
            ) => {
                let refs = self.refs(&ctx.cwd);
                let mut all = refs.local.clone();
                for b in refs.remote.iter().chain(&refs.tags) {
                    if !all.contains(b) {
                        all.push(b.clone());
                    }
                }
                all
            }
            (Some("push" | "pull" | "fetch"), 1) => self.refs(&ctx.cwd).remotes.clone(),
            (Some("push" | "pull"), 2) => self.refs(&ctx.cwd).local.clone(),
            _ => return Vec::new(),
        };
        ctx.ranked_words(words.iter().map(String::as_str), "", 0.6, "git")
    }

    fn rejects(&self, ctx: &Context, text: &str) -> bool {
        if !text.contains("checkout") && !text.contains("switch") {
            return false;
        }
        let line = parser::parse(text.trim_end());
        let target = line.current().value.as_str();
        let pos = line.positionals();
        if line.command() != Some("git")
            || pos.len() != 1
            || !matches!(pos[0], "checkout" | "switch")
            || line.words.iter().any(|w| w.value.starts_with('-'))
            || target.is_empty()
            || target.contains(['~', '^', '@', ':', '$'])
            || (target.len() >= 7 && target.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return false;
        }
        let refs = self.refs(&ctx.cwd);
        if refs.local.is_empty() && refs.remote.is_empty() {
            return false;
        }
        let remote_ref = target.split_once('/').is_some_and(|(r, b)| {
            refs.remotes.iter().any(|x| x == r) && refs.remote.iter().any(|x| x == b)
        });
        let known = refs
            .local
            .iter()
            .chain(&refs.remote)
            .chain(&refs.tags)
            .any(|r| r == target);
        !(known || remote_ref || ctx.cwd.join(target).exists())
    }

    fn rewrite(&self, ctx: &Context, text: &str) -> Option<String> {
        if !text.contains("git") || !text.contains("push") {
            return None;
        }
        let text = text.trim_end();
        let line = parser::parse(text);
        let branch = line.current();
        let pos = line.positionals();
        if line.command() != Some("git")
            || pos.len() != 2
            || pos[0] != "push"
            || branch.value.is_empty()
            || branch.value.starts_with('-')
            || branch.value.contains(':')
        {
            return None;
        }
        let refs = self.refs(&ctx.cwd);
        if refs.current.is_empty()
            || refs.current == branch.value
            || !refs.remotes.iter().any(|r| r == pos[1])
        {
            return None;
        }
        Some(format!(
            "{}{}",
            &text[..branch.start],
            shell_escape(&refs.current)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_context;

    #[test]
    fn ref_cache_shares_data_and_refreshes_after_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let src = GitSource::default();
        let refs = Arc::new(Refs {
            current: "cached".into(),
            ..Refs::default()
        });
        src.cache
            .lock()
            .unwrap()
            .insert(dir.path().into(), (Instant::now(), refs.clone()));
        assert!(Arc::ptr_eq(&refs, &src.refs(dir.path())));
        src.cache
            .lock()
            .unwrap()
            .insert(dir.path().into(), (Instant::now() - TTL, refs.clone()));
        let refreshed = src.refs(dir.path());
        assert!(!Arc::ptr_eq(&refs, &refreshed));
        assert!(refreshed.current.is_empty());
    }

    #[test]
    fn adapts_history_to_this_repo() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success());
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q", "-b", "feature/login"]);
        run(&["remote", "add", "origin", "https://example.com/x.git"]);

        let src = GitSource::default();
        let ctx = test_context("git", dir.path());
        assert_eq!(
            src.rewrite(&ctx, "git push -u origin old-branch")
                .as_deref(),
            Some("git push -u origin feature/login")
        );
        assert_eq!(src.rewrite(&ctx, "git push origin feature/login"), None);
        assert_eq!(src.rewrite(&ctx, "git push upstream old-branch"), None);
        assert_eq!(src.rewrite(&ctx, "git push origin a:b"), None);
        assert_eq!(src.rewrite(&ctx, "git pull origin dev"), None);
        assert_eq!(
            src.rewrite(&ctx, "git push origin old ").as_deref(),
            Some("git push origin feature/login")
        );

        run(&["commit", "-q", "--allow-empty", "-m", "init"]);
        run(&["tag", "1.5.2"]);
        let src = GitSource::default();
        assert!(src.rejects(&ctx, "git checkout release-9.0.0"));
        assert!(src.rejects(&ctx, "git checkout release-9.0.0 "));
        assert!(!src.rejects(&ctx, "git checkout feature/login"));
        assert!(!src.rejects(&ctx, "git checkout 1.5.2"));
        assert!(!src.rejects(&ctx, "git checkout -b release-9.0.0"));
        assert!(!src.rejects(&ctx, "git checkout abc1234"));
        assert!(!src.rejects(&ctx, "git log release-9.0.0"));
        let outside = tempfile::tempdir().unwrap();
        let ctx = test_context("git", outside.path());
        assert!(!src.rejects(&ctx, "git checkout release-9.0.0"));
    }
}
