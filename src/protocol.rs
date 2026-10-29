
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Complete {
        id: String,
        session: String,
        cwd: String,
        cursor: usize,
        buffer: String,
    },
    Suggest {
        id: String,
        session: String,
        cwd: String,
        buffer: String,
    },
    Record {
        session: String,
        cwd: String,
        status: i32,
        command: String,
    },
    Ping,
    Quit,
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

pub fn parse_request(line: &str) -> Option<Request> {
    let mut f = line.splitn(5, '\t');
    match f.next()? {
        "S" => Some(Request::Suggest {
            id: f.next()?.to_string(),
            session: f.next()?.to_string(),
            cwd: unescape(f.next()?),
            buffer: unescape(f.next()?),
        }),
        "C" => {
            let id = f.next()?.to_string();
            let session = f.next()?.to_string();
            let cwd = unescape(f.next()?);
            let (cursor, buffer) = f.next()?.split_once('\t')?;
            Some(Request::Complete {
                id,
                session,
                cwd,
                cursor: cursor.parse().ok()?,
                buffer: unescape(buffer),
            })
        }
        "R" => Some(Request::Record {
            session: f.next()?.to_string(),
            cwd: unescape(f.next()?),
            status: f.next()?.parse().ok()?,
            command: unescape(f.next()?),
        }),
        "P" => Some(Request::Ping),
        "Q" => Some(Request::Quit),
        _ => None,
    }
}

pub fn format_reply(id: &str, candidates: &[String]) -> String {
    let mut out = String::from(id);
    for c in candidates {
        out.push('\t');
        out.push_str(&escape(c));
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_round_trip() {
        let s = "a\\b\tc\nd\\n";
        assert_eq!(unescape(&escape(s)), s);
        assert!(!escape(s).contains(['\n', '\t']));
    }

    #[test]
    fn parses_requests() {
        assert_eq!(
            parse_request("C\t7\t123\t/tmp\t4\tgit chek\\targ"),
            Some(Request::Complete {
                id: "7".into(),
                session: "123".into(),
                cwd: "/tmp".into(),
                cursor: 4,
                buffer: "git chek\targ".into()
            })
        );
        assert!(parse_request("C\t7\t123\t/tmp\tbad\tgit").is_none());
        assert_eq!(
            parse_request("S\t7\t123\t/tmp\tgit\\tst"),
            Some(Request::Suggest {
                id: "7".into(),
                session: "123".into(),
                cwd: "/tmp".into(),
                buffer: "git\tst".into()
            })
        );
        assert_eq!(
            parse_request("R\t1\t/x\t2\tmake\\ntest"),
            Some(Request::Record {
                session: "1".into(),
                cwd: "/x".into(),
                status: 2,
                command: "make\ntest".into()
            })
        );
        assert_eq!(parse_request("X"), None);
    }
}
