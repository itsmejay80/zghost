use super::{Context, Suggestion, SuggestionSource};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PATH_TTL: Duration = Duration::from_secs(60);
const MAX_DIR_ENTRIES: usize = 2000;

/// Executables on `$PATH`, offered while typing the command word.
#[derive(Default)]
pub struct ExecutableSource {
    cache: Mutex<Option<(Instant, Arc<Vec<String>>)>>,
}

impl ExecutableSource {
    fn executables(&self) -> Arc<Vec<String>> {
        let mut cache = self.cache.lock().unwrap();
        if let Some((at, names)) = cache.as_ref() {
            if at.elapsed() < PATH_TTL {
                return names.clone();
            }
        }
        let mut names: Vec<String> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flatten()
            .flatten()
            .filter(|e| {
                e.metadata()
                    .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            })
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        // Shorter names first: `git` before `git-lfs`.
        names.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        names.dedup();
        let names = Arc::new(names);
        *cache = Some((Instant::now(), names.clone()));
        names
    }
}

impl SuggestionSource for ExecutableSource {
    fn name(&self) -> &'static str {
        "executables"
    }

    fn suggest(&self, ctx: &Context) -> Vec<Suggestion> {
        let raw = &ctx.line.current().raw;
        if !ctx.line.typing_command() || (raw.is_empty() && !ctx.fuzzy) || raw.contains('/') {
            return Vec::new();
        }
        let names = self.executables();
        ctx.ranked_words(names.iter().map(String::as_str), " ", 0.3, "executables")
    }
}

/// File and directory names for path-like words and file-taking commands.
pub struct FileSource;

const FILE_COMMANDS: &[&str] = &[
    "cat", "less", "bat", "head", "tail", "vim", "nvim", "vi", "nano", "code", "open", "rm", "cp",
    "mv", "ls", "source", "touch", "chmod", "du", "tar", "unzip", "python", "python3", "node",
    "bash", "sh", "zsh",
];
const DIR_COMMANDS: &[&str] = &["cd", "pushd", "mkdir", "rmdir"];

impl SuggestionSource for FileSource {
    fn name(&self) -> &'static str {
        "files"
    }

    fn suggest(&self, ctx: &Context) -> Vec<Suggestion> {
        let word = ctx.line.current();
        let path_value = super::specs::path_value(&ctx.line);
        if path_value.is_none() && super::specs::arguments(&ctx.line).1.is_some() {
            return Vec::new();
        }
        let ended = ctx
            .line
            .words
            .iter()
            .take(ctx.line.words.len() - 1)
            .any(|w| w.value == "--");
        if (word.raw.starts_with('-') && path_value.is_none() && !ended) || word.raw.contains('$') {
            return Vec::new();
        }
        let (option_prefix, value) = if path_value.is_some() {
            word.value
                .split_once('=')
                .map(|(p, v)| (format!("{p}="), v))
                .unwrap_or((String::new(), word.value.as_str()))
        } else {
            (String::new(), word.value.as_str())
        };
        let cmd = ctx.line.command();
        let dirs_only = path_value == Some(super::specs::Value::Directory)
            || cmd.is_some_and(|c| DIR_COMMANDS.contains(&c));
        let path_like = value.contains('/') || value.starts_with(['.', '~']);
        if !(path_value.is_some()
            || path_like
            || dirs_only
            || cmd.is_some_and(|c| FILE_COMMANDS.contains(&c)))
        {
            return Vec::new();
        }

        let (dir_part, name_part) = match value.rfind('/') {
            Some(i) => value.split_at(i + 1),
            None => ("", value),
        };
        let listing_dir = match dir_part {
            "" => ctx.cwd.clone(),
            d if d.starts_with("~/") => home().join(&d[2..]),
            d => ctx.cwd.join(d),
        };
        let Ok(entries) = std::fs::read_dir(&listing_dir) else {
            return Vec::new();
        };

        let mut found: Vec<(String, bool)> = entries
            .take(MAX_DIR_ENTRIES)
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let hidden_ok = !name.starts_with('.') || name_part.starts_with('.');
                if !hidden_ok
                    || !(name.starts_with(name_part)
                        || (ctx.fuzzy && super::matches_word(name_part, &name)))
                {
                    return None;
                }
                // Directory entries usually carry their type already. Follow
                // symlinks as before, and retain the metadata fallback.
                let is_dir = e
                    .file_type()
                    .ok()
                    .filter(|t| !t.is_symlink())
                    .map(|t| t.is_dir())
                    .unwrap_or_else(|| e.path().is_dir());
                (is_dir || !dirs_only).then_some((name, is_dir))
            })
            .collect();
        found.sort_by(|(a, _), (b, _)| {
            (!a.starts_with(name_part))
                .cmp(&!b.starts_with(name_part))
                .then_with(|| a.cmp(b))
        });

        found
            .iter()
            .filter_map(|(name, is_dir)| {
                ctx.complete_word(
                    &format!("{option_prefix}{dir_part}{name}"),
                    if *is_dir { "/" } else { " " },
                )
            })
            .take(100)
            .enumerate()
            .map(|(rank, text)| Suggestion {
                text,
                score: (0.45 - 0.01 * rank as f64).max(0.2),
                source: "files",
            })
            .collect()
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_context;

    #[test]
    fn executable_cache_shares_names_until_expiry() {
        let src = ExecutableSource::default();
        // Seed directly to keep the test independent of the user's PATH.
        let names = Arc::new(vec!["tool".to_string()]);
        *src.cache.lock().unwrap() = Some((Instant::now(), names.clone()));
        assert!(Arc::ptr_eq(&names, &src.executables()));
        assert!(Arc::ptr_eq(&src.executables(), &src.executables()));
        src.cache.lock().unwrap().as_mut().unwrap().0 = Instant::now() - PATH_TTL;
        assert!(!Arc::ptr_eq(&names, &src.executables()));
    }

    #[test]
    fn file_types_follow_symlinks_and_include_new_files_immediately() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real-dir")).unwrap();
        std::fs::write(dir.path().join("real-file"), "").unwrap();
        symlink("real-dir", dir.path().join("link-dir")).unwrap();
        symlink("real-file", dir.path().join("link-file")).unwrap();
        symlink("missing", dir.path().join("link-broken")).unwrap();
        let texts = |buffer| {
            FileSource
                .suggest(&test_context(buffer, dir.path()))
                .into_iter()
                .map(|s| s.text)
                .collect::<Vec<_>>()
        };
        assert_eq!(texts("cd link"), ["cd link-dir/"]);
        assert_eq!(
            texts("cat link"),
            ["cat link-broken ", "cat link-dir/", "cat link-file "]
        );
        assert!(texts("cat new").is_empty());
        std::fs::write(dir.path().join("new-file"), "").unwrap();
        assert_eq!(texts("cat new"), ["cat new-file "]);
    }

    #[test]
    fn completes_files_and_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("my notes.md"), "").unwrap();
        std::fs::write(dir.path().join("setup.sh"), "").unwrap();

        let texts = |buf: &str| -> Vec<String> {
            FileSource
                .suggest(&test_context(buf, dir.path()))
                .into_iter()
                .map(|s| s.text)
                .collect()
        };
        assert_eq!(texts("cd s"), ["cd src/"]);
        assert_eq!(texts("cat s"), ["cat setup.sh ", "cat src/"]);
        assert_eq!(texts("vim my"), ["vim my\\ notes.md "]);
        assert_eq!(texts("vim my\\ n"), ["vim my\\ notes.md "]);
        assert!(texts("echo s").is_empty());
        assert_eq!(texts("echo ./s"), ["echo ./setup.sh ", "echo ./src/"]);
    }
}
