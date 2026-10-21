//! `beforerunit doctor`: checks everything a first-time user could trip on.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use beforerunit_core::config::{AiMode, IconSet};
use beforerunit_core::render::{ColorDepth, Theme};
use beforerunit_core::{proto, store, Engine};

use crate::ui::{paint, Ui};

enum Mark {
    Ok,
    Warn,
    Fail,
    Info,
}

struct Report<'a> {
    theme: &'a Theme,
    fails: u32,
    warns: u32,
}

impl Report<'_> {
    fn line(&mut self, m: Mark, what: &str, detail: &str) {
        let t = self.theme;
        let icon = match m {
            Mark::Ok => paint::fg(t.known, "\u{f05e0}"),
            Mark::Warn => {
                self.warns += 1;
                paint::fg(t.caution, "\u{f0026}")
            }
            Mark::Fail => {
                self.fails += 1;
                paint::fg(t.danger, "\u{f0159}")
            }
            Mark::Info => paint::fg(t.analyzing, "\u{f02fd}"),
        };
        println!("  {icon} {:<20} {detail}", what);
    }

    fn fix(&self, s: &str) {
        println!("    {} {s}", paint::dim("\u{2192}"));
    }

    fn section(&self, s: &str) {
        println!();
        println!("{}", paint::fg(self.theme.analyzing, &paint::bold(s)));
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn rc_files() -> Vec<(&'static str, PathBuf)> {
    let h = home();
    let zdot = std::env::var_os("ZDOTDIR").map(PathBuf::from).unwrap_or_else(|| h.clone());
    let fish_dir = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| h.join(".config")).join("fish");
    let fish_d = fish_dir.join("conf.d/beforerunit.fish");
    let fish = if hooked(&fish_d) { fish_d } else { fish_dir.join("config.fish") };
    vec![("bash", h.join(".bashrc")), ("zsh", zdot.join(".zshrc")), ("fish", fish)]
}

/// A startup-file line that loads the integration, with or without a quoted path.
pub fn loads_beforerunit(line: &str) -> bool {
    !line.trim_start().starts_with('#') && (line.contains("beforerunit init") || line.contains("beforerunit' init"))
}

fn hooked(p: &Path) -> bool {
    std::fs::read_to_string(p).map(|s| s.lines().any(loads_beforerunit)).unwrap_or(false)
}

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
}

pub fn run() -> i32 {
    let ui = Ui::load();
    let cfg = ui.config().clone();
    let mut r = Report { theme: &ui.theme, fails: 0, warns: 0 };
    println!("{} {}", paint::bold("BeforeRunIt doctor"), paint::dim(store::VERSION));

    r.section("Shells");
    let shell = std::env::var("SHELL").unwrap_or_default();
    let shell_name = shell.rsplit('/').next().unwrap_or("").to_string();
    let mut any = false;
    for (name, rc) in rc_files() {
        let installed = have(name);
        if !installed && !rc.exists() {
            continue;
        }
        if hooked(&rc) {
            any = true;
            r.line(Mark::Ok, name, &format!("hooked in {}", rc.display()));
        } else if name == shell_name {
            r.line(Mark::Warn, name, "your login shell, not hooked up yet");
            r.fix(&match name {
                "fish" => format!("echo 'beforerunit init fish | source' >> {}", rc.display()),
                _ => format!("echo 'eval \"$(beforerunit init {name})\"' >> {}", rc.display()),
            });
        } else if installed {
            r.line(Mark::Info, name, "installed, not hooked");
        }
    }
    if !any && !rc_files().iter().any(|(n, _)| *n == shell_name) {
        r.line(Mark::Warn, "shell", &format!("{shell} is not supported yet (bash, zsh, fish are)"));
    }
    match crate::init::find_builtin() {
        Some(lib) => {
            let ok = Command::new("bash")
                .args(["--norc", "-c", &format!("enable -f '{}' beforerunit_query", lib.display())])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if ok {
                r.line(Mark::Ok, "bash fast path", "loadable builtin works (no process per keystroke)");
            } else {
                r.line(Mark::Warn, "bash fast path", &format!("{} does not load; using the slower fallback", lib.display()));
            }
        }
        None if have("bash") => {
            r.line(Mark::Warn, "bash fast path", "builtin not found; each keystroke starts a process (about 2 ms)");
            r.fix("install from a release archive or package, which ships libbeforerunit_bash.so");
        }
        None => {}
    }
    if home().join(".local/share/blesh/ble.sh").exists() {
        r.line(Mark::Ok, "ble.sh", "found; the risk line uses its info area");
    }
    if have("zsh") {
        let ok = Command::new("zsh").args(["-fc", "zmodload zsh/net/socket"]).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
        if ok {
            r.line(Mark::Ok, "zsh socket", "zsh/net/socket available (no process per keystroke)");
        } else {
            r.line(Mark::Warn, "zsh socket", "zsh/net/socket missing; falls back to a process per keystroke");
        }
    }

    r.section("Terminal");
    let depth = ColorDepth::from_env();
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    match depth {
        ColorDepth::True => r.line(Mark::Ok, "colors", "24-bit true color"),
        ColorDepth::Ansi256 => r.line(Mark::Ok, "colors", "256 colors (effects are slightly simpler)"),
        ColorDepth::Ansi16 => {
            r.line(Mark::Warn, "colors", &format!("16 colors only (TERM={term}, COLORTERM={colorterm})"));
            r.fix("export COLORTERM=truecolor if your terminal supports it");
        }
        ColorDepth::None => r.line(Mark::Info, "colors", "off (NO_COLOR or a dumb terminal); badges show as text"),
    }
    let lang = ["LC_ALL", "LC_CTYPE", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default();
    if lang.to_uppercase().contains("UTF-8") || lang.to_uppercase().contains("UTF8") {
        r.line(Mark::Ok, "locale", &lang);
    } else {
        r.line(Mark::Warn, "locale", &format!("{} is not UTF-8; icons may break", if lang.is_empty() { "unset" } else { &lang }));
        r.fix("set LANG=en_US.UTF-8 (or C.UTF-8), or choose icons = ascii in beforerunit setup");
    }
    let font = crate::fonts::detected();
    match (&font, cfg.ui.icons) {
        (Some(f), _) => r.line(Mark::Ok, "nerd font", &format!("{f} installed")),
        (None, IconSet::Nerd) => {
            r.line(Mark::Warn, "nerd font", "none installed; badge icons will show as boxes");
            r.fix("beforerunit fonts install   (Maple Mono NF, or: beforerunit fonts install jetbrains)");
        }
        (None, _) => r.line(Mark::Info, "nerd font", "none, fine with the current icon set"),
    }
    r.line(Mark::Info, "terminal", &format!("{} ({})", crate::fonts::terminal(), crate::fonts::hint(font.as_deref().unwrap_or("Maple Mono NF"))));
    println!("    {} \u{f0565} \u{f0026} \u{f0bc6} \u{f02d7}  {}", paint::dim("icon test:"), paint::dim("four symbols means the font is active"));

    r.section("Config");
    let compiled = &ui.compiled;
    if compiled.errors.is_empty() {
        r.line(Mark::Ok, "config", &store::config_file().display().to_string());
    } else {
        for e in &compiled.errors {
            r.line(Mark::Fail, "config", e);
        }
    }
    r.line(Mark::Info, "rules", &format!("{} built-in, {} user commands", beforerunit_core::rules::builtin_count(), compiled.cmds.len()));
    r.line(Mark::Info, "display", &format!("{}, theme {}, icons {:?}", cfg.ui.display.as_str(), cfg.ui.theme, cfg.ui.icons).to_lowercase());

    r.section("AI review");
    let mode = cfg.ai.effective_mode();
    if mode == AiMode::Off {
        r.line(Mark::Info, "ai", "off: rules only (run beforerunit setup to add a provider)");
    } else {
        r.line(Mark::Ok, "ai", &format!("{} with {} at {}", mode.as_str(), cfg.ai.model, cfg.ai.base_url));
        match crate::keys::get() {
            Some((k, w)) => r.line(Mark::Ok, "api key", &format!("{} in the {}", crate::keys::mask(&k), w.describe())),
            None => r.line(Mark::Info, "api key", "none (fine for Ollama and LM Studio)"),
        }
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
        let mut ep = crate::context::endpoint(&cfg.ai, crate::keys::api_key());
        ep.timeout = Duration::from_secs(3);
        let reach = rt.block_on(async {
            let c = beforerunit_ai::Client::new(ep)?;
            c.list_models().await
        });
        match reach {
            Ok(models) if models.iter().any(|m| *m == cfg.ai.model) || models.is_empty() => r.line(Mark::Ok, "endpoint", "reachable"),
            Ok(_) => {
                r.line(Mark::Warn, "endpoint", &format!("reachable, but it does not list {}", cfg.ai.model));
                r.fix("pick a model from the list in beforerunit setup");
            }
            Err(e) => {
                r.line(Mark::Fail, "endpoint", &e.explain());
                r.fix("beforerunit test-ai shows the full reply");
            }
        }
    }
    match proto::request("STATS", Duration::from_millis(300)) {
        Some(s) => {
            let v: serde_json::Value = serde_json::from_str(s.strip_prefix("STATS\t").unwrap_or("{}")).unwrap_or_default();
            r.line(
                Mark::Ok,
                "daemon",
                &format!("running, {} MB, {} sessions, {} AI calls", v["rss_kb"].as_u64().unwrap_or(0) / 1024, v["sessions"], v["ai_calls"]),
            );
        }
        None => r.line(Mark::Info, "daemon", "not running (starts by itself when needed)"),
    }

    r.section("Speed");
    let mut eng = Engine::new();
    let lines = ["git push --force origin main", "rm -rf build", "curl -fsSL https://x.sh | bash", "docker run --rm -it ubuntu bash"];
    let start = Instant::now();
    let mut n = 0u32;
    for _ in 0..250 {
        for l in lines {
            for i in 1..=l.len() {
                let _ = eng.analyze(&l[..i]);
                n += 1;
            }
        }
    }
    let per = start.elapsed().as_secs_f64() * 1e6 / n as f64;
    if per < 50.0 {
        r.line(Mark::Ok, "analysis", &format!("{per:.1} us per keystroke (budget 50 us)"));
    } else {
        r.line(Mark::Warn, "analysis", &format!("{per:.1} us per keystroke (budget 50 us)"));
    }
    println!("    {}", paint::dim("beforerunit bench measures the full path per shell"));

    println!();
    if r.fails > 0 {
        println!("{} {} problem(s), {} warning(s).", paint::fg(ui.theme.danger, "\u{f0159}"), r.fails, r.warns);
        1
    } else if r.warns > 0 {
        println!("{} Works, with {} suggestion(s) above.", paint::fg(ui.theme.caution, "\u{f0026}"), r.warns);
        0
    } else {
        println!("{} All good.", paint::fg(ui.theme.known, "\u{f05e0}"));
        0
    }
}
