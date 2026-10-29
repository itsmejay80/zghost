//! Offline replay of a Zsh history file to measure suggestion quality and
//! latency. Older commands train the index; the newest `test` commands are
//! "typed" one character at a time and checked against the top suggestion.

use crate::config::Config;
use crate::engine::Engine;
use crate::store::parse_zsh_history;
use crate::Result;
use std::path::Path;
use std::time::{Duration, Instant};

pub fn run(histfile: &Path, test: usize) -> Result<()> {
    let entries: Vec<(i64, String)> = parse_zsh_history(&std::fs::read(histfile)?)
        .into_iter()
        .filter(|(_, c)| !c.starts_with(' '))
        .collect();
    let engine = Engine::history_only(Config::load());
    let split = entries.len().saturating_sub(test);
    for (ts, cmd) in &entries[..split] {
        engine.record("bench", None, cmd, 0, *ts);
    }
    // Baseline: zsh-autosuggestions' default strategy, most recent match.
    let mut recent: Vec<&str> = entries[..split].iter().map(|(_, c)| c.as_str()).collect();
    let (mut base_predicted, mut base_saved) = (0usize, 0usize);

    let cwd = Path::new("/");
    let mut latencies: Vec<Duration> = Vec::new();
    let (mut predicted, mut within3, mut total_chars, mut saved_chars) =
        (0usize, 0usize, 0usize, 0usize);
    let (mut next_shown, mut next_right) = (0usize, 0usize);
    for (ts, cmd) in &entries[split..] {
        total_chars += cmd.len();
        if let Some(top) = engine.suggest_at("", cwd, "bench", 1, *ts).first() {
            next_shown += 1;
            next_right += (top.text == *cmd) as usize;
        }
        for (k, _) in cmd.char_indices().skip(1) {
            let prefix = &cmd[..k];
            let hit = recent
                .iter()
                .rev()
                .find(|c| c.len() > k && c.starts_with(prefix));
            if hit == Some(&cmd.as_str()) {
                base_predicted += 1;
                base_saved += cmd.len() - k;
                break;
            }
        }
        recent.push(cmd);
        for (k, _) in cmd.char_indices().skip(1) {
            let t = Instant::now();
            let top = engine.suggest_at(&cmd[..k], cwd, "bench", 1, *ts);
            latencies.push(t.elapsed());
            if top.first().is_some_and(|r| r.text == *cmd) {
                predicted += 1;
                within3 += (k <= 3) as usize;
                saved_chars += cmd.len() - k;
                break;
            }
        }
        engine.record("bench", None, cmd, 0, *ts);
    }

    let tested = entries.len() - split;
    let pct = |n: usize, d: usize| {
        if d == 0 {
            0.0
        } else {
            100.0 * n as f64 / d as f64
        }
    };
    latencies.sort();
    let q = |p: f64| {
        latencies
            .get(((latencies.len() as f64 * p) as usize).min(latencies.len().saturating_sub(1)))
    };
    let us = |d: Option<&Duration>| d.map(|d| d.as_secs_f64() * 1e6).unwrap_or(0.0);

    println!("zghost ranking");
    println!("history file      {}", histfile.display());
    println!("training commands {split}");
    println!("test commands     {tested}");
    println!("distinct indexed  {}", engine.history_len());
    println!();
    println!(
        "predicted before fully typed  {:5.1}%",
        pct(predicted, tested)
    );
    println!(
        "predicted within 3 chars      {:5.1}%",
        pct(within3, tested)
    );
    println!(
        "keystrokes saved              {:5.1}%",
        pct(saved_chars, total_chars)
    );
    println!(
        "empty prompt: shown {:5.1}%, right when shown {:5.1}%",
        pct(next_shown, tested),
        pct(next_right, next_shown)
    );
    println!();
    println!("baseline (most recent match, like zsh-autosuggestions)");
    println!(
        "predicted before fully typed  {:5.1}%",
        pct(base_predicted, tested)
    );
    println!(
        "keystrokes saved              {:5.1}%",
        pct(base_saved, total_chars)
    );
    println!();
    println!("queries           {}", latencies.len());
    println!("latency p50       {:8.1} us", us(q(0.50)));
    println!("latency p95       {:8.1} us", us(q(0.95)));
    println!("latency p99       {:8.1} us", us(q(0.99)));
    println!("latency max       {:8.1} us", us(latencies.last()));
    Ok(())
}
