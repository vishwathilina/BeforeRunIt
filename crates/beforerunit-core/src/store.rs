//! Paths and the precompiled config cache (`~/.cache/beforerunit/compiled.bin`).
//!
//! The cache holds the user config, `rules.d` packs and themes as an rkyv
//! archive. The keystroke path memory-maps it and checks a handful of mtimes;
//! TOML is only parsed by `compile` (feature `compile`) when a source changed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::config::{Config, ThemeDef};
use crate::schema::CmdDef;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Source {
    pub path: String,
    pub mtime: u64,
}

#[derive(Debug, Clone, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Compiled {
    pub version: String,
    pub config: Config,
    pub cmds: Vec<CmdDef>,
    pub themes: Vec<ThemeDef>,
    pub sources: Vec<Source>,
    /// Parse errors, shown by `beforerunit doctor`.
    pub errors: Vec<String>,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("BEFORERUNIT_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    xdg("XDG_CONFIG_HOME", ".config").join("beforerunit")
}

pub fn cache_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("BEFORERUNIT_CACHE_DIR") {
        return PathBuf::from(p);
    }
    xdg("XDG_CACHE_HOME", ".cache").join("beforerunit")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn rules_dir() -> PathBuf {
    config_dir().join("rules.d")
}

pub fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}

pub fn compiled_file() -> PathBuf {
    cache_dir().join("compiled.bin")
}

pub fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(v) if !v.is_empty() && Path::new(&v).is_dir() => PathBuf::from(v),
        _ => PathBuf::from(format!("/tmp/beforerunit-{}", uid())),
    }
}

pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("BEFORERUNIT_SOCKET") {
        return PathBuf::from(p);
    }
    runtime_dir().join("beforerunit.sock")
}

pub fn mtime(p: &Path) -> u64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Files and directories whose mtimes decide whether the cache is stale.
pub fn source_paths() -> Vec<PathBuf> {
    let mut v = vec![config_file(), rules_dir(), themes_dir()];
    for d in [rules_dir(), themes_dir()] {
        if let Ok(rd) = fs::read_dir(&d) {
            let mut files: Vec<PathBuf> = rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect();
            files.sort();
            v.extend(files);
        }
    }
    v
}

/// Memory-maps and validates the cache. `None` if missing or corrupt.
pub fn load() -> Option<Compiled> {
    load_from(&compiled_file())
}

pub fn load_from(path: &Path) -> Option<Compiled> {
    let f = fs::File::open(path).ok()?;
    // SAFETY: the file is only ever replaced atomically via rename, never
    // truncated in place, so the mapping stays valid while we read it.
    let map = unsafe { memmap2::Mmap::map(&f) }.ok()?;
    // Page-aligned, so it satisfies rkyv's alignment without copying.
    rkyv::from_bytes::<Compiled, rkyv::rancor::Error>(&map[..]).ok()
}

/// True if no source changed since the cache was written.
pub fn is_fresh(c: &Compiled) -> bool {
    if c.version != VERSION {
        return false;
    }
    let now = source_paths();
    if now.len() != c.sources.len() {
        return false;
    }
    now.iter().zip(&c.sources).all(|(p, s)| p.to_string_lossy() == s.path && mtime(p) == s.mtime)
}

pub fn save(c: &Compiled) -> std::io::Result<()> {
    let path = compiled_file();
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(c).map_err(|e| std::io::Error::other(e.to_string()))?;
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, &path)
}

/// Parses all TOML sources into a fresh cache.
#[cfg(feature = "compile")]
pub fn compile() -> Compiled {
    let mut c = Compiled { version: VERSION.to_string(), ..Default::default() };
    let cfg_path = config_file();
    if let Ok(src) = fs::read_to_string(&cfg_path) {
        match toml::from_str::<Config>(&src) {
            Ok(cfg) => c.config = cfg,
            Err(e) => c.errors.push(format!("{}: {e}", cfg_path.display())),
        }
    }
    for p in source_paths() {
        let is_rule = p.parent() == Some(rules_dir().as_path());
        let is_theme = p.parent() == Some(themes_dir().as_path());
        if is_rule || is_theme {
            match fs::read_to_string(&p) {
                Ok(src) if is_rule => match toml::from_str::<crate::schema::PackDef>(&src) {
                    Ok(pack) => c.cmds.extend(pack.cmds),
                    Err(e) => c.errors.push(format!("{}: {e}", p.display())),
                },
                Ok(src) => match toml::from_str::<ThemeDef>(&src) {
                    Ok(mut t) => {
                        if t.name.is_empty() {
                            t.name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        }
                        c.themes.push(t)
                    }
                    Err(e) => c.errors.push(format!("{}: {e}", p.display())),
                },
                Err(e) => c.errors.push(format!("{}: {e}", p.display())),
            }
        }
        c.sources.push(Source { path: p.to_string_lossy().into_owned(), mtime: mtime(&p) });
    }
    c
}

/// Loads the cache, recompiling (and saving) it if stale.
#[cfg(feature = "compile")]
pub fn load_or_compile() -> Compiled {
    if let Some(c) = load() {
        if is_fresh(&c) {
            return c;
        }
    }
    let c = compile();
    let _ = save(&c);
    c
}

/// Writes the config back as TOML (used by the setup app).
#[cfg(feature = "compile")]
pub fn write_config(cfg: &Config) -> std::io::Result<()> {
    let path = config_file();
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let body = toml::to_string_pretty(cfg).map_err(|e| std::io::Error::other(e.to_string()))?;
    let header = "# BeforeRunIt configuration. Edit by hand or run `beforerunit setup`.\n# The API key is stored in the system keyring, not in this file.\n\n";
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, format!("{header}{body}"))?;
    fs::rename(&tmp, &path)
}
