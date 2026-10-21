//! Facts sent to the AI next to the command. Only names and booleans: never
//! file contents, full paths or environment values.

use std::path::{Path, PathBuf};
use std::time::Duration;

use beforerunit_ai::{Context, Endpoint};
use beforerunit_core::config::AiConfig;

const MAX_TARGETS: usize = 4;

fn find_git(cwd: &Path) -> Option<PathBuf> {
    let mut d = Some(cwd);
    while let Some(p) = d {
        if p.join(".git").exists() {
            return Some(p.to_path_buf());
        }
        d = p.parent();
    }
    None
}

fn basename(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

fn expand(t: &str, cwd: &Path) -> PathBuf {
    if let Some(rest) = t.strip_prefix("~/") {
        if let Some(h) = std::env::var_os("HOME") {
            return Path::new(&h).join(rest);
        }
    }
    let p = Path::new(t);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// Short display name for a target: relative ones as typed, absolute ones
/// only if they are system paths (never the user's home layout).
fn target_name(t: &str) -> String {
    if t.starts_with('/') && !t.starts_with("/home/") {
        return t.to_string();
    }
    if t.starts_with('/') || t.starts_with("~/") {
        return basename(Path::new(t));
    }
    t.chars().take(60).collect()
}

/// Gathers context. `git status` runs with a hard timeout.
pub async fn gather(cwd: &str, root: bool, targets: &[String]) -> Context {
    let cwd_path = PathBuf::from(if cwd.is_empty() { "/" } else { cwd });
    let git_root = find_git(&cwd_path);
    let git_dirty = match &git_root {
        Some(_) => git_dirty(&cwd_path).await,
        None => None,
    };
    let targets = targets
        .iter()
        .filter(|t| !t.is_empty() && !t.contains(['*', '?', '$', '`']) && !t.contains("://"))
        .take(MAX_TARGETS)
        .map(|t| (target_name(t), expand(t, &cwd_path).symlink_metadata().is_ok()))
        .collect();
    Context { cwd_name: basename(&cwd_path), git_repo: git_root.is_some(), git_dirty, root, targets }
}

async fn git_dirty(cwd: &Path) -> Option<bool> {
    let child = tokio::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_millis(400), child).await {
        Ok(Ok(o)) if o.status.success() => Some(!o.stdout.is_empty()),
        _ => None,
    }
}

pub fn endpoint(cfg: &AiConfig, api_key: Option<String>) -> Endpoint {
    Endpoint {
        base_url: cfg.base_url.clone(),
        model: cfg.model.clone(),
        api_key,
        timeout: Duration::from_millis(cfg.timeout_ms.clamp(500, 60_000)),
    }
}
