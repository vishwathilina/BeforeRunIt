//! `beforerunit uninstall`: removes everything BeforeRunIt created.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use beforerunit_core::{proto, store};

use crate::ui::paint;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn rc_files() -> Vec<PathBuf> {
    let h = home();
    let zdot = std::env::var_os("ZDOTDIR").map(PathBuf::from).unwrap_or_else(|| h.clone());
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| h.join(".config"));
    vec![h.join(".bashrc"), h.join(".bash_profile"), zdot.join(".zshrc"), cfg.join("fish/config.fish"), cfg.join("fish/conf.d/beforerunit.fish")]
}

/// Lines that load BeforeRunIt; comment lines placed by install.sh go too.
fn is_hook(line: &str) -> bool {
    crate::doctor::loads_beforerunit(line) || line.trim() == "# BeforeRunIt"
}

fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().lock().read_line(&mut s);
    matches!(s.trim(), "y" | "Y" | "yes")
}

pub fn run(yes: bool, keep_config: bool) -> i32 {
    let mut plan: Vec<String> = Vec::new();
    let hooks: Vec<PathBuf> = rc_files()
        .into_iter()
        .filter(|p| std::fs::read_to_string(p).map(|s| s.lines().any(is_hook)).unwrap_or(false))
        .collect();
    for p in &hooks {
        plan.push(format!("remove the hook from {} (backup: .beforerunit.bak)", p.display()));
    }
    plan.push("stop the daemon".into());
    plan.push(format!("delete the cache {}", store::cache_dir().display()));
    if !keep_config {
        plan.push(format!("delete settings and rules {}", store::config_dir().display()));
        plan.push("delete the stored API key".into());
    }
    let fonts = crate::fonts::install_root();
    if fonts.exists() {
        plan.push(format!("delete fonts installed by BeforeRunIt in {}", fonts.display()));
    }
    println!("{}", paint::bold("This will:"));
    for p in &plan {
        println!("  - {p}");
    }
    if !yes && !confirm("Continue?") {
        println!("Nothing changed.");
        return 1;
    }

    for p in &hooks {
        if let Ok(src) = std::fs::read_to_string(p) {
            let _ = std::fs::write(p.with_extension("beforerunit.bak"), &src);
            let kept: Vec<&str> = src.lines().filter(|l| !is_hook(l)).collect();
            let mut out = kept.join("\n");
            if src.ends_with('\n') {
                out.push('\n');
            }
            if p.file_name().is_some_and(|n| n == "beforerunit.fish") {
                let _ = std::fs::remove_file(p);
            } else {
                let _ = std::fs::write(p, out);
            }
        }
    }
    let _ = proto::request("STOP", std::time::Duration::from_millis(500));
    let _ = std::fs::remove_file(store::socket_path());
    let _ = std::fs::remove_dir_all(store::cache_dir());
    if !keep_config {
        crate::keys::delete();
        let _ = std::fs::remove_dir_all(store::config_dir());
    }
    if fonts.exists() {
        let _ = std::fs::remove_dir_all(&fonts);
        let _ = std::process::Command::new("fc-cache").arg("-f").status();
    }

    println!("{}", paint::bold("Done. Open a new terminal to finish."));
    let exe = crate::init::exe_path();
    let dir = exe.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    println!("The program itself is still at {}. Remove it with your package manager, or:", exe.display());
    let mut rm = format!("  rm {}", exe.display());
    let bri = dir.join("bri");
    if bri.read_link().is_ok_and(|t| t.ends_with("beforerunit")) {
        rm.push_str(&format!(" {}", bri.display()));
    }
    if let Some(lib) = crate::init::find_builtin() {
        rm.push_str(&format!(" {}", lib.display()));
    }
    println!("{rm}");
    0
}

#[cfg(test)]
mod tests {
    use super::is_hook;

    #[test]
    fn hook_lines() {
        assert!(is_hook("eval \"$(beforerunit init bash)\""));
        assert!(is_hook("beforerunit init fish | source"));
        assert!(is_hook("eval \"$('/home/a/.local/bin/beforerunit' init bash)\""));
        assert!(!is_hook("alias ll='ls -la'"));
    }
}
