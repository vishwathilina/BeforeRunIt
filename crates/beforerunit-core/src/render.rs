//! Ghost-line renderer: badge pill, icons, themes, color-depth fallbacks and
//! the light-sweep / glow effects. Produces a styled line that is serialized
//! either to ANSI escapes (bash, fish, setup preview) or to zsh
//! `region_highlight` specs.

use std::fmt::Write as _;

use crate::config::{IconSet, ThemeDef};
use crate::schema::Level;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse(s: &str) -> Option<Rgb> {
        let h = s.trim().strip_prefix('#')?;
        if h.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    pub fn lerp(self, o: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb(m(self.0, o.0), m(self.1, o.1), m(self.2, o.2))
    }

    pub fn lighten(self, t: f32) -> Rgb {
        self.lerp(Rgb(255, 255, 255), t)
    }

    pub fn darken(self, t: f32) -> Rgb {
        self.lerp(Rgb(0, 0, 0), t)
    }

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
}

impl Style {
    pub fn fg(c: Rgb) -> Style {
        Style { fg: Some(c), ..Default::default() }
    }
    pub fn bold(mut self) -> Style {
        self.bold = true;
        self
    }
    pub fn italic(mut self) -> Style {
        self.italic = true;
        self
    }
    pub fn on(mut self, bg: Rgb) -> Style {
        self.bg = Some(bg);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Styled {
    pub spans: Vec<Span>,
}

impl Styled {
    fn push(&mut self, text: impl Into<String>, style: Style) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut() {
            if last.style == style {
                last.text.push_str(&text);
                return;
            }
        }
        self.spans.push(Span { text, style });
    }

    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.text.chars().count()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Cuts the line to `max` columns, ending with an ellipsis.
    fn truncate(&mut self, max: usize) {
        if max == 0 || self.width() <= max {
            return;
        }
        let mut left = max.saturating_sub(1);
        let mut out = Vec::new();
        for mut s in std::mem::take(&mut self.spans) {
            let n = s.text.chars().count();
            if n <= left {
                left -= n;
                out.push(s);
                continue;
            }
            s.text = s.text.chars().take(left).collect::<String>() + "\u{2026}";
            out.push(s);
            break;
        }
        self.spans = out;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColorDepth {
    None,
    Ansi16,
    Ansi256,
    True,
}

impl ColorDepth {
    /// From `NO_COLOR`, `COLORTERM` and `TERM`.
    pub fn detect(no_color: bool, colorterm: Option<&str>, term: Option<&str>) -> ColorDepth {
        if no_color {
            return ColorDepth::None;
        }
        if colorterm.is_some_and(|c| c.contains("truecolor") || c.contains("24bit")) {
            return ColorDepth::True;
        }
        match term {
            None | Some("dumb") => ColorDepth::None,
            Some(t) if t.contains("direct") => ColorDepth::True,
            Some(t) if t.contains("256") || t.contains("kitty") || t.contains("alacritty") || t.contains("foot") || t.contains("ghostty") || t.contains("wezterm") => {
                ColorDepth::Ansi256
            }
            Some(_) => ColorDepth::Ansi16,
        }
    }

    pub fn from_env() -> ColorDepth {
        let colorterm = std::env::var("COLORTERM").ok();
        let term = std::env::var("TERM").ok();
        ColorDepth::detect(std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()), colorterm.as_deref(), term.as_deref())
    }

    pub fn code(self) -> char {
        match self {
            ColorDepth::None => 'n',
            ColorDepth::Ansi16 => '1',
            ColorDepth::Ansi256 => '2',
            ColorDepth::True => 't',
        }
    }

    pub fn from_code(c: char) -> ColorDepth {
        match c {
            'n' => ColorDepth::None,
            '1' => ColorDepth::Ansi16,
            '2' => ColorDepth::Ansi256,
            _ => ColorDepth::True,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub known: Rgb,
    pub caution: Rgb,
    pub danger: Rgb,
    pub unknown: Rgb,
    pub analyzing: Rgb,
    pub badge_fg: Rgb,
    pub text: Rgb,
    pub dim: Rgb,
    pub highlight: Rgb,
}

pub const THEME_NAMES: [&str; 6] = ["midnight", "nord", "catppuccin", "gruvbox", "contrast", "mono"];

fn hex(s: &str) -> Rgb {
    Rgb::parse(s).unwrap_or_default()
}

impl Theme {
    pub fn builtin(name: &str) -> Option<Theme> {
        let t = |name: &str, k, c, d, u, a, bf, tx, dm, hl| Theme {
            name: name.to_string(),
            known: hex(k),
            caution: hex(c),
            danger: hex(d),
            unknown: hex(u),
            analyzing: hex(a),
            badge_fg: hex(bf),
            text: hex(tx),
            dim: hex(dm),
            highlight: hex(hl),
        };
        Some(match name {
            "midnight" => t("midnight", "#2dd4bf", "#fbbf24", "#f43f5e", "#64748b", "#a78bfa", "#0b1020", "#cbd5e1", "#64748b", "#ffffff"),
            "nord" => t("nord", "#88c0d0", "#ebcb8b", "#bf616a", "#4c566a", "#b48ead", "#2e3440", "#d8dee9", "#616e88", "#eceff4"),
            "catppuccin" => t("catppuccin", "#94e2d5", "#f9e2af", "#f38ba8", "#6c7086", "#cba6f7", "#1e1e2e", "#cdd6f4", "#7f849c", "#ffffff"),
            "gruvbox" => t("gruvbox", "#83a598", "#fabd2f", "#fb4934", "#665c54", "#d3869b", "#282828", "#ebdbb2", "#928374", "#fbf1c7"),
            // Okabe-Ito based: distinguishable with the common color-vision deficiencies.
            "contrast" => t("contrast", "#56b4e9", "#f0e442", "#ff5a1f", "#bbbbbb", "#cc79a7", "#000000", "#ffffff", "#c8c8c8", "#ffffff"),
            "mono" => t("mono", "#c8c8c8", "#e4e4e4", "#ffffff", "#808080", "#a0a0a0", "#000000", "#d0d0d0", "#8a8a8a", "#ffffff"),
            _ => return None,
        })
    }

    pub fn from_def(d: &ThemeDef) -> Theme {
        let base = Theme::builtin("midnight").unwrap();
        let or = |s: &str, f: Rgb| Rgb::parse(s).unwrap_or(f);
        Theme {
            name: d.name.clone(),
            known: or(&d.known, base.known),
            caution: or(&d.caution, base.caution),
            danger: or(&d.danger, base.danger),
            unknown: or(&d.unknown, base.unknown),
            analyzing: or(&d.analyzing, base.analyzing),
            badge_fg: or(&d.badge_fg, base.badge_fg),
            text: or(&d.text, base.text),
            dim: or(&d.dim, base.dim),
            highlight: or(&d.highlight, base.highlight),
        }
    }

    pub fn resolve(name: &str, user: &[ThemeDef]) -> Theme {
        user.iter()
            .find(|t| t.name == name)
            .map(Theme::from_def)
            .or_else(|| Theme::builtin(name))
            .unwrap_or_else(|| Theme::builtin("midnight").unwrap())
    }

    pub fn level_color(&self, l: Level) -> Rgb {
        match l {
            Level::Idle => self.dim,
            Level::Known => self.known,
            Level::Unknown => self.unknown,
            Level::Caution => self.caution,
            Level::Danger => self.danger,
        }
    }
}

pub struct Icons {
    pub known: &'static str,
    pub caution: &'static str,
    pub danger: &'static str,
    pub unknown: &'static str,
    pub idle: &'static str,
    pub ai: &'static str,
    pub root: &'static str,
    pub cap_l: &'static str,
    pub cap_r: &'static str,
    pub connector: &'static str,
    pub spinner: &'static [&'static str],
}

pub fn icons(set: IconSet) -> Icons {
    const BRAILLE: &[&str] = &["\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}", "\u{280f}"];
    match set {
        IconSet::Nerd => Icons {
            known: "\u{f0565}",   // md-shield_check
            caution: "\u{f0026}", // md-alert
            danger: "\u{f0bc6}",  // md-skull_crossbones
            unknown: "\u{f02d7}", // md-help_circle
            idle: "\u{f097b}",    // md-keyboard_outline
            ai: "\u{f167a}",      // md-robot_outline
            root: "\u{f01a5}",    // md-crown
            cap_l: "\u{e0b6}",
            cap_r: "\u{e0b4}",
            connector: "\u{2570}\u{2500} ",
            spinner: BRAILLE,
        },
        IconSet::Unicode => Icons {
            known: "\u{2713}",
            caution: "\u{25b2}",
            danger: "\u{2715}",
            unknown: "?",
            idle: "\u{2026}",
            ai: "\u{25c6}",
            root: "#",
            cap_l: "",
            cap_r: "",
            connector: "\u{2570}\u{2500} ",
            spinner: BRAILLE,
        },
        IconSet::Ascii => Icons {
            known: "ok",
            caution: "!",
            danger: "!!",
            unknown: "?",
            idle: "..",
            ai: "ai",
            root: "#",
            cap_l: "",
            cap_r: "",
            connector: "  ",
            spinner: &["|", "/", "-", "\\"],
        },
    }
}

impl Icons {
    pub fn level(&self, l: Level) -> &'static str {
        match l {
            Level::Idle => self.idle,
            Level::Known => self.known,
            Level::Unknown => self.unknown,
            Level::Caution => self.caution,
            Level::Danger => self.danger,
        }
    }
}

pub fn label(l: Level) -> &'static str {
    match l {
        Level::Idle => "TYPING",
        Level::Known => "KNOWN",
        Level::Unknown => "UNKNOWN",
        Level::Caution => "CAUTION",
        Level::Danger => "DANGER",
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum AiTag {
    #[default]
    None,
    /// Review requested, waiting for the debounce or the model.
    Pending,
    /// The AI agreed with Tier 1.
    Agree,
    /// The AI disagreed (or explained an unknown command).
    Reason(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct View {
    pub level: Level,
    pub text: String,
    pub target_span: Option<(usize, usize)>,
    pub root: bool,
    /// Unknown/complex command waiting for the AI: sweep the text.
    pub analyzing: bool,
    pub ai: AiTag,
    /// Frames since the AI changed the level (drives a sweep over the badge).
    pub changed_frames: Option<u32>,
}

pub struct Opts<'a> {
    pub theme: &'a Theme,
    pub icons: IconSet,
    pub depth: ColorDepth,
    pub frame: u32,
    pub motion: bool,
    pub screen_reader: bool,
    pub max_cols: usize,
}

const SWEEP_BAND: f32 = 5.0;
pub const CHANGE_SWEEP_FRAMES: u32 = 14;

fn sweep_intensity(i: usize, n: usize, frame: u32, speed: f32) -> f32 {
    let span = n as f32 + SWEEP_BAND * 2.0;
    let pos = (frame as f32 * speed) % span - SWEEP_BAND;
    let d = (i as f32 - pos).abs();
    (1.0 - d / SWEEP_BAND).max(0.0)
}

/// True if the line looks different on the next frame.
pub fn is_animated(v: &View, o: &Opts) -> bool {
    if !o.motion || o.screen_reader || o.depth < ColorDepth::Ansi256 {
        return false;
    }
    v.analyzing || v.ai == AiTag::Pending || v.level == Level::Danger || v.changed_frames.is_some_and(|f| f < CHANGE_SWEEP_FRAMES)
}

/// Renders the ghost line.
pub fn render(v: &View, o: &Opts) -> Styled {
    let mut out = Styled::default();
    if v.text.is_empty() && v.level == Level::Idle {
        return out;
    }
    if o.screen_reader {
        let mut s = format!("[{}] {}", label(v.level), v.text);
        if v.root {
            s.push_str(" (as root)");
        }
        match &v.ai {
            AiTag::Reason(r) => {
                let _ = write!(s, " AI: {r}");
            }
            AiTag::Pending => s.push_str(" (AI checking)"),
            _ => {}
        }
        out.push(s, Style::default());
        out.truncate(o.max_cols);
        return out;
    }

    let t = o.theme;
    let ic = icons(o.icons);
    let animate = o.motion && o.depth >= ColorDepth::Ansi256;
    let f = o.frame;
    let base = t.level_color(v.level);

    out.push(ic.connector, Style::fg(t.dim));

    if v.level == Level::Idle {
        out.push(format!("{} ", ic.idle), Style::fg(t.dim));
        out.push(v.text.clone(), Style::fg(t.dim).italic());
        out.truncate(o.max_cols);
        return out;
    }

    // Danger glow: the badge breathes between two shades of red.
    let pulse = if animate && v.level == Level::Danger {
        ((f as f32 * 0.45).sin() + 1.0) / 2.0
    } else {
        0.0
    };
    let badge_bg = if v.level == Level::Danger { base.lerp(base.lighten(0.3), pulse) } else { base };

    let icon = if v.analyzing && o.icons != IconSet::Ascii {
        ic.spinner[(f as usize) % ic.spinner.len()]
    } else if v.analyzing {
        ic.spinner[(f as usize / 2) % ic.spinner.len()]
    } else {
        ic.level(v.level)
    };
    let badge_text = format!(" {} {} ", icon, label(v.level));

    if !ic.cap_l.is_empty() {
        out.push(ic.cap_l, Style::fg(badge_bg));
    }
    let changing = animate && v.changed_frames.is_some_and(|c| c < CHANGE_SWEEP_FRAMES);
    if changing {
        let n = badge_text.chars().count();
        let cf = v.changed_frames.unwrap_or(0);
        for (i, ch) in badge_text.chars().enumerate() {
            let k = sweep_intensity(i, n, cf, (n as f32 + SWEEP_BAND * 2.0) / CHANGE_SWEEP_FRAMES as f32);
            out.push(ch.to_string(), Style::fg(t.badge_fg).on(badge_bg.lerp(t.highlight, k * 0.7)).bold());
        }
    } else {
        out.push(badge_text, Style::fg(t.badge_fg).on(badge_bg).bold());
    }
    if !ic.cap_r.is_empty() {
        out.push(ic.cap_r, Style::fg(badge_bg));
    }
    out.push(" ", Style::default());

    // Summary text, italic, with the target slot in bold.
    let text_color = match v.level {
        Level::Danger => base.lighten(0.25).lerp(base.lighten(0.55), pulse),
        Level::Caution => base.lighten(0.35),
        Level::Unknown => t.text,
        _ => t.text,
    };
    if v.analyzing && animate {
        let n = v.text.chars().count();
        for (i, ch) in v.text.chars().enumerate() {
            let k = sweep_intensity(i, n, f, 1.6);
            out.push(ch.to_string(), Style::fg(t.dim.lerp(t.highlight, k)).italic());
        }
    } else {
        let (a, b) = v.target_span.filter(|(a, b)| a <= b && *b <= v.text.len() && v.text.is_char_boundary(*a) && v.text.is_char_boundary(*b)).unwrap_or((v.text.len(), v.text.len()));
        let st = Style::fg(text_color).italic();
        out.push(&v.text[..a], st);
        out.push(&v.text[a..b], Style::fg(t.highlight).bold());
        out.push(&v.text[b..], st);
    }

    if v.root {
        out.push("  ", Style::default());
        out.push(format!("{} root", ic.root), Style::fg(t.caution).bold());
    }

    match &v.ai {
        AiTag::None => {}
        AiTag::Pending => {
            out.push("  ", Style::default());
            let c = if animate { t.dim.lerp(t.analyzing, ((f as f32 * 0.5).sin() + 1.0) / 2.0) } else { t.dim };
            out.push(ic.ai, Style::fg(c));
        }
        AiTag::Agree => {
            out.push("  ", Style::default());
            out.push(ic.ai, Style::fg(t.dim));
        }
        AiTag::Reason(r) => {
            out.push("  ", Style::default());
            out.push(format!("{} AI: ", ic.ai), Style::fg(t.analyzing).bold());
            out.push(r.clone(), Style::fg(t.analyzing.lerp(t.text, 0.35)).italic());
        }
        AiTag::Error(e) => {
            out.push("  ", Style::default());
            out.push(format!("{} {}", ic.ai, e), Style::fg(t.dim).italic());
        }
    }

    out.truncate(o.max_cols);
    out
}

// ---------- serialization ----------

const ANSI16: [(u8, Rgb); 16] = [
    (0, Rgb(0, 0, 0)),
    (1, Rgb(205, 49, 49)),
    (2, Rgb(13, 188, 121)),
    (3, Rgb(229, 229, 16)),
    (4, Rgb(36, 114, 200)),
    (5, Rgb(188, 63, 188)),
    (6, Rgb(17, 168, 205)),
    (7, Rgb(229, 229, 229)),
    (8, Rgb(102, 102, 102)),
    (9, Rgb(241, 76, 76)),
    (10, Rgb(35, 209, 139)),
    (11, Rgb(245, 245, 67)),
    (12, Rgb(59, 142, 234)),
    (13, Rgb(214, 112, 214)),
    (14, Rgb(41, 184, 219)),
    (15, Rgb(255, 255, 255)),
];

fn dist(a: Rgb, b: Rgb) -> i32 {
    let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
    d(a.0, b.0) * 3 + d(a.1, b.1) * 4 + d(a.2, b.2) * 2
}

pub fn to_16(c: Rgb) -> u8 {
    ANSI16.iter().min_by_key(|(_, p)| dist(c, *p)).map(|(i, _)| *i).unwrap_or(7)
}

pub fn to_256(c: Rgb) -> u8 {
    let q = |v: u8| -> u8 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            (v - 35) / 40
        }
    };
    let (r, g, b) = (q(c.0), q(c.1), q(c.2));
    let lv = [0u8, 95, 135, 175, 215, 255];
    let cube = Rgb(lv[r as usize], lv[g as usize], lv[b as usize]);
    let gray_avg = (c.0 as u16 + c.1 as u16 + c.2 as u16) / 3;
    let gi = if gray_avg > 238 { 23 } else { (gray_avg.saturating_sub(3) / 10) as u8 };
    let gv = 8 + 10 * gi;
    let gray = Rgb(gv, gv, gv);
    if dist(c, gray) < dist(c, cube) {
        232 + gi
    } else {
        16 + 36 * r + 6 * g + b
    }
}

fn sgr_color(buf: &mut String, c: Rgb, bg: bool, depth: ColorDepth) {
    match depth {
        ColorDepth::None => {}
        ColorDepth::True => {
            let _ = write!(buf, ";{};2;{};{};{}", if bg { 48 } else { 38 }, c.0, c.1, c.2);
        }
        ColorDepth::Ansi256 => {
            let _ = write!(buf, ";{};5;{}", if bg { 48 } else { 38 }, to_256(c));
        }
        ColorDepth::Ansi16 => {
            let n = to_16(c);
            let code = match (bg, n < 8) {
                (false, true) => 30 + n,
                (false, false) => 90 + n - 8,
                (true, true) => 40 + n,
                (true, false) => 100 + n - 8,
            };
            let _ = write!(buf, ";{code}");
        }
    }
}

/// Serializes to ANSI SGR escapes. Uses a single reusable buffer.
pub fn to_ansi_into(s: &Styled, depth: ColorDepth, buf: &mut String) {
    buf.clear();
    for sp in &s.spans {
        if depth == ColorDepth::None {
            if sp.style.bold {
                buf.push_str("\x1b[0;1m");
            } else {
                buf.push_str("\x1b[0m");
            }
        } else {
            buf.push_str("\x1b[0");
            if sp.style.bold {
                buf.push_str(";1");
            }
            if sp.style.italic {
                buf.push_str(";3");
            }
            if let Some(c) = sp.style.fg {
                sgr_color(buf, c, false, depth);
            }
            if let Some(c) = sp.style.bg {
                sgr_color(buf, c, true, depth);
            }
            buf.push('m');
        }
        buf.push_str(&sp.text);
    }
    if !s.spans.is_empty() {
        buf.push_str("\x1b[0m");
    }
}

pub fn to_ansi(s: &Styled, depth: ColorDepth) -> String {
    let mut b = String::with_capacity(256);
    to_ansi_into(s, depth, &mut b);
    b
}

fn zsh_color(c: Rgb, depth: ColorDepth) -> String {
    match depth {
        ColorDepth::True => c.hex(),
        ColorDepth::Ansi256 => to_256(c).to_string(),
        _ => to_16(c).to_string(),
    }
}

/// Serializes for zsh: plain text plus `start end style` specs (character
/// offsets relative to the text) joined by `|`.
///
/// Some zsh builds treat the Powerline round caps (U+E0B6, U+E0B4, in the
/// BMP private use area) as unprintable and then miscount the line width, so
/// they become half blocks of the same width here.
pub fn to_zsh(s: &Styled, depth: ColorDepth, italic: bool) -> (String, String) {
    let mut text = String::new();
    let mut specs = String::new();
    let mut pos = 0usize;
    for sp in &s.spans {
        let clean: String = sp
            .text
            .chars()
            .map(|c| match c {
                '\t' | '\n' | '|' => ' ',
                '\u{e0b6}' => '\u{2590}',
                '\u{e0b4}' => '\u{258c}',
                c => c,
            })
            .collect();
        let n = clean.chars().count();
        text.push_str(&clean);
        let mut st = Vec::new();
        if depth != ColorDepth::None {
            if let Some(c) = sp.style.fg {
                st.push(format!("fg={}", zsh_color(c, depth)));
            }
            if let Some(c) = sp.style.bg {
                st.push(format!("bg={}", zsh_color(c, depth)));
            }
        }
        if sp.style.bold {
            st.push("bold".into());
        }
        if sp.style.italic && italic {
            st.push("italic".into());
        }
        if !st.is_empty() && n > 0 {
            if !specs.is_empty() {
                specs.push('|');
            }
            let _ = write!(specs, "{} {} {}", pos, pos + n, st.join(","));
        }
        pos += n;
    }
    (text, specs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(theme: &Theme) -> Opts<'_> {
        Opts { theme, icons: IconSet::Nerd, depth: ColorDepth::True, frame: 0, motion: true, screen_reader: false, max_cols: 80 }
    }

    #[test]
    fn renders_badge_and_text() {
        let th = Theme::builtin("midnight").unwrap();
        let v = View { level: Level::Danger, text: "Deletes \"build\" forever.".into(), target_span: Some((8, 15)), ..Default::default() };
        let s = render(&v, &opts(&th));
        let p = s.plain();
        assert!(p.contains("DANGER"));
        assert!(p.contains("Deletes \"build\" forever."));
        assert!(to_ansi(&s, ColorDepth::True).contains("\x1b[0;1"));
    }

    #[test]
    fn screen_reader_is_plain() {
        let th = Theme::builtin("contrast").unwrap();
        let mut o = opts(&th);
        o.screen_reader = true;
        let v = View { level: Level::Caution, text: "Overwrites x.".into(), ..Default::default() };
        assert_eq!(render(&v, &o).plain(), "[CAUTION] Overwrites x.");
    }

    #[test]
    fn truncates() {
        let th = Theme::builtin("nord").unwrap();
        let mut o = opts(&th);
        o.max_cols = 20;
        let v = View { level: Level::Known, text: "Lists every file in this very long folder name".into(), ..Default::default() };
        assert!(render(&v, &o).width() <= 20);
    }

    #[test]
    fn color_downsampling() {
        assert_eq!(to_256(Rgb(255, 0, 0)), 196);
        assert_eq!(to_16(Rgb(250, 70, 70)), 9);
    }

    #[test]
    fn zsh_specs() {
        let th = Theme::builtin("gruvbox").unwrap();
        let v = View { level: Level::Known, text: "Lists files.".into(), ..Default::default() };
        let (text, specs) = to_zsh(&render(&v, &opts(&th)), ColorDepth::True, false);
        assert!(text.contains("KNOWN"));
        assert!(specs.contains("fg=#"));
    }
}
