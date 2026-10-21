//! Incremental, allocation-light shell lexer. It understands enough of POSIX
//! shell grammar to split a line into simple commands: quotes, escapes,
//! pipelines, `&&` / `||` / `;` / `&`, redirections, `VAR=x` prefixes,
//! comments, `$( )` / backtick / `<( )` substitutions and `$'..'` quoting.
//! It never fails: half-typed input yields a best-effort parse.

use std::borrow::Cow;

use smallvec::SmallVec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Start,
    Pipe,
    And,
    Or,
    Seq,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word<'a> {
    pub raw: &'a str,
    pub value: Cow<'a, str>,
    pub quoted: bool,
}

impl Word<'_> {
    pub fn as_str(&self) -> &str {
        &self.value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirKind {
    Write,
    Append,
    Read,
    Dup,
    HereDoc,
    HereStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect<'a> {
    pub kind: RedirKind,
    pub fd: Option<u32>,
    pub target: Cow<'a, str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment<'a> {
    /// Operator that joined this segment to the previous one.
    pub op: Op,
    pub assigns: SmallVec<[&'a str; 2]>,
    pub words: SmallVec<[Word<'a>; 8]>,
    pub redirects: SmallVec<[Redirect<'a>; 2]>,
}

impl Segment<'_> {
    fn new(op: Op) -> Self {
        Segment { op, assigns: SmallVec::new(), words: SmallVec::new(), redirects: SmallVec::new() }
    }

    fn is_empty(&self) -> bool {
        self.words.is_empty() && self.assigns.is_empty() && self.redirects.is_empty()
    }

    /// True if the next segment receives this one's output.
    pub fn piped_into_next(&self, next: &Segment) -> bool {
        next.op == Op::Pipe
    }
}

#[derive(Debug, Clone, Default)]
pub struct Parsed<'a> {
    pub segments: SmallVec<[Segment<'a>; 4]>,
    /// Bodies of `$( )`, backtick and `<( )` substitutions, analyzed recursively.
    pub subs: SmallVec<[&'a str; 2]>,
    /// The line ends in whitespace, so the last word is complete.
    pub trailing_space: bool,
    pub open_quote: bool,
    pub fork_bomb: bool,
}

fn is_meta(b: u8) -> bool {
    matches!(b, b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' | b'\n' | b' ' | b'\t' | b'\r')
}

fn is_assignment(w: &str) -> bool {
    let Some(eq) = w.find('=') else { return false };
    let name = &w[..eq];
    let name = name.strip_suffix('+').unwrap_or(name);
    !name.is_empty()
        && name.bytes().enumerate().all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
}

/// Skips a balanced `( ... )` starting right after the opening paren.
/// Returns the index just past the closing paren (or the end of input).
fn skip_parens(b: &[u8], mut i: usize) -> (usize, bool) {
    let mut depth = 1usize;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'\'' => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return (i + 1, true);
                }
            }
            _ => {}
        }
        i += 1;
    }
    (b.len(), false)
}

struct WordScan<'a> {
    word: Word<'a>,
    end: usize,
}

/// Scans one word starting at `start`. Records substitutions into `subs`.
fn scan_word<'a>(s: &'a str, start: usize, subs: &mut SmallVec<[&'a str; 2]>, open_quote: &mut bool) -> WordScan<'a> {
    let b = s.as_bytes();
    let mut i = start;
    let mut owned: Option<String> = None;
    let mut quoted = false;
    // Copies s[from..to] into the owned buffer if we are building one.
    macro_rules! flush {
        ($from:expr, $to:expr) => {
            if let Some(o) = owned.as_mut() {
                o.push_str(&s[$from..$to]);
            }
        };
    }
    let mut lit_start = start;
    let ensure_owned = |owned: &mut Option<String>, upto: usize| {
        if owned.is_none() {
            *owned = Some(s[start..upto].to_string());
        }
    };
    while i < b.len() {
        let c = b[i];
        if is_meta(c) {
            // `<(` and `>(` process substitution glued to a word start.
            if (c == b'<' || c == b'>') && i == start && b.get(i + 1) == Some(&b'(') {
                let (end, _) = skip_parens(b, i + 2);
                let inner_end = end.saturating_sub(1).max(i + 2);
                subs.push(&s[i + 2..inner_end.min(b.len())]);
                i = end;
                continue;
            }
            break;
        }
        match c {
            b'\'' => {
                quoted = true;
                flush!(lit_start, i);
                ensure_owned(&mut owned, i);
                let q = i + 1;
                let mut j = q;
                while j < b.len() && b[j] != b'\'' {
                    j += 1;
                }
                if j >= b.len() {
                    *open_quote = true;
                }
                owned.as_mut().unwrap().push_str(&s[q..j.min(b.len())]);
                i = (j + 1).min(b.len());
                lit_start = i;
            }
            b'"' => {
                quoted = true;
                flush!(lit_start, i);
                ensure_owned(&mut owned, i);
                let mut j = i + 1;
                let mut seg = j;
                let mut closed = false;
                while j < b.len() {
                    match b[j] {
                        b'"' => {
                            closed = true;
                            break;
                        }
                        b'\\' if j + 1 < b.len() && matches!(b[j + 1], b'"' | b'\\' | b'$' | b'`') => {
                            owned.as_mut().unwrap().push_str(&s[seg..j]);
                            seg = j + 1;
                            j += 2;
                            continue;
                        }
                        b'$' if b.get(j + 1) == Some(&b'(') => {
                            let (end, _) = skip_parens(b, j + 2);
                            subs.push(&s[j + 2..end.saturating_sub(1).max(j + 2).min(b.len())]);
                            j = end;
                            continue;
                        }
                        b'`' => {
                            let k = s[j + 1..].find('`').map(|k| j + 1 + k).unwrap_or(b.len());
                            subs.push(&s[j + 1..k]);
                            j = (k + 1).min(b.len());
                            continue;
                        }
                        _ => {}
                    }
                    j += 1;
                }
                let j = j.min(b.len());
                owned.as_mut().unwrap().push_str(&s[seg..j]);
                if !closed {
                    *open_quote = true;
                }
                i = if closed { j + 1 } else { j };
                lit_start = i;
            }
            b'\\' => {
                flush!(lit_start, i);
                ensure_owned(&mut owned, i);
                if i + 1 < b.len() {
                    let ch_len = s[i + 1..].chars().next().map(char::len_utf8).unwrap_or(1);
                    if b[i + 1] != b'\n' {
                        owned.as_mut().unwrap().push_str(&s[i + 1..i + 1 + ch_len]);
                    }
                    i += 1 + ch_len;
                } else {
                    i += 1;
                }
                lit_start = i;
            }
            b'$' if b.get(i + 1) == Some(&b'(') => {
                let (end, _) = skip_parens(b, i + 2);
                subs.push(&s[i + 2..end.saturating_sub(1).max(i + 2).min(b.len())]);
                i = end;
            }
            b'$' if b.get(i + 1) == Some(&b'\'') => {
                quoted = true;
                flush!(lit_start, i);
                ensure_owned(&mut owned, i);
                let mut j = i + 2;
                while j < b.len() && b[j] != b'\'' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                let j = j.min(b.len());
                owned.as_mut().unwrap().push_str(&s[i + 2..j]);
                if j >= b.len() {
                    *open_quote = true;
                }
                i = (j + 1).min(b.len());
                lit_start = i;
            }
            b'$' if b.get(i + 1) == Some(&b'{') => {
                let k = s[i..].find('}').map(|k| i + k + 1).unwrap_or(b.len());
                i = k;
            }
            b'`' => {
                let k = s[i + 1..].find('`').map(|k| i + 1 + k).unwrap_or(b.len());
                subs.push(&s[i + 1..k]);
                i = (k + 1).min(b.len());
            }
            _ => i += 1,
        }
    }
    let end = i.min(b.len());
    flush!(lit_start.min(end), end);
    let raw = &s[start..end];
    let value = match owned {
        Some(o) => Cow::Owned(o),
        None => Cow::Borrowed(raw),
    };
    WordScan { word: Word { raw, value, quoted }, end }
}

/// Lexes a full line.
pub fn lex(s: &str) -> Parsed<'_> {
    let b = s.as_bytes();
    let mut p = Parsed { fork_bomb: detect_fork_bomb(s), ..Default::default() };
    let mut seg = Segment::new(Op::Start);
    let mut i = 0usize;
    let mut pending_redir: Option<(RedirKind, Option<u32>)> = None;
    let mut open_quote = false;

    macro_rules! push_seg {
        ($op:expr) => {{
            let done = std::mem::replace(&mut seg, Segment::new($op));
            if !done.is_empty() {
                p.segments.push(done);
            } else if let Some(last) = p.segments.last() {
                // `a | | b`: keep the stronger joining operator.
                let _ = last;
            }
        }};
    }

    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            b'\n' => {
                push_seg!(Op::Seq);
                i += 1;
                continue;
            }
            b'#' => {
                // Comment only at word start.
                let k = s[i..].find('\n').map(|k| i + k).unwrap_or(b.len());
                i = k;
                continue;
            }
            b'|' => {
                if b.get(i + 1) == Some(&b'|') {
                    push_seg!(Op::Or);
                    i += 2;
                } else {
                    push_seg!(Op::Pipe);
                    i += if b.get(i + 1) == Some(&b'&') { 2 } else { 1 };
                }
                continue;
            }
            b'&' => {
                if b.get(i + 1) == Some(&b'&') {
                    push_seg!(Op::And);
                    i += 2;
                } else if b.get(i + 1) == Some(&b'>') {
                    let append = b.get(i + 2) == Some(&b'>');
                    pending_redir = Some((if append { RedirKind::Append } else { RedirKind::Write }, Some(1)));
                    i += if append { 3 } else { 2 };
                } else {
                    push_seg!(Op::Background);
                    i += 1;
                }
                continue;
            }
            b';' => {
                push_seg!(Op::Seq);
                i += if b.get(i + 1) == Some(&b';') { 2 } else { 1 };
                continue;
            }
            b'(' | b')' => {
                push_seg!(Op::Seq);
                i += 1;
                continue;
            }
            b'<' | b'>' if b.get(i + 1) != Some(&b'(') => {
                let (kind, len) = redir_at(b, i);
                pending_redir = Some((kind, None));
                i += len;
                if kind == RedirKind::Dup {
                    // `>&2` / `<&0`: consume the fd number.
                    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'-') {
                        i += 1;
                    }
                    pending_redir = None;
                    seg.redirects.push(Redirect { kind, fd: None, target: Cow::Borrowed("") });
                }
                continue;
            }
            _ => {}
        }

        // `2>file`, `2>&1`: digits glued to a redirect operator.
        if c.is_ascii_digit() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < b.len() && (b[j] == b'>' || b[j] == b'<') && b.get(j + 1) != Some(&b'(') {
                let fd = s[i..j].parse().ok();
                let (kind, len) = redir_at(b, j);
                i = j + len;
                if kind == RedirKind::Dup {
                    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'-') {
                        i += 1;
                    }
                    seg.redirects.push(Redirect { kind, fd, target: Cow::Borrowed("") });
                } else {
                    pending_redir = Some((kind, fd));
                }
                continue;
            }
        }

        let ws = scan_word(s, i, &mut p.subs, &mut open_quote);
        i = if ws.end > i { ws.end } else { i + 1 };
        let word = ws.word;
        if let Some((kind, fd)) = pending_redir.take() {
            seg.redirects.push(Redirect { kind, fd, target: word.value });
            continue;
        }
        if seg.words.is_empty() && !word.quoted && is_assignment(word.raw) {
            seg.assigns.push(word.raw);
            continue;
        }
        // Reserved words that only group or introduce commands.
        if seg.words.is_empty()
            && !word.quoted
            && matches!(word.raw, "{" | "}" | "!" | "then" | "do" | "else" | "elif" | "if" | "while" | "until" | "fi" | "done" | "esac")
        {
            continue;
        }
        seg.words.push(word);
    }
    if !seg.is_empty() {
        p.segments.push(seg);
    }
    p.open_quote = open_quote;
    p.trailing_space = !open_quote && s.ends_with([' ', '\t']);
    p
}

fn redir_at(b: &[u8], i: usize) -> (RedirKind, usize) {
    let c = b[i];
    let n1 = b.get(i + 1).copied();
    let n2 = b.get(i + 2).copied();
    if c == b'>' {
        match n1 {
            Some(b'>') => (RedirKind::Append, 2),
            Some(b'&') => (RedirKind::Dup, 2),
            Some(b'|') => (RedirKind::Write, 2),
            _ => (RedirKind::Write, 1),
        }
    } else {
        match (n1, n2) {
            (Some(b'<'), Some(b'<')) => (RedirKind::HereStr, 3),
            (Some(b'<'), Some(b'-')) => (RedirKind::HereDoc, 3),
            (Some(b'<'), _) => (RedirKind::HereDoc, 2),
            (Some(b'&'), _) => (RedirKind::Dup, 2),
            (Some(b'>'), _) => (RedirKind::Read, 2),
            _ => (RedirKind::Read, 1),
        }
    }
}

/// Detects the classic `name(){ name|name& };name` fork bomb, with any name
/// and spacing.
pub fn detect_fork_bomb(s: &str) -> bool {
    if !s.contains('(') || !s.contains('|') || !s.contains('&') {
        return false;
    }
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let mut from = 0;
    while let Some(pos) = compact[from..].find("(){") {
        let at = from + pos;
        let name_start = compact[..at]
            .rfind([';', '&', '|', '\n'])
            .map(|k| k + 1)
            .unwrap_or(0);
        let name = compact[name_start..at].trim_start_matches("function");
        if !name.is_empty() {
            let body = &compact[at + 3..];
            let piped = format!("{name}|{name}");
            if body.contains(&piped) && body.contains('&') {
                return true;
            }
        }
        from = at + 3;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<Vec<String>> {
        lex(s)
            .segments
            .iter()
            .map(|sg| sg.words.iter().map(|w| w.value.to_string()).collect())
            .collect()
    }

    #[test]
    fn simple() {
        assert_eq!(words("rm -rf build"), vec![vec!["rm", "-rf", "build"]]);
    }

    #[test]
    fn quotes_and_escapes() {
        assert_eq!(words(r#"echo "a b" 'c d' e\ f"#), vec![vec!["echo", "a b", "c d", "e f"]]);
        assert_eq!(words(r#"echo "x\"y""#), vec![vec!["echo", "x\"y"]]);
    }

    #[test]
    fn pipelines_and_ops() {
        let p = lex("find . -name '*.bak' | xargs rm && echo ok; ls &");
        let ops: Vec<Op> = p.segments.iter().map(|s| s.op).collect();
        assert_eq!(ops, vec![Op::Start, Op::Pipe, Op::And, Op::Seq]);
        assert_eq!(p.segments.len(), 4);
    }

    #[test]
    fn redirects() {
        let p = lex("echo hi > out.txt 2>&1");
        let s = &p.segments[0];
        assert_eq!(s.words.len(), 2);
        assert_eq!(s.redirects[0].kind, RedirKind::Write);
        assert_eq!(s.redirects[0].target, "out.txt");
        assert_eq!(s.redirects[1].kind, RedirKind::Dup);
        let p = lex("cat >>log");
        assert_eq!(p.segments[0].redirects[0].kind, RedirKind::Append);
        assert_eq!(p.segments[0].redirects[0].target, "log");
    }

    #[test]
    fn assignments_and_subs() {
        let p = lex("FOO=1 BAR=2 make $(nproc) `date`");
        assert_eq!(p.segments[0].assigns.len(), 2);
        assert_eq!(p.segments[0].words[0].value, "make");
        assert_eq!(p.subs.as_slice(), &["nproc", "date"]);
        let p = lex("bash <(curl -s x.sh)");
        assert_eq!(p.subs.as_slice(), &["curl -s x.sh"]);
    }

    #[test]
    fn partial_input_never_panics() {
        for s in ["\"", "'", "$(", "echo \"$(rm", "a |", "2>", "\\", "$'", "${", "`", "<(", "é\"ü", "&>"] {
            let _ = lex(s);
        }
        assert!(lex("echo 'abc").open_quote);
    }

    #[test]
    fn fork_bomb() {
        assert!(detect_fork_bomb(":(){ :|:& };:"));
        assert!(detect_fork_bomb("bomb() { bomb | bomb & }; bomb"));
        assert!(!detect_fork_bomb("f() { echo hi; }; f | cat &"));
    }

    /// Cheap always-on fuzzing with shell-heavy alphabets (cargo-fuzz in `fuzz/` goes deeper).
    #[test]
    fn random_inputs_never_panic() {
        let alphabet: Vec<char> = " \t\n|&;()<>$`'\"\\{}=#*~/.-_0123456789abcrmfé日".chars().collect();
        let mut x: u64 = 0x9e3779b97f4a7c15;
        for _ in 0..20_000 {
            let len = (x % 24) as usize;
            let mut s = String::new();
            for _ in 0..len {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                s.push(alphabet[(x % alphabet.len() as u64) as usize]);
            }
            let _ = lex(&s);
            let _ = crate::engine::analyze(&s);
        }
    }

    #[test]
    fn trailing_space() {
        assert!(lex("rm ").trailing_space);
        assert!(!lex("rm").trailing_space);
    }
}
