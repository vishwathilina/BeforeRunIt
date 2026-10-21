//! Tier 1: the instant local slot engine.

use smallvec::SmallVec;

use crate::lexer::{self, Op, RedirKind, Segment};
use crate::rules::{self, Cmd, Escalate, Level, Target, Verdict};

const MAX_DEPTH: u8 = 3;
const MAX_TARGET_CHARS: usize = 22;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Analysis {
    pub level: Level,
    /// Plain-English summary, at most ~10 words.
    pub text: String,
    /// Byte range of the filled `{target}` slot inside `text`, shown in bold.
    pub target_span: Option<(usize, usize)>,
    /// Hard verdicts cannot be lowered by the AI.
    pub hard: bool,
    /// Runs as root (sudo, doas, ...).
    pub root: bool,
    /// Pipelines of 3+ commands, substitutions, `find -exec`: worth an AI look.
    pub complex: bool,
    /// Primary command name, if known.
    pub cmd: Option<&'static str>,
    /// Raw target paths (max 4), used for AI context such as "does it exist".
    pub targets: Vec<String>,
    /// Set when only the trailing target word may change the result.
    pub reuse: Option<Box<Reuse>>,
}

/// What `Engine` needs to refill the `{target}` slot without re-matching.
#[derive(Debug, Clone, PartialEq)]
pub struct Reuse {
    /// Everything before the last word, including the separating whitespace.
    prefix: String,
    template: &'static str,
    pending: &'static str,
    /// Targets before the last word.
    earlier: Vec<String>,
    pipe_items: bool,
}

impl Analysis {
    fn idle(text: &str) -> Self {
        Analysis { level: Level::Idle, text: text.to_string(), ..Default::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.level == Level::Idle && self.text.is_empty()
    }

    /// Tier 1 has nothing useful to say without help.
    pub fn needs_ai(&self) -> bool {
        self.level == Level::Unknown || self.complex
    }
}

pub const TEXT_TYPING: &str = "Typing command...";
pub const TEXT_UNKNOWN: &str = "Unknown command. Not in the rule book.";

/// Analyzes one full command line.
pub fn analyze(line: &str) -> Analysis {
    analyze_depth(line, 0)
}

struct SegOut {
    level: Level,
    text: String,
    span: Option<(usize, usize)>,
    hard: bool,
    root: bool,
    complex: bool,
    cmd: Option<&'static str>,
    targets: Vec<String>,
    downloader: bool,
    shell_sink: bool,
    shell_like: bool,
    reuse: Option<(&'static str, &'static str, bool)>,
}

impl SegOut {
    fn simple(level: Level, text: &str) -> Self {
        SegOut {
            level,
            text: text.to_string(),
            span: None,
            hard: false,
            root: false,
            complex: false,
            cmd: None,
            targets: Vec::new(),
            downloader: false,
            shell_sink: false,
            shell_like: false,
            reuse: None,
        }
    }
}

fn analyze_depth(line: &str, depth: u8) -> Analysis {
    if line.trim().is_empty() {
        return Analysis::default();
    }
    let p = lexer::lex(line);
    if p.fork_bomb {
        return Analysis {
            level: Level::Danger,
            text: "Fork bomb. Freezes your computer instantly.".into(),
            hard: true,
            ..Default::default()
        };
    }
    if p.segments.is_empty() {
        // Only reserved words so far (`fi`, `do`): still typing.
        return if p.trailing_space { Analysis::default() } else { Analysis::idle(TEXT_TYPING) };
    }

    // Still typing the very first command name.
    if p.segments.len() == 1 && !p.trailing_space && p.subs.is_empty() {
        let s = &p.segments[0];
        if s.words.len() == 1 && s.redirects.is_empty() && s.assigns.is_empty() {
            let w = s.words[0].as_str();
            let base = basename(w);
            if rules::lookup(base).is_none() && rules::is_command_prefix(base) && !w.contains('/') {
                return Analysis::idle(TEXT_TYPING);
            }
        }
    }

    let last = p.segments.len() - 1;
    let mut outs: SmallVec<[SegOut; 4]> = SmallVec::new();
    for (i, seg) in p.segments.iter().enumerate() {
        let incomplete = i == last && !p.trailing_space;
        outs.push(analyze_segment(seg, depth, incomplete));
    }

    // Pipeline rule: downloader piped straight into an interpreter.
    for i in 0..outs.len().saturating_sub(1) {
        if p.segments[i + 1].op == Op::Pipe && outs[i].downloader && outs[i + 1].shell_sink {
            let o = &mut outs[i + 1];
            o.level = Level::Danger;
            o.hard = true;
            o.text = "Runs an internet script without checking it.".into();
            o.span = None;
        }
    }

    let mut complex = p.segments.len() >= 3 || outs.iter().any(|o| o.complex);

    // Substitutions: `bash <(curl ..)`, `sh -c "$(wget ..)"`, `echo $(rm -rf /)`.
    if depth < MAX_DEPTH {
        for sub in &p.subs {
            complex = true;
            let inner = analyze_depth(sub, depth + 1);
            let sub_parsed = lexer::lex(sub);
            let inner_downloads = sub_parsed.segments.first().is_some_and(|s| first_cmd_is(s, is_downloader));
            if inner_downloads && outs.iter().any(|o| o.shell_like) {
                let mut o = SegOut::simple(Level::Danger, "Runs an internet script without checking it.");
                o.hard = true;
                outs.push(o);
            } else if inner.level > Level::Known {
                let mut o = SegOut::simple(inner.level, &inner.text);
                o.span = inner.target_span;
                o.hard = inner.hard;
                o.root = inner.root;
                o.cmd = inner.cmd;
                o.targets = inner.targets;
                outs.push(o);
            }
        }
    }

    // Highest severity wins. On risky ties the later segment explains the line
    // (`find | xargs rm`); on harmless ties the first one does (`ls | grep`).
    let mut best = 0;
    for (i, o) in outs.iter().enumerate() {
        let b = outs[best].level;
        if o.level > b || (o.level == b && o.level > Level::Unknown) {
            best = i;
        }
    }
    let any_root = outs.iter().any(|o| o.root);
    let single = outs.len() == 1 && p.segments.len() == 1 && p.subs.is_empty() && !p.trailing_space && depth == 0;
    let b = outs.swap_remove(best);
    let reuse = match b.reuse {
        Some((template, pending, pipe_items)) if single && !b.targets.is_empty() && b.targets.len() < 4 => {
            let last_word = line.rsplit([' ', '\t']).next().unwrap_or("");
            if b.targets.last().is_some_and(|t| t == last_word) && is_plain_word(last_word) {
                Some(Box::new(Reuse {
                    prefix: line[..line.len() - last_word.len()].to_string(),
                    template,
                    pending,
                    earlier: b.targets[..b.targets.len() - 1].to_vec(),
                    pipe_items,
                }))
            } else {
                None
            }
        }
        _ => None,
    };
    Analysis {
        level: b.level,
        text: b.text,
        target_span: b.span,
        hard: b.hard,
        root: any_root,
        complex,
        cmd: b.cmd,
        targets: b.targets,
        reuse,
    }
}

/// A word that cannot change how the line lexes or which rule matches.
fn is_plain_word(w: &str) -> bool {
    !w.is_empty()
        && !w.starts_with('-')
        && !w.starts_with('~')
        && w.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'+' | b',' | b'@' | b'%') || b >= 0x80)
        && w != "."
        && w != ".."
}

fn basename(w: &str) -> &str {
    w.rsplit('/').next().unwrap_or(w)
}

fn is_downloader(name: &str) -> bool {
    matches!(name, "curl" | "wget" | "fetch" | "aria2c" | "http" | "https" | "xh" | "lynx" | "links")
}

fn is_interpreter(name: &str) -> bool {
    matches!(
        name,
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "ash" | "python" | "python3" | "perl" | "ruby" | "node" | "php" | "lua"
    )
}

fn is_shell_like(name: &str) -> bool {
    is_interpreter(name) || matches!(name, "eval" | "source" | "." | "exec")
}

/// Tests the command name after unwrapping wrappers, without analysis.
fn first_cmd_is(seg: &Segment, pred: fn(&str) -> bool) -> bool {
    let words: SmallVec<[&str; 8]> = seg.words.iter().map(|w| w.as_str()).collect();
    let u = unwrap(&words);
    words.get(u.idx).is_some_and(|w| pred(basename(w)))
}

struct Unwrapped {
    idx: usize,
    root: bool,
    from_pipe_items: bool,
    /// The innermost wrapper, used when nothing follows it (`sudo `).
    wrapper: Option<&'static str>,
    /// `sudo -i` / `sudo -s` / `sudo su` without a command.
    root_shell: bool,
}

fn wrapper_flags(name: &str) -> Option<(&'static str, &'static [&'static str])> {
    // (canonical name, flags that take a value)
    Some(match name {
        "sudo" => ("sudo", &["u", "g", "C", "D", "h", "p", "r", "t", "U", "T", "user", "group", "prompt", "chdir"]),
        "doas" => ("doas", &["u", "C"]),
        "run0" => ("run0", &["u", "g", "unit", "description", "property", "setenv", "nice", "chdir"]),
        "pkexec" => ("pkexec", &["user"]),
        "env" => ("env", &["u", "C", "S", "unset", "chdir", "split-string"]),
        "nice" => ("nice", &["n", "adjustment"]),
        "ionice" => ("ionice", &["c", "n", "p", "P", "u", "class", "classdata"]),
        "nohup" => ("nohup", &[]),
        "time" => ("time", &["f", "o", "format", "output"]),
        "timeout" => ("timeout", &["s", "k", "signal", "kill-after"]),
        "watch" => ("watch", &["n", "interval", "q", "equexit"]),
        "stdbuf" => ("stdbuf", &["i", "o", "e", "input", "output", "error"]),
        "exec" => ("exec", &["a"]),
        "command" => ("command", &[]),
        "builtin" => ("builtin", &[]),
        "xargs" => ("xargs", &["I", "n", "P", "d", "L", "s", "a", "E", "e", "max-args", "max-procs", "delimiter", "arg-file", "replace"]),
        "chrt" => ("chrt", &[]),
        "unbuffer" => ("unbuffer", &[]),
        "caffeinate" => ("caffeinate", &["w", "t"]),
        "systemd-run" => ("systemd-run", &["u", "unit", "p", "property", "E", "setenv", "description"]),
        "flatpak-spawn" => ("flatpak-spawn", &[]),
        "proxychains" | "proxychains4" => ("proxychains", &["f"]),
        "torsocks" => ("torsocks", &[]),
        "firejail" => ("firejail", &[]),
        _ => return None,
    })
}

fn unwrap(words: &[&str]) -> Unwrapped {
    let mut u = Unwrapped { idx: 0, root: false, from_pipe_items: false, wrapper: None, root_shell: false };
    let mut guard = 0;
    while u.idx < words.len() && guard < 8 {
        guard += 1;
        let name = basename(words[u.idx]);
        let Some((canon, value_flags)) = wrapper_flags(name) else { break };
        // `command -v x` and `time` on its own are lookups, not wrappers.
        if canon == "command" && words.get(u.idx + 1).is_some_and(|w| *w == "-v" || *w == "-V") {
            break;
        }
        u.wrapper = Some(canon);
        if matches!(canon, "sudo" | "doas" | "run0" | "pkexec") {
            u.root = true;
        }
        if canon == "xargs" {
            u.from_pipe_items = true;
        }
        u.idx += 1;
        // Skip the wrapper's own flags and assignments.
        while u.idx < words.len() {
            let w = words[u.idx];
            if w == "--" {
                u.idx += 1;
                break;
            }
            if canon == "env" && w.contains('=') && !w.starts_with('-') {
                u.idx += 1;
                continue;
            }
            if !w.starts_with('-') || w == "-" {
                break;
            }
            if canon == "sudo" && matches!(w, "-i" | "-s" | "--login" | "--shell") {
                u.root_shell = true;
            }
            let (name, attached) = if let Some(l) = w.strip_prefix("--") {
                match l.split_once('=') {
                    Some((n, _)) => (n, true),
                    None => (l, false),
                }
            } else {
                let body = &w[1..];
                let first = &body[..body.chars().next().map(char::len_utf8).unwrap_or(0)];
                (first, body.len() > first.len())
            };
            u.idx += 1;
            if value_flags.contains(&name) && !attached {
                u.idx += 1;
            }
        }
        // `timeout 5s cmd`, `chrt 10 cmd`: one leading positional.
        if matches!(canon, "timeout" | "chrt") && u.idx < words.len() {
            u.idx += 1;
        }
    }
    if u.root && words.get(u.idx).is_some_and(|w| matches!(basename(w), "su" | "bash" | "sh" | "zsh" | "fish")) && words.len() == u.idx + 1 {
        u.root_shell = true;
    }
    u
}

/// Arguments after the command name, with flags normalized.
struct Args<'a> {
    flags: SmallVec<[&'a str; 8]>,
    flag_values: SmallVec<[(&'a str, &'a str); 2]>,
    pos: SmallVec<[&'a str; 8]>,
}

fn parse_args<'a>(cmd: &Cmd, words: &[&'a str]) -> Args<'a> {
    let mut a = Args { flags: SmallVec::new(), flag_values: SmallVec::new(), pos: SmallVec::new() };
    let mut i = 0;
    let mut only_pos = false;
    while i < words.len() {
        let w = words[i];
        i += 1;
        if only_pos || w == "-" || !w.starts_with('-') || w.len() == 1 {
            a.pos.push(w);
            continue;
        }
        if w == "--" {
            only_pos = true;
            continue;
        }
        if let Some(long) = w.strip_prefix("--") {
            let (name, val) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (long, None),
            };
            let canon = cmd.canonical_long(name).unwrap_or(name);
            a.flags.push(canon);
            if let Some(v) = val {
                a.flag_values.push((canon, v));
            } else if (cmd.takes_value(name) || cmd.takes_value(canon)) && i < words.len() {
                a.flag_values.push((canon, words[i]));
                i += 1;
            }
            continue;
        }
        let body = &w[1..];
        if !cmd.bundle {
            let canon = cmd.canonical_long(body).unwrap_or(body);
            a.flags.push(canon);
            if cmd.takes_value(body) && i < words.len() {
                a.flag_values.push((canon, words[i]));
                i += 1;
            }
            continue;
        }
        // `-9` style numeric flags stay whole.
        if body.bytes().all(|b| b.is_ascii_digit()) {
            a.flags.push(body);
            continue;
        }
        let mut chars = body.char_indices();
        while let Some((ci, ch)) = chars.next() {
            let f = &body[ci..ci + ch.len_utf8()];
            let canon = cmd.canonical_long(f).unwrap_or(f);
            a.flags.push(canon);
            if cmd.takes_value(f) {
                let rest = &body[ci + ch.len_utf8()..];
                if !rest.is_empty() {
                    a.flag_values.push((canon, rest));
                } else if i < words.len() {
                    a.flag_values.push((canon, words[i]));
                    i += 1;
                }
                break;
            }
        }
    }
    a
}

/// Number of positionals consumed by a matching `sub` entry, if any.
fn match_sub(subs: &[&str], pos: &[&str]) -> Option<usize> {
    for s in subs {
        let n = s.split(' ').count();
        if pos.len() >= n && s.split(' ').zip(pos.iter()).all(|(a, b)| a == *b) {
            return Some(n);
        }
    }
    None
}

fn rule_matches(r: &rules::Rule, a: &Args, pos_after_sub: &[&str]) -> bool {
    let has = |f: &str| a.flags.contains(&f);
    if !r.flags_all.iter().all(|f| has(f)) {
        return false;
    }
    if !r.flags_any.is_empty() && !r.flags_any.iter().any(|f| has(f)) {
        return false;
    }
    if r.flags_none.iter().any(|f| has(f)) {
        return false;
    }
    if !r.args_any.is_empty()
        && !r
            .args_any
            .iter()
            .any(|g| pos_after_sub.iter().any(|p| rules::glob(g, p)) || a.flag_values.iter().any(|(_, v)| rules::glob(g, v)))
    {
        return false;
    }
    true
}

fn select_targets<'a>(t: Target, a: &Args<'a>, pos: &[&'a str]) -> SmallVec<[&'a str; 4]> {
    let mut out = SmallVec::new();
    match t {
        Target::Positional => out.extend(pos.iter().copied().filter(|p| !p.is_empty())),
        Target::First => out.extend(pos.first().copied()),
        Target::Last => out.extend(pos.last().copied()),
        Target::None => {}
        Target::After(n) => out.extend(pos.iter().skip(n as usize).copied()),
        Target::Kv(k) => {
            for p in pos {
                if let Some(v) = p.strip_prefix(k).and_then(|r| r.strip_prefix('=')) {
                    out.push(v);
                }
            }
        }
        Target::Flag(f) => {
            for (n, v) in &a.flag_values {
                if *n == f {
                    out.push(*v);
                }
            }
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Critical {
    Root,
    Home,
    Here,
    System,
    /// A file or folder below a system directory, e.g. `/etc/hosts`.
    SystemFile,
    Disk,
}

const SYSTEM_PREFIXES: [&str; 8] = ["/etc/", "/boot/", "/usr/", "/bin/", "/sbin/", "/lib/", "/lib64/", "/efi/"];

fn critical(t: &str) -> Option<Critical> {
    if t.is_empty() {
        return None;
    }
    let t = t.trim_end_matches('/');
    let t = t.strip_suffix("/*").unwrap_or(t);
    let t = t.trim_end_matches('/');
    if t.is_empty() || t == "/." || t == "/.." {
        return Some(Critical::Root);
    }
    if matches!(t, "~" | "$HOME" | "${HOME}" | "/home" | "~/." | "\"$HOME\"") {
        return Some(Critical::Home);
    }
    if matches!(t, "*" | "." | ".." | "./*" | ".*" | "./") {
        return Some(Critical::Here);
    }
    for d in ["/dev/sd", "/dev/nvme", "/dev/vd", "/dev/hd", "/dev/mmcblk", "/dev/xvd", "/dev/disk/"] {
        if t.starts_with(d) && t.len() > d.len() {
            return Some(Critical::Disk);
        }
    }
    const SYS: [&str; 16] = [
        "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/var", "/opt", "/root", "/sys", "/proc", "/dev", "/usr/bin", "/usr/lib", "/efi",
    ];
    if SYS.contains(&t) {
        return Some(Critical::System);
    }
    if SYSTEM_PREFIXES.iter().any(|p| t.starts_with(p)) && !t.starts_with("/usr/local/") {
        return Some(Critical::SystemFile);
    }
    None
}

fn escalation_text(e: Escalate, c: Critical) -> (&'static str, bool) {
    match (e, c) {
        (_, Critical::Disk) => ("Overwrites disk {target}. All data lost.", true),
        (Escalate::Delete, Critical::Root) => ("Deletes your entire system. Unrecoverable.", true),
        (Escalate::Delete, Critical::Home) => ("Deletes your entire home folder.", true),
        (Escalate::Delete, Critical::Here) => ("Deletes everything in this folder.", false),
        (Escalate::Delete, Critical::System) => ("Deletes system folder {target}. Breaks the OS.", true),
        (Escalate::Delete, Critical::SystemFile) => ("Deletes system files in {target}.", false),
        (_, Critical::SystemFile) => ("Changes system file {target}.", false),
        (_, Critical::Root) => ("Changes your entire system. Can break booting.", true),
        (_, Critical::Home) => ("Changes your entire home folder.", true),
        (_, Critical::Here) => ("Changes everything in this folder.", false),
        (_, Critical::System) => ("Changes system folder {target}. Can break the OS.", true),
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

fn format_one(t: &str) -> String {
    if t == "{}" {
        return "matching files".into();
    }
    format!("\"{}\"", truncate_chars(t, MAX_TARGET_CHARS))
}

fn format_targets(ts: &[&str]) -> String {
    match ts.len() {
        0 => String::new(),
        1 => format_one(ts[0]),
        2 => format!("{} and {}", format_one(ts[0]), format_one(ts[1])),
        n => format!("{} and {} more", format_one(ts[0]), n - 1),
    }
}

struct Fill<'a> {
    targets: &'a [&'a str],
    pos: &'a [&'a str],
    sub: &'a str,
    pipe_items: bool,
}

/// Fills `{target}`, `{arg1}`, `{arg2}` and `{sub}`. Falls back to `pending`
/// when a needed slot is still empty.
fn fill(template: &str, pending: &str, f: &Fill) -> (String, Option<(usize, usize)>) {
    let needs_target = template.contains("{target}");
    let needs_arg = |n: usize| template.contains(&format!("{{arg{n}}}")) && f.pos.len() < n;
    let needs_sub = template.contains("{sub}") && f.sub.is_empty();
    if (needs_target && f.targets.is_empty() && !f.pipe_items) || needs_arg(1) || needs_arg(2) || needs_sub {
        if !pending.is_empty() {
            return (pending.to_string(), None);
        }
    }
    let mut out = String::with_capacity(template.len() + 24);
    let mut span = None;
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let key = &rest[open + 1..open + close];
        let start = out.len();
        match key {
            "target" => {
                if f.targets.is_empty() {
                    out.push_str(if f.pipe_items { "the piped items" } else { "files" });
                } else {
                    out.push_str(&format_targets(f.targets));
                }
                span = Some((start, out.len()));
            }
            "arg1" | "arg2" => {
                let n = if key == "arg1" { 0 } else { 1 };
                out.push_str(&truncate_chars(f.pos.get(n).copied().unwrap_or("?"), MAX_TARGET_CHARS));
            }
            "sub" => out.push_str(f.sub),
            other => {
                out.push('{');
                out.push_str(other);
                out.push('}');
            }
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    (out, span)
}

fn is_quiet_sink(t: &str) -> bool {
    matches!(t, "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty" | "/dev/zero" | "-")
}

fn analyze_segment(seg: &Segment, depth: u8, incomplete: bool) -> SegOut {
    let mut words: SmallVec<[&str; 8]> = seg.words.iter().map(|w| w.as_str()).collect();
    // A half-typed `-` or `--` is a flag in progress, not a target.
    if incomplete && words.len() > 1 && matches!(words.last(), Some(&"-") | Some(&"--")) && seg.words.last().is_some_and(|w| !w.quoted) {
        words.pop();
    }

    // Redirect-only or assignment-only segments.
    if words.is_empty() {
        if let Some(r) = seg.redirects.iter().find(|r| matches!(r.kind, RedirKind::Write)) {
            let t = [r.target.as_ref()];
            let (text, span) = fill("Empties {target}.", "Empties a file.", &Fill { targets: &t, pos: &[], sub: "", pipe_items: false });
            let mut o = SegOut::simple(Level::Caution, &text);
            o.span = span;
            return o;
        }
        return SegOut::simple(Level::Known, "Sets a shell variable.");
    }

    let u = unwrap(&words);
    if u.idx >= words.len() {
        // A wrapper with nothing after it yet.
        let name = u.wrapper.unwrap_or("sudo");
        if u.root_shell {
            let mut o = SegOut::simple(Level::Caution, "Opens a root shell. Every command runs as admin.");
            o.root = true;
            return o;
        }
        let mut o = match rules::lookup(name) {
            Some(c) => SegOut::simple(c.default.level, if c.default.pending.is_empty() { c.default.text } else { c.default.pending }),
            None => SegOut::simple(Level::Known, TEXT_TYPING),
        };
        o.root = u.root;
        return o;
    }
    if u.root_shell {
        let mut o = SegOut::simple(Level::Caution, "Opens a root shell. Every command runs as admin.");
        o.root = true;
        // `curl .. | sudo bash` feeds the download into that root shell.
        o.shell_sink = true;
        o.shell_like = true;
        return o;
    }

    let raw_name = words[u.idx];
    let name = basename(raw_name);
    let rest = &words[u.idx + 1..];

    // Still typing this segment's command name (`ls | gr`, `sudo ap`).
    if incomplete && rest.is_empty() && !raw_name.contains('/') && rules::lookup(name).is_none() && rules::is_command_prefix(name) {
        let mut o = SegOut::simple(Level::Idle, TEXT_TYPING);
        o.root = u.root;
        return o;
    }
    let in_bin_dir = |p: &str| ["/bin/", "/sbin/", "/usr/bin/", "/usr/sbin/", "/usr/local/bin/", "/usr/local/sbin/"].iter().any(|d| p.starts_with(d));
    let is_path_script = raw_name.contains('/') && !in_bin_dir(raw_name);

    let Some(cmd) = (if is_path_script { None } else { rules::lookup(name) }) else {
        let mut o = if raw_name.contains('/') {
            let t = [raw_name];
            let (text, span) =
                fill("Runs script {target}. It can do anything.", "", &Fill { targets: &t, pos: &[], sub: "", pipe_items: false });
            let mut o = SegOut::simple(Level::Caution, &text);
            o.span = span;
            o
        } else {
            SegOut::simple(Level::Unknown, TEXT_UNKNOWN)
        };
        o.root = u.root;
        if u.root && o.level == Level::Unknown {
            o.level = Level::Caution;
            o.text = "Runs an unknown command as root.".into();
        }
        o.shell_like = is_shell_like(name);
        return o;
    };

    let args = parse_args(cmd, rest);

    // Pick the first matching rule.
    let mut verdict: Verdict = cmd.default;
    let mut sub_len = 0usize;
    let mut target_sel = cmd.target;
    let mut matched_sub = "";
    for r in cmd.rules {
        let n = if r.sub.is_empty() {
            0
        } else {
            match match_sub(r.sub, &args.pos) {
                Some(n) => n,
                None => continue,
            }
        };
        if rule_matches(r, &args, &args.pos[n..]) {
            verdict = r.verdict;
            sub_len = n;
            if let Some(t) = r.target {
                target_sel = t;
            }
            if n > 0 {
                matched_sub = r.sub.iter().find(|s| s.split(' ').count() == n).copied().unwrap_or("");
            }
            break;
        }
    }
    if sub_len == 0 && cmd.has_subs() && !args.pos.is_empty() && verdict.text == cmd.default.text {
        // A subcommand still being typed (`git pu`) shows the pending text.
        let typing_sub = incomplete && args.pos.len() == 1 && rest.last() == Some(&args.pos[0]);
        if !typing_sub {
            matched_sub = args.pos[0];
        }
        sub_len = 1;
    }
    let pos = &args.pos[sub_len.min(args.pos.len())..];
    let targets = select_targets(target_sel, &args, pos);

    let mut level = verdict.level;
    let mut hard = verdict.hard;
    let mut template = verdict.text;
    let mut pending = verdict.pending;

    // Critical-path escalation.
    let mut crit_target: SmallVec<[&str; 1]> = SmallVec::new();
    if cmd.escalate != Escalate::None && level >= Level::Caution {
        let mut worst: Option<(Critical, &str)> = None;
        for t in &targets {
            if let Some(c) = critical(t) {
                let rank = |c: Critical| match c {
                    Critical::SystemFile => 0,
                    Critical::Here => 0,
                    Critical::System => 1,
                    Critical::Home => 2,
                    Critical::Disk => 3,
                    Critical::Root => 4,
                };
                if worst.is_none_or(|(w, _)| rank(c) > rank(w)) {
                    worst = Some((c, t));
                }
            }
        }
        if let Some((c, t)) = worst {
            let (txt, h) = escalation_text(cmd.escalate, c);
            level = Level::Danger;
            hard = hard || h;
            template = txt;
            pending = "";
            crit_target.push(t);
        }
    }

    let fill_targets: &[&str] = if crit_target.is_empty() { &targets } else { &crit_target };
    let (mut text, mut span) =
        fill(template, pending, &Fill { targets: fill_targets, pos, sub: matched_sub, pipe_items: u.from_pipe_items });
    let mut complex = false;

    // `find ... -exec rm {} \;` analyzes the inner command too.
    if cmd.name == "find" && depth < MAX_DEPTH {
        if let Some(i) = rest.iter().position(|w| matches!(*w, "-exec" | "-execdir" | "-ok" | "-okdir")) {
            complex = true;
            let inner: Vec<&str> = rest[i + 1..].iter().take_while(|w| !matches!(**w, ";" | "\\;" | "+")).copied().collect();
            if !inner.is_empty() {
                let inner_line = inner.join(" ");
                let ia = analyze_depth(&inner_line, depth + 1);
                if ia.level > level {
                    level = ia.level;
                    hard = hard || ia.hard;
                    text = ia.text;
                    span = ia.target_span;
                }
            }
        }
    }

    // Output redirections.
    for r in &seg.redirects {
        if !matches!(r.kind, RedirKind::Write | RedirKind::Append) || is_quiet_sink(&r.target) {
            continue;
        }
        let t = [r.target.as_ref()];
        let f = Fill { targets: &t, pos: &[], sub: "", pipe_items: false };
        let (lvl, tpl, h) = match critical(&r.target) {
            Some(Critical::Disk) => (Level::Danger, "Overwrites disk {target}. All data lost.", true),
            Some(Critical::SystemFile) => (Level::Danger, "Overwrites system file {target}.", false),
            _ if r.kind == RedirKind::Write => (Level::Caution, "Overwrites {target} with new output.", false),
            _ => (Level::Known, "Appends output to {target}.", false),
        };
        if lvl > level {
            level = lvl;
            hard = hard || h;
            let (t2, s2) = fill(tpl, "", &f);
            text = t2;
            span = s2;
        }
    }

    // Running as root raises anything that changes state.
    if u.root && level == Level::Known && !matches!(cmd.category, "read" | "info") {
        level = Level::Caution;
    }

    let interp_reads_stdin = is_interpreter(cmd.name) && pos.is_empty() && !args.flags.contains(&"c");
    let reusable = cmd.escalate == Escalate::None
        && !complex
        && seg.redirects.is_empty()
        && text_is_template(&text, template)
        && !matches!(target_sel, Target::Kv(_) | Target::Flag(_) | Target::None)
        && cmd.rules.iter().all(|r| r.args_any.is_empty())
        && !template.contains("{arg")
        && !template.contains("{sub}");
    SegOut {
        reuse: reusable.then_some((template, pending, u.from_pipe_items)),
        level,
        text,
        span,
        hard,
        root: u.root,
        complex,
        cmd: Some(cmd.name),
        targets: targets.iter().take(4).map(|s| s.to_string()).collect(),
        downloader: is_downloader(cmd.name),
        shell_sink: interp_reads_stdin || (is_interpreter(cmd.name) && pos.first().is_some_and(|p| *p == "-" || *p == "-s")),
        shell_like: is_shell_like(cmd.name),
    }
}

/// The summary came from the template itself (not from pending text or an
/// inner `find -exec` analysis), so refilling the slot reproduces it.
fn text_is_template(text: &str, template: &str) -> bool {
    match template.split_once("{target}") {
        Some((a, b)) => text.starts_with(a) && text.ends_with(b),
        None => false,
    }
}

/// Keystroke-to-keystroke cache. Identical lines are free; when only the
/// trailing target word changed, the `{target}` slot is refilled without
/// lexing or rule matching.
#[derive(Default)]
pub struct Engine {
    last_line: String,
    last: Analysis,
    has_last: bool,
    /// Counters for `beforerunit bench`.
    pub hits_same: u64,
    pub hits_slot: u64,
    pub misses: u64,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn analyze(&mut self, line: &str) -> &Analysis {
        if self.has_last && self.last_line == line {
            self.hits_same += 1;
            return &self.last;
        }
        if self.has_last && self.refill(line) {
            self.hits_slot += 1;
        } else {
            self.misses += 1;
            self.last = analyze(line);
        }
        self.last_line.clear();
        self.last_line.push_str(line);
        self.has_last = true;
        &self.last
    }

    fn refill(&mut self, line: &str) -> bool {
        let Some(r) = self.last.reuse.as_deref() else { return false };
        if line.len() <= r.prefix.len() || !line.starts_with(r.prefix.as_str()) {
            return false;
        }
        let w = &line[r.prefix.len()..];
        if !is_plain_word(w) {
            return false;
        }
        let mut ts: SmallVec<[&str; 4]> = r.earlier.iter().map(String::as_str).collect();
        ts.push(w);
        let (text, span) = fill(r.template, r.pending, &Fill { targets: &ts, pos: &[], sub: "", pipe_items: r.pipe_items });
        let targets: Vec<String> = ts.iter().map(|s| s.to_string()).collect();
        self.last.text = text;
        self.last.target_span = span;
        self.last.targets = targets;
        true
    }

    pub fn reset(&mut self) {
        self.has_last = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> (Level, String) {
        let r = analyze(s);
        (r.level, r.text)
    }

    #[test]
    fn rm_progression() {
        assert_eq!(a("r").0, Level::Idle);
        assert_eq!(a("rm").0, Level::Caution);
        assert_eq!(a("rm -r").0, Level::Danger);
        let (l, t) = a("rm -rf build");
        assert_eq!(l, Level::Danger);
        assert!(t.contains("\"build\""), "{t}");
    }

    #[test]
    fn flag_normalization() {
        for s in ["rm -rf x", "rm -fr x", "rm -r -f x", "rm --recursive --force x"] {
            assert_eq!(analyze(s).text, analyze("rm -rf x").text, "{s}");
        }
    }

    #[test]
    fn critical_targets() {
        let r = analyze("sudo rm -rf /");
        assert_eq!(r.level, Level::Danger);
        assert!(r.hard);
        assert!(r.root);
        assert!(r.text.contains("entire system"));
        assert!(analyze("rm -rf ~").text.contains("home"));
        assert!(analyze("chmod -R 777 /").hard);
    }

    #[test]
    fn pipelines() {
        let r = analyze("curl -fsSL https://x.sh | sh");
        assert_eq!(r.level, Level::Danger);
        assert!(r.hard);
        let r = analyze("bash <(curl -s https://x.sh)");
        assert_eq!(r.level, Level::Danger);
        let r = analyze("find . -name '*.bak' | xargs rm");
        assert!(r.level >= Level::Caution);
        assert!(r.text.contains("piped"), "{}", r.text);
    }

    #[test]
    fn unknown_and_scripts() {
        assert_eq!(analyze("frobnicate --all").level, Level::Unknown);
        assert_eq!(analyze("./deploy.sh").level, Level::Caution);
        assert_eq!(analyze("ls -la").level, Level::Known);
        assert_eq!(analyze("ls | frobnicate").level, Level::Unknown);
    }

    #[test]
    fn redirects() {
        assert_eq!(analyze("echo hi > notes.txt").level, Level::Caution);
        assert_eq!(analyze("echo hi > /dev/null").level, Level::Known);
        assert!(analyze("cat image.iso > /dev/sda").hard);
    }

    #[test]
    fn fork_bomb() {
        assert!(analyze(":(){ :|:& };:").hard);
    }

    #[test]
    fn engine_cache() {
        let mut e = Engine::new();
        let t1 = e.analyze("ls").text.clone();
        let t2 = e.analyze("ls").text.clone();
        assert_eq!(t1, t2);
    }

    #[test]
    fn slot_reuse_matches_full_analysis() {
        let lines = [
            "rm -rf build_output_dir",
            "cp notes.txt backup.txt",
            "cat /etc/hosts",
            "git add src/main.rs",
            "mkdir -p a/b/c",
            "sudo apt install neovim",
            "chmod 755 script.sh",
            "rm a b c d e f",
            "ls ~/Downloads",
            "find . -name x -exec rm {} ;",
            "echo hi > out.txt",
            "tar -xzf archive.tar.gz",
        ];
        let mut e = Engine::new();
        for l in lines {
            for i in 1..=l.len() {
                if !l.is_char_boundary(i) {
                    continue;
                }
                let part = &l[..i];
                let got = e.analyze(part).clone();
                let want = analyze(part);
                assert_eq!((got.level, &got.text, got.target_span, &got.targets), (want.level, &want.text, want.target_span, &want.targets), "{part:?}");
            }
        }
        assert!(e.hits_slot > 0, "slot reuse never triggered");
    }
}
