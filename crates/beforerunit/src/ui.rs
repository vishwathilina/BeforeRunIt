//! Loaded configuration plus the resolved theme, shared by every command.

use beforerunit_core::config::Config;
use beforerunit_core::render::{self, ColorDepth, Opts, Styled, Theme, View};
use beforerunit_core::store::{self, Compiled};

pub struct Ui {
    pub compiled: Compiled,
    pub theme: Theme,
}

impl Ui {
    pub fn load() -> Ui {
        let compiled = store::load_or_compile();
        beforerunit_core::apply(&compiled);
        let theme = Theme::resolve(&compiled.config.ui.theme, &compiled.themes);
        Ui { compiled, theme }
    }

    pub fn config(&self) -> &Config {
        &self.compiled.config
    }

    pub fn opts(&self, depth: ColorDepth, frame: u32, cols: usize, motion: bool) -> Opts<'_> {
        let ui = &self.compiled.config.ui;
        Opts {
            theme: &self.theme,
            icons: ui.icons,
            depth,
            frame,
            motion: motion && !ui.reduce_motion,
            screen_reader: ui.screen_reader,
            max_cols: cols.saturating_sub(1),
        }
    }

    pub fn render(&self, v: &View, depth: ColorDepth, frame: u32, cols: usize, motion: bool) -> Styled {
        render::render(v, &self.opts(depth, frame, cols, motion))
    }
}

pub fn term_cols() -> usize {
    // SAFETY: TIOCGWINSZ only writes into the provided winsize struct.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            return ws.ws_col as usize;
        }
    }
    std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()).unwrap_or(100)
}

/// Small ANSI helpers for plain CLI output (doctor, explain, test-ai).
pub mod paint {
    use beforerunit_core::render::Rgb;

    pub fn enabled() -> bool {
        std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
            && unsafe { libc::isatty(libc::STDOUT_FILENO) } == 1
    }

    pub fn fg(c: Rgb, s: &str) -> String {
        if enabled() {
            format!("\x1b[38;2;{};{};{}m{s}\x1b[0m", c.0, c.1, c.2)
        } else {
            s.to_string()
        }
    }

    pub fn bold(s: &str) -> String {
        if enabled() {
            format!("\x1b[1m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn dim(s: &str) -> String {
        if enabled() {
            format!("\x1b[2m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}
