//! Line protocol between shells and the daemon over a Unix socket.
//!
//! One message per line, fields separated by TAB. Free-text fields are
//! escaped so they never contain TAB or newline.
//!
//! Client to daemon:
//! - `Q sid seq cols caps mode cwd euid buffer` synchronous query (zsh), answered with `R`.
//! - `N sid seq cols caps mode cwd euid tty pid down buffer` async notify (bash, fish):
//!   the daemon runs the AI review and repaints the ghost line on `tty` itself.
//! - `S sid` subscribe: keep the connection open and receive pushed `R` frames.
//! - `X sid` the line was accepted or aborted: cancel work, stop animating.
//! - `E sid` the shell exited.
//! - `PING`, `RELOAD`, `STATS`, `STOP`.
//!
//! Daemon to client:
//! - `R seq text specs` (zsh) / `A seq ansi` (ANSI), `PONG version sessions`, `OK`, `STATS json`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            c => o.push(c),
        }
    }
    o
}

pub fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('t') => o.push('\t'),
            Some('n') => o.push('\n'),
            Some('r') => o.push('\r'),
            Some('\\') => o.push('\\'),
            Some(x) => {
                o.push('\\');
                o.push(x);
            }
            None => o.push('\\'),
        }
    }
    o
}

/// Rendering capabilities sent by a shell: color depth code plus flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    pub depth: crate::render::ColorDepth,
    pub italic: bool,
}

impl Caps {
    pub fn parse(s: &str) -> Caps {
        let mut chars = s.chars();
        let depth = crate::render::ColorDepth::from_code(chars.next().unwrap_or('t'));
        Caps { depth, italic: s.contains('i') }
    }

    pub fn encode(&self) -> String {
        let mut s = String::new();
        s.push(self.depth.code());
        if self.italic {
            s.push('i');
        }
        s
    }
}

/// Connects with a short timeout so a hung daemon can never stall typing.
pub fn connect(timeout: Duration) -> Option<UnixStream> {
    let s = UnixStream::connect(crate::store::socket_path()).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    Some(s)
}

/// Sends one line without waiting for a reply.
pub fn send(line: &str) -> bool {
    match connect(Duration::from_millis(50)) {
        Some(mut s) => s.write_all(format!("{line}\n").as_bytes()).is_ok(),
        None => false,
    }
}

/// Sends one line and reads one reply line.
pub fn request(line: &str, timeout: Duration) -> Option<String> {
    let mut s = connect(timeout)?;
    s.write_all(format!("{line}\n").as_bytes()).ok()?;
    let mut r = BufReader::new(s);
    let mut out = String::new();
    r.read_line(&mut out).ok()?;
    if out.is_empty() {
        return None;
    }
    Some(out.trim_end_matches('\n').to_string())
}

pub fn ping() -> Option<String> {
    request("PING", Duration::from_millis(300))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_roundtrip() {
        for s in ["a\tb\nc\\d", "", "\\n literal", "é\t"] {
            assert_eq!(unescape(&escape(s)), s);
            assert!(!escape(s).contains('\t') && !escape(s).contains('\n'));
        }
    }
}
