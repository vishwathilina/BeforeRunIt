// Shared between the library and `build.rs` (via `#[path]`), so it may only
// depend on `serde` and `rkyv`.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// Risk level, ordered by severity. `Unknown` outranks `Known` on purpose:
/// a pipeline containing anything we cannot vouch for is never shown as known.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    #[default]
    Idle,
    Known,
    Unknown,
    Caution,
    Danger,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "idle" => Level::Idle,
            "known" | "safe" | "green" | "low" => Level::Known,
            "unknown" => Level::Unknown,
            "caution" | "warn" | "warning" | "yellow" | "medium" => Level::Caution,
            "danger" | "dangerous" | "red" | "high" | "critical" => Level::Danger,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Idle => "idle",
            Level::Known => "known",
            Level::Unknown => "unknown",
            Level::Caution => "caution",
            Level::Danger => "danger",
        }
    }

    /// One step down on the Known / Caution / Danger scale.
    pub fn lower(self) -> Level {
        match self {
            Level::Danger => Level::Caution,
            Level::Caution => Level::Known,
            other => other,
        }
    }
}

fn default_true() -> bool {
    true
}

fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

/// One rule pack file (`rules/*.toml` or `~/.config/beforerunit/rules.d/*.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PackDef {
    #[serde(default, rename = "cmd")]
    pub cmds: Vec<CmdDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CmdDef {
    pub name: String,
    #[serde(default, deserialize_with = "one_or_many")]
    pub aliases: Vec<String>,
    /// Whether `-abc` means `-a -b -c` (false for `find -name`, `java -jar`, ...).
    #[serde(default = "default_true")]
    pub bundle: bool,
    /// Flags whose value is the next word (or the rest of a bundle).
    #[serde(default, deserialize_with = "one_or_many")]
    pub takes_value: Vec<String>,
    /// Long flag name to canonical short name, e.g. `recursive = "r"`.
    #[serde(default)]
    pub long: BTreeMap<String, String>,
    /// How to pick `{target}`: positional, first, last, none, `kv:KEY`, `flag:NAME`, `after:N`.
    #[serde(default)]
    pub target: String,
    /// Critical-path escalation family: "delete", "modify" or "disk".
    #[serde(default)]
    pub escalate: String,
    /// "read" and "info" commands are not raised when run as root.
    #[serde(default)]
    pub category: String,
    pub risk: Level,
    pub text: String,
    #[serde(default)]
    pub pending: String,
    #[serde(default)]
    pub hard: bool,
    #[serde(default, rename = "rule")]
    pub rules: Vec<RuleDef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RuleDef {
    /// Subcommand(s); "system prune" matches two leading positionals.
    #[serde(default, deserialize_with = "one_or_many")]
    pub sub: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub flags_all: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub flags_any: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub flags_none: Vec<String>,
    /// Glob patterns (`*`, `?`) matched against positional arguments.
    #[serde(default, deserialize_with = "one_or_many")]
    pub args_any: Vec<String>,
    #[serde(default)]
    pub target: String,
    pub risk: Level,
    pub text: String,
    #[serde(default)]
    pub pending: String,
    #[serde(default)]
    pub hard: bool,
}
