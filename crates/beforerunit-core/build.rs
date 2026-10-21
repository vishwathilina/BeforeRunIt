//! Compiles the TOML rule packs into static Rust data plus a `phf` command map,
//! so the keystroke path never parses anything at runtime. Rule mistakes
//! (unknown target selectors, summaries over 10 words) fail the build.

#[allow(dead_code)]
#[path = "src/schema.rs"]
mod schema;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

use schema::{CmdDef, Level, PackDef};

const MAX_WORDS: usize = 10;

fn rules_dir() -> PathBuf {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let in_crate = manifest.join("rules");
    if in_crate.is_dir() {
        return in_crate;
    }
    manifest.join("../../rules")
}

fn level_expr(l: Level) -> &'static str {
    match l {
        Level::Idle => "Level::Idle",
        Level::Known => "Level::Known",
        Level::Unknown => "Level::Unknown",
        Level::Caution => "Level::Caution",
        Level::Danger => "Level::Danger",
    }
}

fn target_expr(s: &str, ctx: &str) -> String {
    let s = s.trim();
    match s {
        "" | "positional" => "Target::Positional".into(),
        "first" => "Target::First".into(),
        "last" => "Target::Last".into(),
        "none" => "Target::None".into(),
        _ => {
            if let Some(k) = s.strip_prefix("kv:") {
                format!("Target::Kv({k:?})")
            } else if let Some(f) = s.strip_prefix("flag:") {
                format!("Target::Flag({f:?})")
            } else if let Some(n) = s.strip_prefix("after:") {
                let n: u8 = n.parse().unwrap_or_else(|_| panic!("{ctx}: bad target {s:?}"));
                format!("Target::After({n})")
            } else {
                panic!("{ctx}: unknown target selector {s:?}")
            }
        }
    }
}

fn escalate_expr(s: &str, ctx: &str) -> &'static str {
    match s {
        "" => "Escalate::None",
        "delete" => "Escalate::Delete",
        "modify" => "Escalate::Modify",
        "disk" => "Escalate::Disk",
        other => panic!("{ctx}: unknown escalate {other:?}"),
    }
}

fn check_words(text: &str, ctx: &str) {
    let n = text.split_whitespace().count();
    if n > MAX_WORDS {
        panic!("{ctx}: summary has {n} words (max {MAX_WORDS}): {text:?}");
    }
    if text.chars().any(|c| (c as u32) >= 0x1F000 || ('\u{2600}'..='\u{27BF}').contains(&c)) {
        panic!("{ctx}: summaries must not contain emoji: {text:?}");
    }
}

fn str_slice(v: &[String]) -> String {
    let mut s = String::from("&[");
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        write!(s, "{x:?}").unwrap();
    }
    s.push(']');
    s
}

fn verdict_expr(risk: Level, text: &str, pending: &str, hard: bool, ctx: &str) -> String {
    check_words(text, ctx);
    check_words(pending, ctx);
    if text.contains("{target}") && pending.is_empty() {
        println!("cargo:warning={ctx}: template uses {{target}} without a pending text");
    }
    format!(
        "Verdict {{ level: {}, text: {text:?}, pending: {pending:?}, hard: {hard} }}",
        level_expr(risk)
    )
}

fn cmd_expr(c: &CmdDef, file: &Path) -> String {
    let ctx = format!("{}:{}", file.display(), c.name);
    let long: Vec<String> = c
        .long
        .iter()
        .map(|(k, v)| format!("({k:?}, {v:?})"))
        .collect();
    let mut rules = String::from("&[");
    for (i, r) in c.rules.iter().enumerate() {
        let rctx = format!("{ctx} rule #{}", i + 1);
        let target = if r.target.is_empty() {
            "None".to_string()
        } else {
            format!("Some({})", target_expr(&r.target, &rctx))
        };
        write!(
            rules,
            "Rule {{ sub: {}, flags_all: {}, flags_any: {}, flags_none: {}, args_any: {}, target: {target}, verdict: {} }},",
            str_slice(&r.sub),
            str_slice(&r.flags_all),
            str_slice(&r.flags_any),
            str_slice(&r.flags_none),
            str_slice(&r.args_any),
            verdict_expr(r.risk, &r.text, &r.pending, r.hard, &rctx),
        )
        .unwrap();
    }
    rules.push(']');
    format!(
        "Cmd {{ name: {:?}, bundle: {}, takes_value: {}, long: &[{}], target: {}, escalate: {}, category: {:?}, default: {}, rules: {rules} }}",
        c.name,
        c.bundle,
        str_slice(&c.takes_value),
        long.join(", "),
        target_expr(&c.target, &ctx),
        escalate_expr(&c.escalate, &ctx),
        c.category,
        verdict_expr(c.risk, &c.text, &c.pending, c.hard, &ctx),
    )
}

fn main() {
    let dir = rules_dir();
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read rules dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();

    let mut cmds: Vec<(CmdDef, PathBuf)> = Vec::new();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let src = fs::read_to_string(f).unwrap();
        let pack: PackDef =
            toml::from_str(&src).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        for c in pack.cmds {
            cmds.push((c, f.clone()));
        }
    }

    let mut out = String::new();
    writeln!(out, "pub static CMDS: [Cmd; {}] = [", cmds.len()).unwrap();
    for (c, f) in &cmds {
        writeln!(out, "    {},", cmd_expr(c, f)).unwrap();
    }
    out.push_str("];\n");

    let mut seen = BTreeSet::new();
    let mut map = phf_codegen::Map::new();
    let mut entries: Vec<(String, String)> = Vec::new();
    for (i, (c, f)) in cmds.iter().enumerate() {
        for name in std::iter::once(&c.name).chain(c.aliases.iter()) {
            if !seen.insert(name.clone()) {
                panic!("{}: duplicate command {name:?}", f.display());
            }
            entries.push((name.clone(), i.to_string()));
        }
    }
    for (k, v) in &entries {
        map.entry(k.as_str(), v);
    }
    writeln!(
        out,
        "pub static CMD_INDEX: phf::Map<&'static str, u16> = {};",
        map.build()
    )
    .unwrap();

    let names: Vec<&String> = seen.iter().collect();
    writeln!(out, "pub static CMD_NAMES_SORTED: [&str; {}] = [", names.len()).unwrap();
    for n in names {
        writeln!(out, "    {n:?},").unwrap();
    }
    out.push_str("];\n");

    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("builtin_rules.rs");
    fs::write(dest, out).unwrap();
}
