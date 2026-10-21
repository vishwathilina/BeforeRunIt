use std::collections::HashMap;
use std::sync::RwLock;

pub use crate::schema::Level;
use crate::schema::{CmdDef, RuleDef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// All positional arguments after the matched subcommand.
    Positional,
    First,
    Last,
    None,
    /// Value of a `key=value` argument, e.g. `dd of=/dev/sda`.
    Kv(&'static str),
    /// Value of a flag, e.g. `curl -o file`.
    Flag(&'static str),
    /// Positionals after skipping N, e.g. `chmod 755 file`.
    After(u8),
}

impl Target {
    pub fn parse(s: &str) -> Target {
        let s = s.trim();
        match s {
            "first" => Target::First,
            "last" => Target::Last,
            "none" => Target::None,
            _ => {
                if let Some(k) = s.strip_prefix("kv:") {
                    Target::Kv(leak(k))
                } else if let Some(f) = s.strip_prefix("flag:") {
                    Target::Flag(leak(f))
                } else if let Some(n) = s.strip_prefix("after:") {
                    Target::After(n.parse().unwrap_or(0))
                } else {
                    Target::Positional
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalate {
    None,
    Delete,
    Modify,
    Disk,
}

impl Escalate {
    pub fn parse(s: &str) -> Escalate {
        match s {
            "delete" => Escalate::Delete,
            "modify" => Escalate::Modify,
            "disk" => Escalate::Disk,
            _ => Escalate::None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Verdict {
    pub level: Level,
    pub text: &'static str,
    pub pending: &'static str,
    /// Hard verdicts can never be lowered by the AI review layer.
    pub hard: bool,
}

#[derive(Debug)]
pub struct Rule {
    pub sub: &'static [&'static str],
    pub flags_all: &'static [&'static str],
    pub flags_any: &'static [&'static str],
    pub flags_none: &'static [&'static str],
    pub args_any: &'static [&'static str],
    pub target: Option<Target>,
    pub verdict: Verdict,
}

#[derive(Debug)]
pub struct Cmd {
    pub name: &'static str,
    pub bundle: bool,
    pub takes_value: &'static [&'static str],
    pub long: &'static [(&'static str, &'static str)],
    pub target: Target,
    pub escalate: Escalate,
    pub category: &'static str,
    pub default: Verdict,
    pub rules: &'static [Rule],
}

impl Cmd {
    pub fn has_subs(&self) -> bool {
        self.rules.iter().any(|r| !r.sub.is_empty())
    }

    pub fn canonical_long(&self, name: &str) -> Option<&'static str> {
        self.long.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
    }

    pub fn takes_value(&self, flag: &str) -> bool {
        self.takes_value.contains(&flag)
    }
}

mod builtin {
    use super::*;
    include!(concat!(env!("OUT_DIR"), "/builtin_rules.rs"));
}

pub fn builtin_cmds() -> &'static [Cmd] {
    &builtin::CMDS
}

pub fn builtin_count() -> usize {
    builtin::CMDS.len()
}

static USER: RwLock<Option<HashMap<&'static str, &'static Cmd>>> = RwLock::new(None);

/// Looks a command up, user rules first. Handles `mkfs.ext4` style names by
/// falling back to the part before the first dot.
pub fn lookup(name: &str) -> Option<&'static Cmd> {
    if let Ok(g) = USER.read() {
        if let Some(map) = g.as_ref() {
            if let Some(c) = map.get(name) {
                return Some(c);
            }
        }
    }
    if let Some(&i) = builtin::CMD_INDEX.get(name) {
        return Some(&builtin::CMDS[i as usize]);
    }
    if let Some((base, _)) = name.split_once('.') {
        if !base.is_empty() {
            if let Some(&i) = builtin::CMD_INDEX.get(base) {
                return Some(&builtin::CMDS[i as usize]);
            }
        }
    }
    None
}

/// True if `prefix` could still grow into a known command name.
pub fn is_command_prefix(prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let names = &builtin::CMD_NAMES_SORTED;
    let i = names.partition_point(|n| *n < prefix);
    if names.get(i).is_some_and(|n| n.starts_with(prefix) && *n != prefix) {
        return true;
    }
    if let Ok(g) = USER.read() {
        if let Some(map) = g.as_ref() {
            return map.keys().any(|k| k.starts_with(prefix) && *k != prefix);
        }
    }
    false
}

/// All user-defined plus built-in commands, for the rules browser.
pub fn all_cmds() -> Vec<&'static Cmd> {
    let mut out: Vec<&'static Cmd> = Vec::new();
    if let Ok(g) = USER.read() {
        if let Some(map) = g.as_ref() {
            out.extend(map.values().copied());
        }
    }
    for c in builtin::CMDS.iter() {
        if !out.iter().any(|u| u.name == c.name) {
            out.push(c);
        }
    }
    out.sort_by_key(|c| c.name);
    out
}

/// Installs user rule packs. Replaces any previous user set; the old entries are
/// leaked, which is fine because reloads are rare and tiny.
pub fn install_user_rules(defs: &[CmdDef]) {
    let mut map = HashMap::new();
    for d in defs {
        let cmd: &'static Cmd = Box::leak(Box::new(cmd_from_def(d)));
        map.insert(cmd.name, cmd);
        for a in &d.aliases {
            map.insert(leak(a), cmd);
        }
    }
    if let Ok(mut g) = USER.write() {
        *g = if map.is_empty() { None } else { Some(map) };
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn leak_list(v: &[String]) -> &'static [&'static str] {
    Box::leak(v.iter().map(|s| leak(s)).collect::<Vec<_>>().into_boxed_slice())
}

fn rule_from_def(r: &RuleDef) -> Rule {
    Rule {
        sub: leak_list(&r.sub),
        flags_all: leak_list(&r.flags_all),
        flags_any: leak_list(&r.flags_any),
        flags_none: leak_list(&r.flags_none),
        args_any: leak_list(&r.args_any),
        target: if r.target.is_empty() { None } else { Some(Target::parse(&r.target)) },
        verdict: Verdict {
            level: r.risk,
            text: leak(&r.text),
            pending: leak(&r.pending),
            hard: r.hard,
        },
    }
}

pub fn cmd_from_def(d: &CmdDef) -> Cmd {
    Cmd {
        name: leak(&d.name),
        bundle: d.bundle,
        takes_value: leak_list(&d.takes_value),
        long: Box::leak(
            d.long
                .iter()
                .map(|(k, v)| (leak(k), leak(v)))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        ),
        target: Target::parse(&d.target),
        escalate: Escalate::parse(&d.escalate),
        category: leak(&d.category),
        default: Verdict {
            level: d.risk,
            text: leak(&d.text),
            pending: leak(&d.pending),
            hard: d.hard,
        },
        rules: Box::leak(d.rules.iter().map(rule_from_def).collect::<Vec<_>>().into_boxed_slice()),
    }
}

/// Minimal glob: `*` matches any run, `?` one char.
pub fn glob(pat: &str, s: &str) -> bool {
    let p = pat.as_bytes();
    let t = s.as_bytes();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = pi;
            pi += 1;
            mark = ti;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_works() {
        assert!(glob("of=/dev/sd*", "of=/dev/sda"));
        assert!(glob("*", ""));
        assert!(glob("7?7", "777"));
        assert!(!glob("/dev/sd*", "/dev/null"));
    }
}
