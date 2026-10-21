//! Draws the ghost line below the command line using plain escape sequences.
//! Used in-process by the bash loadable builtin and by `beforerunit query`
//! (fish, and bash without the builtin). zsh renders via the daemon instead.

use std::time::{Duration, Instant};

use crate::config::{AiMode, Config, DisplayMode};
use crate::engine::Engine;
use crate::ghost::{build_view, AiStatus};
use crate::proto::{self, escape, Caps};
use crate::render::{self, ColorDepth, Opts, Theme};
use crate::store::{self, Compiled};

#[derive(Debug, Clone, Default)]
pub struct DrawArgs<'a> {
    pub line: &'a str,
    /// Cursor position in characters (readline point).
    pub point: usize,
    pub cols: usize,
    /// Visible width of the last prompt line.
    pub prompt_width: usize,
    pub sid: &'a str,
    /// Session override from the toggle hotkey, or empty.
    pub mode: &'a str,
    pub cwd: &'a str,
    pub euid: &'a str,
    pub tty: &'a str,
    pub pid: &'a str,
    /// Cursor sits at column 0 of the first input row (bash `bind -x`), so
    /// the ghost row is below the whole input. Otherwise it is at `point`.
    pub from_line_start: bool,
    /// Color depth code from the shell (`t`, `2`, `1`, `n`), overriding env detection.
    pub caps: &'a str,
}

pub struct Painter {
    engine: Engine,
    compiled: Compiled,
    theme: Theme,
    depth: ColorDepth,
    last_check: Instant,
    last_line: String,
    /// What is currently on screen, to skip identical redraws.
    shown: String,
    drawn: bool,
    last_spawn: Option<Instant>,
    recompile: fn() -> Option<Compiled>,
    tty: Option<String>,
}

/// Row offsets: (rows from the first input row to the ghost row,
/// rows from the cursor row to the ghost row).
pub fn rows_below(prompt_width: usize, line: &str, point: usize, cols: usize) -> (u16, u16) {
    let cols = cols.max(10);
    let len = line.chars().count();
    let last_row = (prompt_width + len) / cols + line.matches('\n').count();
    let point_row = (prompt_width + point.min(len)) / cols + line.chars().take(point).filter(|c| *c == '\n').count();
    ((last_row + 1) as u16, (last_row.saturating_sub(point_row) + 1) as u16)
}

/// Escape sequence that paints `ansi` on the row `down` rows below the
/// cursor and returns the cursor to where it was. With `ensure`, it first
/// scrolls if needed so that row exists (IND/RI keep the column).
pub fn paint_below(ansi: &str, down: u16, ensure: bool) -> String {
    let mut s = String::with_capacity(ansi.len() + 32);
    if ensure {
        for _ in 0..down {
            s.push_str("\x1bD");
        }
        for _ in 0..down {
            s.push_str("\x1bM");
        }
    }
    s.push_str("\x1b7");
    s.push_str(&format!("\x1b[{down}B\r\x1b[2K"));
    s.push_str(ansi);
    s.push_str("\x1b8");
    s
}

fn default_recompile() -> Option<Compiled> {
    None
}

impl Painter {
    /// `recompile` rebuilds the cache when sources changed (the CLI compiles
    /// in-process; the bash builtin spawns `beforerunit compile`).
    pub fn new(recompile: Option<fn() -> Option<Compiled>>) -> Painter {
        let recompile = recompile.unwrap_or(default_recompile);
        let compiled = load_fresh(recompile);
        crate::apply(&compiled);
        let theme = Theme::resolve(&compiled.config.ui.theme, &compiled.themes);
        Painter {
            engine: Engine::new(),
            compiled,
            theme,
            depth: ColorDepth::from_env(),
            last_check: Instant::now(),
            last_line: String::new(),
            shown: String::new(),
            drawn: false,
            last_spawn: None,
            recompile,
            tty: None,
        }
    }

    pub fn config(&self) -> &Config {
        &self.compiled.config
    }

    /// Re-checks config freshness at most once a second.
    fn maybe_reload(&mut self) {
        if self.last_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_check = Instant::now();
        if !store::is_fresh(&self.compiled) {
            self.compiled = load_fresh(self.recompile);
            crate::apply(&self.compiled);
            self.theme = Theme::resolve(&self.compiled.config.ui.theme, &self.compiled.themes);
            self.engine.reset();
            self.shown.clear();
        }
    }

    fn display_mode(&self, a: &DrawArgs) -> DisplayMode {
        DisplayMode::parse(a.mode).unwrap_or(self.compiled.config.ui.display)
    }

    /// Returns the escape sequence to print (empty if nothing changed).
    pub fn draw(&mut self, a: &DrawArgs) -> String {
        let (down_now, down_async) = rows_below(a.prompt_width, a.line, a.point, a.cols);
        let down = if a.from_line_start { down_now } else { down_async };
        let ansi = self.line(a, down_async);
        if ansi.is_empty() {
            return self.clear_inner(down, a.from_line_start);
        }
        // Keyed by row offset too: the input may have grown a row.
        let key = format!("{down}:{ansi}");
        if self.drawn && key == self.shown {
            return String::new();
        }
        self.shown = key;
        self.drawn = true;
        paint_below(&ansi, down, true)
    }

    /// Only the styled ghost line, no cursor movement (ble.sh info area).
    /// Also asks the daemon for an AI review when one is wanted.
    pub fn line(&mut self, a: &DrawArgs, down_async: u16) -> String {
        self.maybe_reload();
        if let Some(c) = a.caps.chars().next() {
            self.depth = ColorDepth::from_code(c);
        }
        let display = self.display_mode(a);
        let analysis = self.engine.analyze(a.line).clone();
        let ai_mode = self.compiled.config.ai.effective_mode();
        let (icons, screen_reader) = (self.compiled.config.ui.icons, self.compiled.config.ui.screen_reader);
        let wants_ai = match ai_mode {
            AiMode::Off => false,
            AiMode::UnknownOnly => analysis.needs_ai(),
            AiMode::ReviewAll => !analysis.is_empty() && analysis.level != crate::Level::Idle,
        } && display != DisplayMode::Off;

        if wants_ai && a.line != self.last_line {
            let tty = if a.tty.is_empty() { self.tty() } else { a.tty.to_string() };
            let caps = Caps { depth: self.depth, italic: true };
            let msg = format!(
                "N\t{}\t0\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                escape(a.sid),
                a.cols,
                caps.encode(),
                escape(a.mode),
                escape(a.cwd),
                escape(a.euid),
                escape(&tty),
                escape(a.pid),
                down_async,
                escape(a.line)
            );
            if !proto::send(&msg) {
                self.spawn_daemon();
            }
        }
        self.last_line.clear();
        self.last_line.push_str(a.line);

        let ai = if wants_ai { AiStatus::Pending } else { AiStatus::Off };
        let view = build_view(&analysis, &ai, display);
        match view {
            Some(v) => {
                let o = Opts {
                    theme: &self.theme,
                    icons,
                    depth: self.depth,
                    frame: 0,
                    motion: false,
                    screen_reader,
                    max_cols: a.cols.saturating_sub(1),
                };
                render::to_ansi(&render::render(&v, &o), self.depth)
            }
            None => String::new(),
        }
    }

    /// Terminal of this process (stdin), looked up once.
    fn tty(&mut self) -> String {
        if self.tty.is_none() {
            // SAFETY: ttyname returns a pointer to a static buffer or NULL;
            // it is copied out immediately.
            let name = unsafe {
                let p = libc::ttyname(0);
                if p.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
                }
            };
            self.tty = Some(name);
        }
        self.tty.clone().unwrap_or_default()
    }

    fn clear_inner(&mut self, down: u16, _from_line_start: bool) -> String {
        if !self.drawn {
            return String::new();
        }
        self.drawn = false;
        self.shown.clear();
        paint_below("", down, false)
    }

    /// Clears the ghost line (Enter, Ctrl-C) and tells the daemon to stop.
    pub fn clear(&mut self, a: &DrawArgs) -> String {
        let (down_now, down_async) = rows_below(a.prompt_width, a.line, a.point, a.cols);
        if !a.sid.is_empty() && self.compiled.config.ai.effective_mode() != AiMode::Off {
            proto::send(&format!("X\t{}", escape(a.sid)));
        }
        self.last_line.clear();
        self.clear_inner(if a.from_line_start { down_now } else { down_async }, a.from_line_start)
    }

    /// A one-shot process (`beforerunit query`) is told by the shell that a
    /// ghost line from an earlier call is on screen, so `clear` erases it.
    pub fn assume_drawn(&mut self, shown: &str) {
        self.drawn = true;
        self.shown = shown.to_string();
    }

    /// Diff key of what is on screen, for shells that keep it between calls.
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// Forget what is on screen (a new prompt was printed). Returns whether a
    /// ghost line was still drawn, i.e. the line was aborted (Ctrl-C).
    pub fn forget(&mut self) -> bool {
        let was = self.drawn;
        self.drawn = false;
        self.shown.clear();
        self.last_line.clear();
        was
    }

    fn spawn_daemon(&mut self) {
        if self.last_spawn.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
            return;
        }
        self.last_spawn = Some(Instant::now());
        spawn_daemon();
    }
}

fn load_fresh(recompile: fn() -> Option<Compiled>) -> Compiled {
    if let Some(c) = store::load() {
        if store::is_fresh(&c) {
            return c;
        }
    }
    recompile().or_else(store::load).unwrap_or_else(|| Compiled { version: store::VERSION.into(), ..Default::default() })
}

static BINARY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Set by the bash plugin, which knows where `beforerunit` lives.
pub fn set_binary_path(p: &str) {
    if !p.is_empty() {
        let _ = BINARY.set(p.to_string());
    }
}

/// Path of the `beforerunit` binary for spawning helpers.
pub fn binary_path() -> String {
    if let Some(p) = BINARY.get() {
        return p.clone();
    }
    if let Ok(p) = std::env::var("BEFORERUNIT_BIN") {
        if !p.is_empty() {
            return p;
        }
    }
    "beforerunit".into()
}

/// Starts the daemon in the background, detached from the terminal.
pub fn spawn_daemon() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(binary_path());
    cmd.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and only detaches the child.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if let Ok(mut child) = cmd.spawn() {
        // The daemon forks into the background itself; reap the launcher.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows() {
        // Prompt "$ " (2) + 10 chars in 80 cols: one row, ghost just below.
        assert_eq!(rows_below(2, "0123456789", 10, 80), (1, 1));
        // Wrapped: 2 + 100 chars in 80 cols = 2 rows; cursor at start.
        assert_eq!(rows_below(2, &"x".repeat(100), 0, 80), (2, 2));
        assert_eq!(rows_below(2, &"x".repeat(100), 100, 80), (2, 1));
    }

    #[test]
    fn paint_sequence() {
        let s = paint_below("X", 1, true);
        assert!(s.starts_with("\x1bD\x1bM\x1b7\x1b[1B\r\x1b[2KX\x1b8"));
    }
}
