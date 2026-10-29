use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zghost::config::{self, Config};
use zghost::engine::Engine;
use zghost::store::Store;
use zghost::Result;

const PLUGIN: &str = include_str!("../shell/zghost.zsh");

const USAGE: &str = "\
zghost: inline autosuggestions for Zsh

USAGE:
    eval \"$(zghost init zsh)\"            Add to ~/.zshrc
    zghost daemon [--socket P] [--histfile F] [--foreground]
    zghost suggest [--cwd DIR] [--limit N] [--after CMD] <buffer...>
    zghost complete [--cwd DIR] [--cursor N] [--limit N] <buffer...>
    zghost bench [--histfile F] [--test N]
    zghost import [--histfile F]
    zghost status [--socket P]
    zghost stop [--socket P]
";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = if args.is_empty() {
        String::new()
    } else {
        args.remove(0)
    };
    let result = match cmd.as_str() {
        "init" => init(&args),
        "daemon" => daemon(&mut args),
        "suggest" => suggest(&mut args),
        "complete" => complete(&mut args),
        "bench" => bench(&mut args),
        "import" => import(&mut args),
        "status" => status(&mut args),
        "stop" => stop(&mut args),
        _ => {
            print!("{USAGE}");
            std::process::exit(if cmd.is_empty() || cmd == "help" || cmd == "--help" {
                0
            } else {
                2
            });
        }
    };
    if let Err(e) = result {
        eprintln!("zghost: {e}");
        std::process::exit(1);
    }
}

/// Remove `--name value` from `args` and return the value.
fn opt(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    (i < args.len()).then(|| args.remove(i))
}

fn flag(args: &mut Vec<String>, name: &str) -> bool {
    let found = args.iter().any(|a| a == name);
    args.retain(|a| a != name);
    found
}

fn socket_path(args: &mut Vec<String>) -> PathBuf {
    opt(args, "--socket")
        .map(PathBuf::from)
        .unwrap_or_else(config::default_socket)
}

fn histfile(args: &mut Vec<String>) -> PathBuf {
    opt(args, "--histfile")
        .map(PathBuf::from)
        .unwrap_or_else(config::default_histfile)
}

fn open_engine() -> Result<Engine> {
    Engine::open(
        Config::load(),
        Store::open(&config::data_dir().join("history.db"))?,
    )
}

fn init(args: &[String]) -> Result<()> {
    if args.first().map(String::as_str) != Some("zsh") {
        return Err("only `zghost init zsh` is supported".into());
    }
    let exe = std::env::current_exe()?;
    let quoted = format!("'{}'", exe.to_string_lossy().replace('\'', r"'\''"));
    print!("{}", PLUGIN.replace("__ZGHOST_BIN__", &quoted));
    Ok(())
}

fn daemon(args: &mut Vec<String>) -> Result<()> {
    let socket = socket_path(args);
    let histfile = histfile(args);
    if !flag(args, "--foreground") {
        // Detach from the terminal so closing the tab doesn't kill us.
        unsafe {
            libc::setsid();
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
        }
    }
    let engine = open_engine()?;
    if histfile.is_file() {
        if let Some(n) = engine.import_histfile(&histfile)? {
            eprintln!("zghost: imported {n} commands from {}", histfile.display());
        }
    }
    zghost::daemon::serve(&socket, Arc::new(engine))
}

fn suggest(args: &mut Vec<String>) -> Result<()> {
    let cwd = opt(args, "--cwd")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let limit = opt(args, "--limit")
        .and_then(|n| n.parse().ok())
        .unwrap_or(5);
    let after = opt(args, "--after");
    let buffer = args.join(" ");
    let t = Instant::now();
    let engine = open_engine()?;
    let loaded = t.elapsed();
    if let Some(prev) = &after {
        engine.set_previous("cli", prev);
    }
    let t = Instant::now();
    let ranked = engine.suggest(&buffer, &cwd, "cli", limit);
    let took = t.elapsed();
    for r in &ranked {
        println!("{:.3}  {:<28}  {}", r.score, r.sources.join("+"), r.text);
    }
    eprintln!(
        "({} indexed; load {:?}, query {:?})",
        engine.history_len(),
        loaded,
        took
    );
    Ok(())
}

fn complete(args: &mut Vec<String>) -> Result<()> {
    let cwd = opt(args, "--cwd")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let cursor = opt(args, "--cursor")
        .map(|v| v.parse::<usize>())
        .transpose()?;
    let limit = opt(args, "--limit")
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(20);
    let buffer = args.join(" ");
    let cursor = cursor.unwrap_or(buffer.chars().count());
    let offset = buffer
        .char_indices()
        .nth(cursor)
        .map(|(i, _)| i)
        .or_else(|| (cursor == buffer.chars().count()).then_some(buffer.len()))
        .ok_or("cursor outside buffer")?;
    for c in open_engine()?.complete(&buffer, offset, &cwd, "cli", limit) {
        println!("{}\t{}", c.cursor, c.text);
    }
    Ok(())
}

fn bench(args: &mut Vec<String>) -> Result<()> {
    let histfile = histfile(args);
    let test = opt(args, "--test")
        .and_then(|n| n.parse().ok())
        .unwrap_or(1000);
    zghost::bench::run(&histfile, test)
}

fn import(args: &mut Vec<String>) -> Result<()> {
    let histfile = histfile(args);
    match open_engine()?.import_histfile(&histfile)? {
        Some(n) => println!("imported {n} commands from {}", histfile.display()),
        None => println!("history was already imported; nothing to do"),
    }
    Ok(())
}

fn connect(args: &mut Vec<String>) -> Result<UnixStream> {
    let socket = socket_path(args);
    UnixStream::connect(&socket)
        .map_err(|e| format!("no daemon at {} ({e})", socket.display()).into())
}

fn status(args: &mut Vec<String>) -> Result<()> {
    let mut s = connect(args)?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    let t = Instant::now();
    s.write_all(b"P\n")?;
    let mut line = String::new();
    BufReader::new(&s).read_line(&mut line)?;
    println!("daemon is running (round trip {:?})", t.elapsed());
    Ok(())
}

fn stop(args: &mut Vec<String>) -> Result<()> {
    connect(args)?.write_all(b"Q\n")?;
    println!("daemon stopped");
    Ok(())
}
