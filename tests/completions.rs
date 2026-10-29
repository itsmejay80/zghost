use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use zghost::{config::Config, engine::Engine, store::Store};

fn fixture() -> (TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"scripts":{"dev":"echo dev","deploy":"echo deploy"}}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("my notes.md"), "").unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    let engine = Engine::open(
        Config::default(),
        Store::open(&dir.path().join("db")).unwrap(),
    )
    .unwrap();
    (dir, engine)
}
fn suggestions(engine: &Engine, cwd: &Path, input: &str) -> Vec<String> {
    engine
        .suggest(input, cwd, "test", 100)
        .into_iter()
        .map(|r| r.text)
        .collect()
}
fn completions(engine: &Engine, cwd: &Path, input: &str) -> Vec<String> {
    engine
        .complete(input, input.len(), cwd, "test", 100)
        .into_iter()
        .map(|r| r.text)
        .collect()
}

#[test]
fn discovers_flags_subcommands_and_values_without_history() {
    let (dir, e) = fixture();
    let cwd = dir.path();
    for (input, expected) in [
        ("cargo build --r", "cargo build --release "),
        (
            "cargo build --manifest-path m",
            "cargo build --manifest-path my\\ notes.md ",
        ),
        ("gh pr c", "gh pr create "),
        ("gh pr create --d", "gh pr create --draft "),
        ("terraform pl", "terraform plan "),
        ("ssh -", "ssh -i "),
        ("kubectl get --output ", "kubectl get --output json "),
        ("kubectl get --output=y", "kubectl get --output=yaml "),
        ("npm run ", "npm run dev"),
        ("cat ", "cat my\\ notes.md "),
        ("cd ", "cd src/"),
    ] {
        assert!(
            suggestions(&e, cwd, input).contains(&expected.into()),
            "{input}: {:?}",
            suggestions(&e, cwd, input)
        );
    }
    assert!(suggestions(&e, cwd, "cargo build --release --r").is_empty());
    assert!(suggestions(&e, cwd, "cargo build -- --r").is_empty());
    assert!(suggestions(&e, cwd, "cargo build --package ").is_empty());
    assert!(suggestions(&e, cwd, "python -c ").is_empty());
    assert!(suggestions(&e, cwd, "git checkout --no-p").is_empty());
    assert!(suggestions(&e, cwd, "docker run --env FOO=bar --e")
        .contains(&"docker run --env FOO=bar --env ".into()));
    assert!(suggestions(&e, cwd, "gh --repo owner/name pr c")
        .contains(&"gh --repo owner/name pr create ".into()));
    assert!(suggestions(&e, cwd, "kubectl --namespace default get p")
        .contains(&"kubectl --namespace default get pods ".into()));
}

#[test]
fn fuzzy_candidates_replace_words_and_stay_out_of_ghost_text() {
    let (dir, e) = fixture();
    let cwd = dir.path();
    assert!(suggestions(&e, cwd, "git chek").is_empty());
    assert!(completions(&e, cwd, "git chek").contains(&"git checkout ".into()));
    assert!(completions(&e, cwd, "git ckt").contains(&"git checkout ".into()));
    assert!(completions(&e, cwd, "cat mn").contains(&"cat my\\ notes.md ".into()));
    assert!(completions(&e, cwd, "git ch")[0].starts_with("git ch"));
    assert!(completions(&e, cwd, "git co f").is_empty());
}

#[test]
fn midline_edits_preserve_tail_and_return_character_cursor() {
    let (dir, e) = fixture();
    let input = "echo é && git chek --detach";
    let at = input.find(" --detach").unwrap();
    let result = e.complete(input, at, dir.path(), "test", 100);
    let c = result
        .iter()
        .find(|c| c.text == "echo é && git checkout --detach")
        .unwrap();
    assert_eq!(c.cursor, "echo é && git checkout".chars().count());
    let input = "gh pr crXX --draft";
    let c = e
        .complete(input, 8, dir.path(), "test", 100)
        .into_iter()
        .find(|c| c.text == "gh pr create --draft")
        .unwrap();
    assert_eq!(c.cursor, "gh pr create".chars().count());
    assert!(e.complete("é", 1, dir.path(), "test", 100).is_empty());
    assert!(e.complete("git", 100, dir.path(), "test", 100).is_empty());
}

#[test]
fn quote_aware_paths_and_option_paths_are_insertable() {
    let (dir, e) = fixture();
    let cwd = dir.path();
    assert!(suggestions(&e, cwd, "cat \"my").contains(&"cat \"my notes.md\" ".into()));
    assert!(suggestions(&e, cwd, "cat 'my").contains(&"cat 'my notes.md' ".into()));
    assert!(suggestions(&e, cwd, "cargo build --manifest-path=m")
        .contains(&"cargo build --manifest-path=my\\ notes.md ".into()));
    let input = "cat \"my\" --flag";
    assert!(e
        .complete(input, 7, cwd, "test", 100)
        .iter()
        .any(|c| c.text == "cat \"my notes.md\" --flag"));
}

#[test]
fn git_refs_are_discovered_before_typing_a_prefix() {
    let (dir, e) = fixture();
    let cwd = dir.path();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["commit", "-q", "--allow-empty", "-m", "fixture"],
        vec!["branch", "feature/login"],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid"
            ])
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    assert!(suggestions(&e, cwd, "git checkout ").contains(&"git checkout feature/login".into()));
    assert!(suggestions(&e, cwd, "git -C . checkout f")
        .contains(&"git -C . checkout feature/login".into()));
}
