//! `beforerunit fonts`: installs a Nerd Font so the badge icons render, and
//! tells the user how to select it in their terminal.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::ui::{paint, Ui};

#[derive(Clone, Copy, clap::ValueEnum, Debug, PartialEq, Eq)]
pub enum Font {
    /// Maple Mono NF: rounded, friendly, great ligatures (default)
    Maple,
    /// JetBrainsMono Nerd Font: the classic choice
    Jetbrains,
}

impl Font {
    fn url(self) -> &'static str {
        match self {
            Font::Maple => "https://github.com/subframe7536/maple-font/releases/latest/download/MapleMono-NF-unhinted.zip",
            Font::Jetbrains => "https://github.com/ryanoasis/nerd-fonts/releases/latest/download/JetBrainsMono.zip",
        }
    }

    pub fn family(self) -> &'static str {
        match self {
            Font::Maple => "Maple Mono NF",
            Font::Jetbrains => "JetBrainsMono Nerd Font",
        }
    }

    fn dir_name(self) -> &'static str {
        match self {
            Font::Maple => "MapleMonoNF",
            Font::Jetbrains => "JetBrainsMonoNF",
        }
    }
}

/// Where `fonts install` puts files; `uninstall` removes this directory.
pub fn install_root() -> PathBuf {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"),
    };
    base.join("fonts/beforerunit")
}

/// Nerd Font families known to fontconfig.
pub fn nerd_families() -> Vec<String> {
    let Ok(out) = std::process::Command::new("fc-list").args([":", "family"]).output() else { return Vec::new() };
    let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .flat_map(|l| l.split(','))
        .map(|s| s.trim().to_string())
        .filter(|f| f.contains("Nerd Font") || f.ends_with(" NF") || f.contains(" NF ") || f.ends_with("NFM"))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The preferred installed Nerd Font, looked up once per process.
pub fn detected() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let fams = nerd_families();
            for want in ["Maple Mono NF", "JetBrainsMono Nerd Font", "JetBrainsMono NF"] {
                if let Some(f) = fams.iter().find(|f| f.as_str() == want) {
                    return Some(f.clone());
                }
            }
            fams.into_iter().next()
        })
        .clone()
}

pub fn terminal() -> &'static str {
    let env = |k: &str| std::env::var_os(k).is_some();
    let term = std::env::var("TERM").unwrap_or_default();
    let prog = std::env::var("TERM_PROGRAM").unwrap_or_default();
    if env("KITTY_WINDOW_ID") || term.contains("kitty") {
        "kitty"
    } else if env("ALACRITTY_WINDOW_ID") || term.contains("alacritty") {
        "alacritty"
    } else if env("WEZTERM_EXECUTABLE") || prog == "WezTerm" {
        "wezterm"
    } else if env("GHOSTTY_RESOURCES_DIR") || prog == "ghostty" {
        "ghostty"
    } else if term.starts_with("foot") {
        "foot"
    } else if env("KONSOLE_VERSION") {
        "konsole"
    } else if prog == "vscode" {
        "vscode"
    } else if env("VTE_VERSION") {
        "vte"
    } else if env("TMUX") {
        "tmux"
    } else {
        "unknown"
    }
}

pub fn hint(family: &str) -> String {
    match terminal() {
        "kitty" => format!("kitty: add `font_family {family}` to ~/.config/kitty/kitty.conf, then press Ctrl+Shift+F5"),
        "alacritty" => format!("Alacritty: in ~/.config/alacritty/alacritty.toml set [font.normal] family = \"{family}\""),
        "wezterm" => format!("WezTerm: config.font = wezterm.font(\"{family}\") in ~/.wezterm.lua"),
        "ghostty" => format!("Ghostty: add `font-family = {family}` to ~/.config/ghostty/config"),
        "foot" => format!("foot: set `font={family}:size=11` in ~/.config/foot/foot.ini"),
        "konsole" => format!("Konsole: Settings > Edit Current Profile > Appearance > Font > {family}"),
        "vscode" => format!("VS Code: set \"terminal.integrated.fontFamily\": \"{family}\""),
        "vte" => format!("GNOME Terminal / Ptyxis / Tilix: Preferences > Profile > Custom font > {family}"),
        "tmux" => format!("tmux uses the outer terminal's font: select {family} there"),
        _ => format!("Select \"{family}\" as the font in your terminal settings"),
    }
}

fn download(url: &str) -> anyhow::Result<Vec<u8>> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        use futures_util::StreamExt;
        let client = reqwest::Client::builder().user_agent(concat!("beforerunit/", env!("CARGO_PKG_VERSION"))).build()?;
        let resp = client.get(url).send().await?.error_for_status()?;
        let total = resp.content_length().unwrap_or(0);
        let mut body = Vec::with_capacity(total as usize);
        let mut stream = resp.bytes_stream();
        let mut last = 0usize;
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk?);
            if total > 0 {
                let pct = (body.len() as u64 * 100 / total) as usize;
                if pct != last {
                    last = pct;
                    progress(pct, body.len(), total as usize);
                }
            }
        }
        eprintln!();
        Ok(body)
    })
}

/// Progress bar with a moving highlight, drawn on stderr.
fn progress(pct: usize, got: usize, total: usize) {
    let width = 30;
    let filled = pct * width / 100;
    let mut bar = String::new();
    for i in 0..width {
        let c = if i < filled { '\u{2501}' } else { '\u{2500}' };
        if paint::enabled() {
            let col = if i < filled {
                if i + 1 == filled {
                    "255;255;255"
                } else {
                    "125;211;252"
                }
            } else {
                "71;85;105"
            };
            bar.push_str(&format!("\x1b[38;2;{col}m{c}"));
        } else {
            bar.push(c);
        }
    }
    if paint::enabled() {
        bar.push_str("\x1b[0m");
    }
    eprint!("\r  {bar} {pct:>3}%  {:.1}/{:.1} MB", got as f64 / 1e6, total as f64 / 1e6);
    let _ = std::io::stderr().flush();
}

pub fn install(font: Font) -> i32 {
    let ui = Ui::load();
    let t = &ui.theme;
    let dir = install_root().join(font.dir_name());
    println!("{} {}", paint::fg(t.analyzing, "Installing"), paint::bold(font.family()));
    let zip = match download(font.url()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("beforerunit: download failed: {e}");
            return 1;
        }
    };
    let mut archive = match zip::ZipArchive::new(std::io::Cursor::new(zip)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("beforerunit: bad archive: {e}");
            return 1;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("beforerunit: cannot create {}: {e}", dir.display());
        return 1;
    }
    let mut n = 0;
    for i in 0..archive.len() {
        let Ok(mut f) = archive.by_index(i) else { continue };
        let Some(name) = f.enclosed_name().and_then(|p| p.file_name().map(|s| s.to_owned())) else { continue };
        let lower = name.to_string_lossy().to_lowercase();
        // Keep the monospace variants; skip proportional and Windows-only copies.
        if !(lower.ends_with(".ttf") || lower.ends_with(".otf")) || lower.contains("propo") || lower.contains("windows") {
            continue;
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_ok() && std::fs::write(dir.join(&name), &buf).is_ok() {
            n += 1;
        }
    }
    let cache = std::process::Command::new("fc-cache").arg("-f").arg(&dir).status();
    println!("{} {n} font files in {}", paint::fg(t.known, "\u{f05e0} Installed"), dir.display());
    if cache.is_err() {
        println!("{}", paint::dim("fc-cache not found; log out and back in so apps see the font."));
    }
    println!();
    println!("  {}", hint(font.family()));
    println!("  Then check the icons with {}", paint::bold("beforerunit doctor"));
    0
}

pub fn status() -> i32 {
    let ui = Ui::load();
    let t = &ui.theme;
    let fams = nerd_families();
    if fams.is_empty() {
        println!("{} No Nerd Font installed.", paint::fg(t.caution, "\u{f0026}"));
        println!("  Install one with {}", paint::bold("beforerunit fonts install"));
        println!("  Or keep plain symbols: set icons = \"unicode\" in beforerunit setup.");
        return 1;
    }
    println!("{} Nerd Fonts found:", paint::fg(t.known, "\u{f05e0}"));
    let mut main: Vec<&String> = fams.iter().filter(|f| f.ends_with("Nerd Font")).collect();
    if main.is_empty() {
        main = fams.iter().take(6).collect();
    }
    for f in &main {
        println!("  {f}");
    }
    if fams.len() > main.len() {
        println!("  {}", paint::dim(&format!("and {} more styles", fams.len() - main.len())));
    }
    let best = detected().unwrap_or_else(|| fams[0].clone());
    println!();
    println!("  {}", hint(&best));
    println!("  Icons: \u{f0565} \u{f0026} \u{f0bc6} \u{f02d7}  (four symbols, not boxes, means the terminal font works)");
    0
}