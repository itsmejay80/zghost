use super::{Context, Suggestion, SuggestionSource};
use crate::parser::ParsedLine;
mod flags;
pub(crate) use flags::Value;
use flags::{Flag, FLAGS};

/// Built-in command specs: subcommands for popular tools, most common first.
/// Keys are the command plus any already-typed subcommands.
const SPECS: &[(&str, &[&str])] = &[
    (
        "gh",
        &[
            "pr", "issue", "repo", "run", "workflow", "release", "auth", "api", "search",
        ],
    ),
    (
        "gh pr",
        &[
            "create", "checkout", "list", "view", "status", "diff", "checks", "merge", "review",
            "close", "reopen",
        ],
    ),
    (
        "gh issue",
        &[
            "create", "list", "view", "status", "close", "reopen", "comment",
        ],
    ),
    (
        "gh repo",
        &["clone", "create", "view", "fork", "list", "sync"],
    ),
    (
        "gh run",
        &["list", "view", "watch", "rerun", "cancel", "download"],
    ),
    (
        "gh auth",
        &["login", "logout", "status", "refresh", "token"],
    ),
    (
        "terraform",
        &[
            "init",
            "plan",
            "apply",
            "destroy",
            "fmt",
            "validate",
            "output",
            "state",
            "workspace",
            "import",
            "show",
            "version",
        ],
    ),
    (
        "terraform workspace",
        &["list", "select", "new", "delete", "show"],
    ),
    (
        "terraform state",
        &["list", "show", "mv", "rm", "pull", "push"],
    ),
    ("ssh", &[]),
    ("scp", &[]),
    ("rsync", &[]),
    ("curl", &[]),
    ("rg", &[]),
    ("find", &[]),
    ("python", &[]),
    ("python3", &[]),
    (
        "pip",
        &[
            "install",
            "uninstall",
            "list",
            "show",
            "freeze",
            "check",
            "download",
            "cache",
        ],
    ),
    (
        "pip3",
        &[
            "install",
            "uninstall",
            "list",
            "show",
            "freeze",
            "check",
            "download",
            "cache",
        ],
    ),
    (
        "uv",
        &[
            "run", "init", "add", "remove", "sync", "lock", "tree", "venv", "pip", "python",
            "tool", "build", "publish",
        ],
    ),
    (
        "uv pip",
        &[
            "install",
            "uninstall",
            "list",
            "show",
            "freeze",
            "compile",
            "sync",
        ],
    ),
    (
        "poetry",
        &[
            "install", "add", "remove", "run", "env", "lock", "update", "build", "publish", "show",
        ],
    ),
    (
        "rustup",
        &[
            "update",
            "check",
            "default",
            "toolchain",
            "target",
            "component",
            "override",
            "show",
        ],
    ),
    ("rustup target", &["add", "remove", "list"]),
    (
        "helm",
        &[
            "install",
            "upgrade",
            "uninstall",
            "list",
            "status",
            "repo",
            "search",
            "template",
            "lint",
            "package",
            "show",
        ],
    ),
    ("helm repo", &["add", "remove", "list", "update"]),
    (
        "aws",
        &[
            "s3",
            "s3api",
            "ec2",
            "ecs",
            "eks",
            "iam",
            "sts",
            "lambda",
            "logs",
            "configure",
            "cloudformation",
        ],
    ),
    (
        "aws s3",
        &["ls", "cp", "sync", "mv", "rm", "mb", "rb", "presign"],
    ),
    (
        "gcloud",
        &[
            "auth",
            "config",
            "compute",
            "container",
            "run",
            "projects",
            "storage",
            "builds",
        ],
    ),
    ("just", &[]),
    ("cmake", &[]),
    (
        "docker container",
        &[
            "ls", "run", "exec", "stop", "start", "rm", "inspect", "logs",
        ],
    ),
    (
        "docker image",
        &["ls", "build", "pull", "push", "rm", "inspect", "prune"],
    ),
    (
        "docker network",
        &[
            "ls",
            "create",
            "inspect",
            "connect",
            "disconnect",
            "rm",
            "prune",
        ],
    ),
    ("docker volume", &["ls", "create", "inspect", "rm", "prune"]),
    (
        "kubectl get",
        &[
            "pods",
            "services",
            "deployments",
            "namespaces",
            "nodes",
            "configmaps",
            "secrets",
            "ingresses",
            "jobs",
            "events",
        ],
    ),
    (
        "kubectl config",
        &[
            "get-contexts",
            "current-context",
            "use-context",
            "set-context",
            "view",
        ],
    ),
    (
        "kubectl rollout",
        &["status", "restart", "undo", "history", "pause", "resume"],
    ),
    (
        "brew services",
        &["list", "start", "stop", "restart", "run", "cleanup"],
    ),
    (
        "git",
        &[
            "status",
            "commit",
            "push",
            "pull",
            "checkout",
            "add",
            "diff",
            "log",
            "switch",
            "branch",
            "merge",
            "rebase",
            "stash",
            "fetch",
            "clone",
            "reset",
            "restore",
            "remote",
            "show",
            "tag",
            "cherry-pick",
            "init",
            "config",
            "rm",
            "mv",
            "grep",
            "blame",
            "bisect",
            "clean",
            "reflog",
            "revert",
            "worktree",
        ],
    ),
    (
        "git stash",
        &["pop", "list", "apply", "push", "show", "drop", "clear"],
    ),
    (
        "git remote",
        &["add", "remove", "rename", "set-url", "show", "prune"],
    ),
    ("git worktree", &["add", "list", "remove", "prune"]),
    (
        "cargo",
        &[
            "build", "run", "test", "check", "clippy", "fmt", "add", "new", "init", "doc", "bench",
            "clean", "install", "update", "remove", "tree", "publish", "search",
        ],
    ),
    (
        "docker",
        &[
            "ps", "run", "exec", "compose", "build", "images", "logs", "pull", "push", "stop",
            "start", "rm", "rmi", "inspect", "network", "volume", "system",
        ],
    ),
    (
        "docker compose",
        &[
            "up", "down", "logs", "ps", "build", "exec", "restart", "pull", "run", "start", "stop",
        ],
    ),
    (
        "npm",
        &[
            "install",
            "run",
            "test",
            "start",
            "ci",
            "exec",
            "init",
            "uninstall",
            "update",
            "outdated",
            "publish",
            "link",
            "list",
        ],
    ),
    (
        "pnpm",
        &["install", "add", "run", "dlx", "exec", "remove", "update"],
    ),
    ("yarn", &["add", "install", "remove", "run", "upgrade"]),
    (
        "bun",
        &["run", "install", "add", "remove", "test", "x", "build"],
    ),
    (
        "kubectl",
        &[
            "get",
            "describe",
            "logs",
            "apply",
            "delete",
            "exec",
            "port-forward",
            "rollout",
            "scale",
            "edit",
            "create",
            "config",
        ],
    ),
    (
        "brew",
        &[
            "install",
            "upgrade",
            "update",
            "search",
            "info",
            "list",
            "uninstall",
            "outdated",
            "services",
            "cleanup",
            "doctor",
        ],
    ),
    (
        "go",
        &[
            "run", "build", "test", "mod", "get", "install", "fmt", "vet",
        ],
    ),
    (
        "go mod",
        &["tidy", "init", "download", "vendor", "why", "graph"],
    ),
];

pub struct SpecSource;

impl SuggestionSource for SpecSource {
    fn name(&self) -> &'static str {
        "specs"
    }

    fn suggest(&self, ctx: &Context) -> Vec<Suggestion> {
        let Some(cmd) = ctx.line.command() else {
            return Vec::new();
        };
        let cur = ctx.line.current().value.as_str();
        let (pos, pending) = arguments(&ctx.line);
        let key = std::iter::once(cmd)
            .chain(pos.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        let (value, prefix) = if let Some((name, _)) = cur.split_once('=') {
            (find_flag(&key, name).map(|f| f.value), format!("{name}="))
        } else {
            (pending.map(|f| f.value), String::new())
        };
        if let Some(value) = value {
            return match value {
                Value::Choices(values) => ctx.ranked_words(
                    values
                        .iter()
                        .map(|v| format!("{prefix}{v}"))
                        .collect::<Vec<_>>()
                        .iter()
                        .map(String::as_str),
                    " ",
                    0.5,
                    "specs",
                ),
                _ => Vec::new(),
            };
        }
        // `--` ends option parsing even when the next word starts with a dash.
        let options_ended = ctx
            .line
            .words
            .iter()
            .take(ctx.line.words.len() - 1)
            .any(|w| w.value == "--");
        let mut out = Vec::new();
        if !cur.starts_with('-') {
            if let Some((_, subs)) = SPECS.iter().find(|(k, _)| *k == key) {
                out.extend(ctx.ranked_words(subs.iter().copied(), " ", 0.4, "specs"));
            }
        }
        if !options_ended && (cur.starts_with('-') || (ctx.fuzzy && cur.is_empty())) {
            for flag in active_flags(&key) {
                let used = ctx.line.words[..ctx.line.words.len() - 1].iter().any(|w| {
                    flag.names
                        .split_whitespace()
                        .any(|n| w.value == n || w.value.starts_with(&format!("{n}=")))
                });
                if used && !flag.repeatable() {
                    continue;
                }
                out.extend(ctx.ranked_words(
                    flag.names.split_whitespace(),
                    " ",
                    if cur.is_empty() { 0.3 } else { 0.45 },
                    "specs",
                ));
            }
            if !cur.is_empty()
                && !["ssh", "scp", "go", "find"].contains(&cmd)
                && SPECS.iter().any(|(k, _)| *k == cmd)
                && !ctx.line.words[..ctx.line.words.len() - 1]
                    .iter()
                    .any(|w| w.value == "--help")
            {
                out.extend(ctx.ranked_words(["--help"], " ", 0.35, "specs"));
            }
        }
        out
    }
}

/// Active option metadata includes inherited global options.
fn active_flags(key: &str) -> impl Iterator<Item = &'static Flag> + '_ {
    FLAGS.iter().filter(move |f| {
        key == f.scope
            || (!matches!(f.scope, "git" | "docker" | "docker compose" | "terraform")
                && key
                    .strip_prefix(f.scope)
                    .is_some_and(|s| s.starts_with(' ')))
    })
}
fn find_flag(key: &str, name: &str) -> Option<&'static Flag> {
    active_flags(key).find(|f| f.names.split_whitespace().any(|n| n == name))
}

/// Classify completed arguments, respecting option values and `--`.
/// The pending flag (if any) describes the word currently being typed.
pub(crate) fn arguments(line: &ParsedLine) -> (Vec<&str>, Option<&'static Flag>) {
    let Some(index) = line.command else {
        return (Vec::new(), None);
    };
    if index >= line.words.len() - 1 {
        return (Vec::new(), None);
    }
    let cmd = line.words[index].value.as_str();
    let mut key = cmd.to_string();
    let mut pos = Vec::new();
    let mut pending: Option<&Flag> = None;
    let mut ended = false;
    for word in &line.words[index + 1..line.words.len() - 1] {
        let value = word.value.as_str();
        if pending.take().is_some() {
            continue;
        }
        if !ended && value == "--" {
            ended = true;
            continue;
        }
        if !ended && value.starts_with('-') {
            let name = value.split('=').next().unwrap_or(value);
            if !value.contains('=') {
                pending = find_flag(&key, name).filter(|f| f.value != Value::None);
            }
            continue;
        }
        pos.push(value);
        key.push(' ');
        key.push_str(value);
    }
    (pos, pending)
}

/// Path-valued options are shared with the file source.
pub(crate) fn path_value(line: &ParsedLine) -> Option<Value> {
    let (pos, pending) = arguments(line);
    let key = std::iter::once(line.command()?)
        .chain(pos)
        .collect::<Vec<_>>()
        .join(" ");
    let flag = line
        .current()
        .value
        .split_once('=')
        .and_then(|(n, _)| find_flag(&key, n))
        .or(pending)?;
    matches!(flag.value, Value::File | Value::Directory).then_some(flag.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_context;
    use std::path::Path;

    #[test]
    fn suggests_subcommands_in_popularity_order() {
        let s = SpecSource.suggest(&test_context("git st", Path::new("/")));
        let texts: Vec<_> = s.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["git status ", "git stash "]);
        let s = SpecSource.suggest(&test_context("docker compose u", Path::new("/")));
        assert_eq!(s[0].text, "docker compose up ");
    }
}
