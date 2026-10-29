use super::cache::Cache;
use super::{Context, Suggestion, SuggestionSource};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Names defined by project files: package.json scripts, Makefile targets
/// and docker compose services. Parsed results are cached by file mtime.
#[derive(Default)]
pub struct ProjectSource {
    cache: Mutex<Cache<(SystemTime, Arc<Vec<String>>)>>,
}

#[derive(Clone, Copy)]
enum Kind {
    PackageScripts,
    MakeTargets,
    ComposeServices,
}

impl ProjectSource {
    fn names(&self, file: &Path, kind: Kind) -> Arc<Vec<String>> {
        let Ok(mtime) = file.metadata().and_then(|m| m.modified()) else {
            return Arc::default();
        };
        if let Some((m, names)) = self.cache.lock().unwrap().get(file) {
            if *m == mtime {
                return names.clone();
            }
        }
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let names = match kind {
            Kind::PackageScripts => package_scripts(&text),
            Kind::MakeTargets => make_targets(&text),
            Kind::ComposeServices => compose_services(&text),
        };
        let names = Arc::new(names);
        self.cache
            .lock()
            .unwrap()
            .insert(file.to_path_buf(), (mtime, names.clone()));
        names
    }
}

/// Walk up from `cwd` looking for the first of `names`.
fn find_up(cwd: &Path, names: &[&str]) -> Option<PathBuf> {
    cwd.ancestors()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

fn package_scripts(text: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| {
            v.get("scripts")?
                .as_object()
                .map(|m| m.keys().cloned().collect())
        })
        .unwrap_or_default()
}

fn make_targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some((head, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') || line.starts_with(['\t', ' ', '.', '#']) {
            continue;
        }
        for t in head.split_whitespace() {
            if t.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c))
                && !out.iter().any(|o| o == t)
            {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// Minimal YAML reading: keys one indent level below a top-level `services:`.
fn compose_services(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_services = false;
    let mut indent: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let depth = line.len() - trimmed.len();
        if depth == 0 {
            in_services = trimmed.starts_with("services:");
            continue;
        }
        if !in_services {
            continue;
        }
        let level = *indent.get_or_insert(depth);
        if depth == level {
            if let Some(name) = trimmed
                .strip_suffix(':')
                .or_else(|| trimmed.split_once(": ").map(|(k, _)| k))
            {
                out.push(name.trim_matches(['"', '\'']).to_string());
            }
        }
    }
    out
}

const COMPOSE_FILES: &[&str] = &[
    "compose.yaml",
    "compose.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
];

impl SuggestionSource for ProjectSource {
    fn name(&self) -> &'static str {
        "project"
    }

    fn suggest(&self, ctx: &Context) -> Vec<Suggestion> {
        let line = &ctx.line;
        if line.current().raw.starts_with('-') {
            return Vec::new();
        }
        let pos = line.positionals();
        if super::specs::arguments(line).1.is_some() {
            return Vec::new();
        }
        let target = match (line.command(), pos.as_slice()) {
            (Some("npm" | "pnpm" | "yarn" | "bun"), ["run" | "run-script"])
            | (Some("pnpm" | "yarn"), []) => Some(("package.json", Kind::PackageScripts)),
            (Some("make" | "gmake"), _) => {
                let mk = ["GNUmakefile", "makefile", "Makefile"]
                    .into_iter()
                    .find(|f| ctx.cwd.join(f).is_file());
                return mk
                    .map(|f| self.names(&ctx.cwd.join(f), Kind::MakeTargets))
                    .map(|n| ctx.ranked_words(n.iter().map(String::as_str), "", 0.55, "project"))
                    .unwrap_or_default();
            }
            (Some("docker"), ["compose", sub]) | (Some("docker-compose"), [sub])
                if matches!(
                    *sub,
                    "up" | "down"
                        | "logs"
                        | "restart"
                        | "exec"
                        | "start"
                        | "stop"
                        | "build"
                        | "run"
                        | "pull"
                        | "ps"
                ) =>
            {
                Some(("compose", Kind::ComposeServices))
            }
            _ => None,
        };
        let Some((file, kind)) = target else {
            return Vec::new();
        };
        let path = match kind {
            Kind::ComposeServices => find_up(&ctx.cwd, COMPOSE_FILES),
            _ => find_up(&ctx.cwd, &[file]),
        };
        let Some(path) = path else { return Vec::new() };
        let names = self.names(&path, kind);
        ctx.ranked_words(names.iter().map(String::as_str), "", 0.55, "project")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_context;

    #[test]
    fn cache_shares_names_refreshes_and_reloads_evicted_projects() {
        let dir = tempfile::tempdir().unwrap();
        let src = ProjectSource::default();
        let file = dir.path().join("package.json");
        std::fs::write(&file, r#"{"scripts":{"dev":"x"}}"#).unwrap();
        let names = src.names(&file, Kind::PackageScripts);
        assert!(Arc::ptr_eq(&names, &src.names(&file, Kind::PackageScripts)));
        // Force a stale timestamp without relying on filesystem clock resolution.
        src.cache
            .lock()
            .unwrap()
            .insert(file.clone(), (SystemTime::UNIX_EPOCH, names.clone()));
        std::fs::write(&file, r#"{"scripts":{"build":"x"}}"#).unwrap();
        assert_eq!(&*src.names(&file, Kind::PackageScripts), &["build"]);
        for i in 0..70 {
            let other = dir.path().join(format!("{i}.json"));
            std::fs::write(&other, r#"{"scripts":{"test":"x"}}"#).unwrap();
            src.names(&other, Kind::PackageScripts);
        }
        assert_eq!(&*src.names(&file, Kind::PackageScripts), &["build"]);
    }

    #[test]
    fn parses_project_files() {
        assert_eq!(
            package_scripts(r#"{"scripts":{"dev":"vite","build":"vite build"}}"#),
            ["build", "dev"]
        );
        assert_eq!(
            make_targets("CC := gcc\nall: build\nbuild test: x\n\techo hi\n.PHONY: all\n"),
            ["all", "build", "test"]
        );
        let compose = "version: '3'\nservices:\n  web:\n    image: x\n    ports:\n      - 80\n  db:\n    image: y\nvolumes:\n  data:\n";
        assert_eq!(compose_services(compose), ["web", "db"]);
    }

    #[test]
    fn suggests_package_scripts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"dev":"vite","deploy":"x"}}"#,
        )
        .unwrap();
        let sub = dir.path().join("src");
        std::fs::create_dir(&sub).unwrap();
        let src = ProjectSource::default();
        let texts: Vec<String> = src
            .suggest(&test_context("npm run de", &sub))
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert_eq!(texts, ["npm run deploy", "npm run dev"]);
        assert!(src
            .suggest(&test_context("npm install de", &sub))
            .is_empty());
    }
}
