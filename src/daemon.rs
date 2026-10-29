//! Unix-socket server. One thread per connected shell; requests are
//! line-delimited (see `protocol`).

use crate::engine::{now, Engine};
use crate::protocol::{format_reply, parse_request, Request};
use crate::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn serve(socket: &Path, engine: Arc<Engine>) -> Result<()> {
    if socket.exists() {
        if UnixStream::connect(socket).is_ok() {
            return Err(format!("a daemon is already listening on {}", socket.display()).into());
        }
        std::fs::remove_file(socket)?;
    }
    // Only the owner may connect: the socket exposes command history.
    let listener = unsafe {
        let old = libc::umask(0o077);
        let l = UnixListener::bind(socket);
        libc::umask(old);
        l?
    };
    eprintln!(
        "zghost: listening on {} ({} commands indexed)",
        socket.display(),
        engine.history_len()
    );

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let engine = engine.clone();
        let socket = socket.to_path_buf();
        std::thread::spawn(move || {
            let _ = handle(stream, &engine, &socket);
        });
    }
    Ok(())
}

fn handle(stream: UnixStream, engine: &Engine, socket: &PathBuf) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            return Ok(());
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim_end_matches(['\n', '\r']);
        match parse_request(line) {
            Some(Request::Complete {
                id,
                session,
                cwd,
                cursor,
                buffer,
            }) => {
                let offset = buffer
                    .char_indices()
                    .nth(cursor)
                    .map(|(i, _)| i)
                    .or_else(|| (cursor == buffer.chars().count()).then_some(buffer.len()));
                let texts = offset
                    .map(|cursor| {
                        let start = crate::parser::parse(&buffer[..cursor]).current().start;
                        let end = crate::parser::word_end(&buffer, start, cursor);
                        let mut texts = vec![format!(
                            "@{}:{}",
                            buffer[..start].chars().count(),
                            buffer[..end].chars().count()
                        )];
                        texts.extend(
                            engine
                                .complete(
                                    &buffer,
                                    cursor,
                                    Path::new(&cwd),
                                    &session,
                                    engine.max_suggestions().max(20),
                                )
                                .into_iter()
                                .map(|r| format!("{}:{}", r.cursor, r.text)),
                        );
                        texts
                    })
                    .unwrap_or_default();
                writer.write_all(format_reply(&id, &texts).as_bytes())?;
            }
            Some(Request::Suggest {
                id,
                session,
                cwd,
                buffer,
            }) => {
                let texts: Vec<String> = engine
                    .suggest(&buffer, Path::new(&cwd), &session, engine.max_suggestions())
                    .into_iter()
                    .map(|r| r.text)
                    .collect();
                writer.write_all(format_reply(&id, &texts).as_bytes())?;
            }
            Some(Request::Record {
                session,
                cwd,
                status,
                command,
            }) => {
                engine.record(&session, Some(&cwd), &command, status, now());
            }
            Some(Request::Ping) => writer.write_all(b"PONG\n")?,
            Some(Request::Quit) => {
                let _ = std::fs::remove_file(socket);
                std::process::exit(0);
            }
            None => {}
        }
    }
}
