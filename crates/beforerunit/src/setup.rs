//! `beforerunit setup`: the settings app.

use std::io::IsTerminal;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use beforerunit_ai::{Client, PRESETS};
use beforerunit_core::config::{AiMode, Config, DisplayMode, IconSet, ThemeDef};
use beforerunit_core::ghost::{build_view, AiStatus};
use beforerunit_core::merge::{AiVerdict, Merged};
use beforerunit_core::render::{self, label, ColorDepth, Opts, Rgb, Styled, Theme, View, THEME_NAMES};
use beforerunit_core::rules::{self, Cmd};
use beforerunit_core::{analyze, store, Level};
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use tui_big_text::{BigText, PixelSize};

use crate::keys;

const FRAME: Duration = Duration::from_millis(66);
const SPIN: [&str; 10] = ["\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}", "\u{280f}"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Ai,
    Look,
    Rules,
    About,
}

const TABS: [(Tab, &str, &str); 4] = [
    (Tab::Ai, "\u{f167a}", "AI review"),
    (Tab::Look, "\u{f03d8}", "Appearance"),
    (Tab::Rules, "\u{f0219}", "Rule book"),
    (Tab::About, "\u{f02fd}", "About"),
];

enum Msg {
    Models(Result<Vec<String>, String>),
    Test(Result<(u128, AiVerdict), String>),
}

struct Edit {
    buf: String,
}

struct App {
    cfg: Config,
    saved_cfg: Config,
    user_themes: Vec<ThemeDef>,
    key_input: Option<String>,
    key_stored: Option<(String, keys::Where)>,
    tab: Tab,
    focus: usize,
    edit: Option<Edit>,
    models: Vec<String>,
    picker: Option<ListState>,
    status: Option<(String, Level, Instant)>,
    busy: Option<(String, Instant)>,
    last_test: Option<Result<(u128, AiVerdict), String>>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    frame: u32,
    started: Instant,
    rules_all: Vec<&'static Cmd>,
    rules_query: String,
    rules_state: ListState,
    try_line: String,
    rules_input: RulesInput,
    quit_armed: bool,
    done: bool,
    saved_any: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RulesInput {
    List,
    Search,
    Try,
}

const AI_FIELDS: usize = 8;
const LOOK_FIELDS: usize = 5;

pub fn run() -> i32 {
    if !std::io::stdout().is_terminal() {
        eprintln!("beforerunit setup needs a terminal. Edit {} by hand instead.", store::config_file().display());
        return 1;
    }
    let compiled = store::load_or_compile();
    beforerunit_core::apply(&compiled);
    let (tx, rx) = mpsc::channel();
    let mut rules_all = rules::all_cmds();
    rules_all.sort_by_key(|c| c.name);
    let mut app = App {
        cfg: compiled.config.clone(),
        saved_cfg: compiled.config.clone(),
        user_themes: compiled.themes.clone(),
        key_input: None,
        key_stored: keys::get(),
        tab: Tab::Ai,
        focus: 0,
        edit: None,
        models: Vec::new(),
        picker: None,
        status: None,
        busy: None,
        last_test: None,
        tx,
        rx,
        frame: 0,
        started: Instant::now(),
        rules_all,
        rules_query: String::new(),
        rules_state: ListState::default().with_selected(Some(0)),
        try_line: "rm -rf build".into(),
        rules_input: RulesInput::List,
        quit_armed: false,
        done: false,
        saved_any: false,
    };
    if !compiled.errors.is_empty() {
        app.flash(&format!("Config error: {}", compiled.errors[0]), Level::Danger);
    }
    let mut term = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste);
    let res = app.run(&mut term);
    let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    if let Err(e) = res {
        eprintln!("beforerunit setup: {e}");
        return 1;
    }
    if app.saved_any {
        println!("Settings saved to {}", store::config_file().display());
        println!("New shells pick them up right away. Not hooked up yet? Add this to your shell config:");
        println!("  bash:  eval \"$(beforerunit init bash)\"");
        println!("  zsh:   eval \"$(beforerunit init zsh)\"");
        println!("  fish:  beforerunit init fish | source");
    }
    0
}

// ---------------------------------------------------------------------------
// Helpers

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

fn to_line(s: &Styled) -> Line<'static> {
    Line::from(
        s.spans
            .iter()
            .map(|sp| {
                let mut st = Style::default();
                if let Some(c) = sp.style.fg {
                    st = st.fg(rgb(c));
                }
                if let Some(c) = sp.style.bg {
                    st = st.bg(rgb(c));
                }
                if sp.style.bold {
                    st = st.add_modifier(Modifier::BOLD);
                }
                if sp.style.italic {
                    st = st.add_modifier(Modifier::ITALIC);
                }
                Span::styled(sp.text.clone(), st)
            })
            .collect::<Vec<_>>(),
    )
}

fn cycle<T: Copy + PartialEq>(all: &[T], cur: T, back: bool) -> T {
    let i = all.iter().position(|x| *x == cur).unwrap_or(0);
    let n = all.len();
    all[if back { (i + n - 1) % n } else { (i + 1) % n }]
}

impl App {
    fn theme(&self) -> Theme {
        Theme::resolve(&self.cfg.ui.theme, &self.user_themes)
    }

    fn flash(&mut self, msg: &str, level: Level) {
        self.status = Some((msg.to_string(), level, Instant::now()));
    }

    fn dirty(&self) -> bool {
        self.cfg != self.saved_cfg || self.key_input.is_some()
    }

    fn opts<'a>(&self, theme: &'a Theme, cols: usize) -> Opts<'a> {
        Opts {
            theme,
            icons: self.cfg.ui.icons,
            depth: ColorDepth::True,
            frame: self.frame,
            motion: !self.cfg.ui.reduce_motion,
            screen_reader: self.cfg.ui.screen_reader,
            max_cols: cols,
        }
    }

    fn api_key(&self) -> Option<String> {
        match &self.key_input {
            Some(k) if k.is_empty() => None,
            Some(k) => Some(k.clone()),
            None => self.key_stored.as_ref().map(|(k, _)| k.clone()),
        }
    }

    fn run(&mut self, term: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.done {
            while let Ok(m) = self.rx.try_recv() {
                self.on_msg(m);
            }
            term.draw(|f| self.draw(f))?;
            if event::poll(FRAME)? {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
                    Event::Paste(s) => self.on_paste(&s),
                    _ => {}
                }
            }
            self.frame = self.frame.wrapping_add(1);
        }
        Ok(())
    }

    fn on_msg(&mut self, m: Msg) {
        self.busy = None;
        match m {
            Msg::Models(Ok(list)) => {
                if list.is_empty() {
                    self.flash("The server returned no models. Type the model name instead.", Level::Caution);
                } else {
                    let sel = list.iter().position(|m| *m == self.cfg.ai.model).unwrap_or(0);
                    self.flash(&format!("Found {} models. Pick one with Enter.", list.len()), Level::Known);
                    self.models = list;
                    self.picker = Some(ListState::default().with_selected(Some(sel)));
                }
            }
            Msg::Models(Err(e)) => self.flash(&e, Level::Danger),
            Msg::Test(r) => {
                match &r {
                    Ok((ms, _)) => self.flash(&format!("Connected in {ms} ms. Save with Ctrl-S."), Level::Known),
                    Err(e) => self.flash(e, Level::Danger),
                }
                self.last_test = Some(r);
            }
        }
    }

    fn spawn_ai<F, Fut>(&mut self, what: &str, job: F)
    where
        F: FnOnce(Client) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Msg>,
    {
        if self.busy.is_some() {
            return;
        }
        if self.cfg.ai.base_url.trim().is_empty() {
            self.flash("Set a base URL first.", Level::Caution);
            return;
        }
        self.busy = Some((what.to_string(), Instant::now()));
        let ep = crate::context::endpoint(&self.cfg.ai, self.api_key());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
            let msg = rt.block_on(async move {
                match Client::new(ep) {
                    Ok(c) => job(c).await,
                    Err(e) => Msg::Test(Err(e.explain())),
                }
            });
            let _ = tx.send(msg);
        });
    }

    fn fetch_models(&mut self) {
        self.spawn_ai("Fetching models", |c| async move { Msg::Models(c.list_models().await.map_err(|e| e.explain())) });
    }

    fn test(&mut self) {
        if self.cfg.ai.model.trim().is_empty() {
            self.flash("Pick a model first (Fetch models, or type one).", Level::Caution);
            return;
        }
        self.spawn_ai("Testing connection", |c| async move {
            let r = c.test().await;
            Msg::Test(match r.verdict {
                Ok(v) => Ok((r.latency_ms, v)),
                Err(e) => Err(e.explain()),
            })
        });
    }

    fn save(&mut self) {
        if let Err(e) = store::write_config(&self.cfg) {
            self.flash(&format!("Cannot save: {e}"), Level::Danger);
            return;
        }
        let mut msg = "Saved.".to_string();
        if let Some(k) = self.key_input.take() {
            if k.is_empty() {
                keys::delete();
                msg.push_str(" API key removed.");
            } else {
                match keys::set(&k) {
                    Ok(w) => msg.push_str(&format!(" Key stored in the {}.", w.describe())),
                    Err(e) => msg = format!("Saved, but the key could not be stored: {e}"),
                }
            }
            self.key_stored = keys::get();
        }
        let c = store::compile();
        let _ = store::save(&c);
        let _ = beforerunit_core::proto::send("RELOAD");
        self.saved_cfg = self.cfg.clone();
        self.saved_any = true;
        self.flash(&msg, Level::Known);
    }

    // -----------------------------------------------------------------------
    // Input

    fn on_paste(&mut self, s: &str) {
        let s = s.trim_end_matches(['\n', '\r']);
        if let Some(e) = &mut self.edit {
            e.buf.push_str(s);
        } else if self.tab == Tab::Rules {
            match self.rules_input {
                RulesInput::Search => self.rules_query.push_str(s),
                _ => {
                    self.rules_input = RulesInput::Try;
                    self.try_line.push_str(s);
                }
            }
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            self.done = true;
            return;
        }
        if ctrl && k.code == KeyCode::Char('s') {
            self.commit_edit();
            self.save();
            return;
        }
        if self.picker.is_some() {
            self.on_picker_key(k);
            return;
        }
        if self.edit.is_some() {
            self.on_edit_key(k);
            return;
        }
        if self.tab == Tab::Rules && self.rules_input != RulesInput::List && self.on_rules_text_key(k) {
            return;
        }
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.dirty() && !self.quit_armed {
                    self.quit_armed = true;
                    self.flash("Unsaved changes. Press q again to quit, Ctrl-S to save.", Level::Caution);
                } else {
                    self.done = true;
                }
                return;
            }
            KeyCode::Tab => self.switch_tab(false),
            KeyCode::BackTab => self.switch_tab(true),
            KeyCode::Char(c @ '1'..='4') => {
                self.tab = TABS[(c as u8 - b'1') as usize].0;
                self.focus = 0;
            }
            _ => match self.tab {
                Tab::Ai => self.on_ai_key(k),
                Tab::Look => self.on_look_key(k),
                Tab::Rules => self.on_rules_key(k),
                Tab::About => {}
            },
        }
        self.quit_armed = false;
    }

    fn switch_tab(&mut self, back: bool) {
        let all: Vec<Tab> = TABS.iter().map(|t| t.0).collect();
        self.tab = cycle(&all, self.tab, back);
        self.focus = 0;
    }

    fn move_focus(&mut self, k: KeyCode, n: usize) {
        match k {
            KeyCode::Up | KeyCode::Char('k') => self.focus = (self.focus + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => self.focus = (self.focus + 1) % n,
            _ => {}
        }
    }

    fn start_edit(&mut self, initial: &str) {
        self.edit = Some(Edit { buf: initial.to_string() });
    }

    fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let v = e.buf.trim().to_string();
        match (self.tab, self.focus) {
            (Tab::Ai, 1) => {
                self.cfg.ai.base_url = v;
                self.cfg.ai.provider = PRESETS.iter().find(|p| p.base_url == self.cfg.ai.base_url).map(|p| p.id).unwrap_or("custom").into();
            }
            (Tab::Ai, 2) => self.key_input = Some(v),
            (Tab::Ai, 3) => self.cfg.ai.model = v,
            (Tab::Ai, 5) => match v.parse::<u64>() {
                Ok(ms) if (500..=60_000).contains(&ms) => self.cfg.ai.timeout_ms = ms,
                _ => self.flash("Timeout must be between 500 and 60000 ms.", Level::Caution),
            },
            _ => {}
        }
    }

    fn on_edit_key(&mut self, k: KeyEvent) {
        let Some(e) = &mut self.edit else { return };
        match k.code {
            KeyCode::Enter | KeyCode::Tab => self.commit_edit(),
            KeyCode::Esc => self.edit = None,
            KeyCode::Backspace => {
                e.buf.pop();
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => e.buf.clear(),
            KeyCode::Char(c) => e.buf.push(c),
            _ => {}
        }
    }

    fn apply_preset(&mut self, back: bool) {
        let ids: Vec<&str> = PRESETS.iter().map(|p| p.id).collect();
        let cur = ids.iter().position(|i| *i == self.cfg.ai.provider).unwrap_or(ids.len() - 1);
        let next = if back { (cur + ids.len() - 1) % ids.len() } else { (cur + 1) % ids.len() };
        let p = &PRESETS[next];
        let old_model_is_suggested = self.cfg.ai.model.is_empty() || PRESETS.iter().any(|q| q.suggested_model == self.cfg.ai.model);
        self.cfg.ai.provider = p.id.into();
        if !p.base_url.is_empty() {
            self.cfg.ai.base_url = p.base_url.into();
        }
        if old_model_is_suggested {
            self.cfg.ai.model = p.suggested_model.into();
        }
        self.models.clear();
        self.last_test = None;
    }

    fn on_ai_key(&mut self, k: KeyEvent) {
        let back = matches!(k.code, KeyCode::Left | KeyCode::Char('h'));
        let act = matches!(k.code, KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l'));
        match k.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('k') => self.move_focus(k.code, AI_FIELDS),
            KeyCode::Char('f') => self.fetch_models(),
            KeyCode::Char('t') => self.test(),
            _ if act => match self.focus {
                0 => self.apply_preset(back),
                1 => self.start_edit(&self.cfg.ai.base_url.clone()),
                2 => self.start_edit(""),
                3 => {
                    if self.models.is_empty() || k.code == KeyCode::Enter && k.modifiers.contains(KeyModifiers::SHIFT) {
                        self.start_edit(&self.cfg.ai.model.clone());
                    } else {
                        let sel = self.models.iter().position(|m| *m == self.cfg.ai.model).unwrap_or(0);
                        self.picker = Some(ListState::default().with_selected(Some(sel)));
                    }
                }
                4 => self.cfg.ai.mode = cycle(&[AiMode::ReviewAll, AiMode::UnknownOnly, AiMode::Off], self.cfg.ai.mode, back),
                5 => self.start_edit(&self.cfg.ai.timeout_ms.to_string()),
                6 => self.fetch_models(),
                7 => self.test(),
                _ => {}
            },
            KeyCode::Char('e') if self.focus == 3 => self.start_edit(&self.cfg.ai.model.clone()),
            KeyCode::Delete if self.focus == 2 => {
                self.key_input = Some(String::new());
                self.flash("The stored key will be removed on save.", Level::Caution);
            }
            _ => {}
        }
    }

    fn on_picker_key(&mut self, k: KeyEvent) {
        let n = self.models.len().max(1);
        let Some(st) = &mut self.picker else { return };
        let i = st.selected().unwrap_or(0);
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => st.select(Some((i + n - 1) % n)),
            KeyCode::Down | KeyCode::Char('j') => st.select(Some((i + 1) % n)),
            KeyCode::PageUp => st.select(Some(i.saturating_sub(10))),
            KeyCode::PageDown => st.select(Some((i + 10).min(n - 1))),
            KeyCode::Enter => {
                if let Some(m) = self.models.get(i) {
                    self.cfg.ai.model = m.clone();
                }
                self.picker = None;
            }
            KeyCode::Esc | KeyCode::Char('q') => self.picker = None,
            _ => {}
        }
    }

    fn theme_names(&self) -> Vec<String> {
        let mut v: Vec<String> = THEME_NAMES.iter().map(|s| s.to_string()).collect();
        v.extend(self.user_themes.iter().map(|t| t.name.clone()));
        v
    }

    fn on_look_key(&mut self, k: KeyEvent) {
        let back = matches!(k.code, KeyCode::Left | KeyCode::Char('h'));
        let act = matches!(k.code, KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l'));
        match k.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('k') => self.move_focus(k.code, LOOK_FIELDS),
            _ if act => match self.focus {
                0 => {
                    let names = self.theme_names();
                    let i = names.iter().position(|n| *n == self.cfg.ui.theme).unwrap_or(0);
                    let n = names.len();
                    self.cfg.ui.theme = names[if back { (i + n - 1) % n } else { (i + 1) % n }].clone();
                }
                1 => self.cfg.ui.icons = cycle(&[IconSet::Nerd, IconSet::Unicode, IconSet::Ascii], self.cfg.ui.icons, back),
                2 => self.cfg.ui.display = cycle(&[DisplayMode::Always, DisplayMode::RiskyOnly, DisplayMode::Off], self.cfg.ui.display, back),
                3 => self.cfg.ui.reduce_motion = !self.cfg.ui.reduce_motion,
                4 => self.cfg.ui.screen_reader = !self.cfg.ui.screen_reader,
                _ => {}
            },
            _ => {}
        }
    }

    fn filtered_rules(&self) -> Vec<&'static Cmd> {
        let q = self.rules_query.trim().to_lowercase();
        let first = q.split_whitespace().next().unwrap_or("");
        self.rules_all
            .iter()
            .copied()
            .filter(|c| q.is_empty() || c.name.contains(&q) || c.name == first || c.category.contains(&q) || c.default.text.to_lowercase().contains(&q))
            .collect()
    }

    fn on_rules_key(&mut self, k: KeyEvent) {
        let n = self.filtered_rules().len().max(1);
        let i = self.rules_state.selected().unwrap_or(0).min(n - 1);
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.rules_state.select(Some((i + n - 1) % n)),
            KeyCode::Down | KeyCode::Char('j') => self.rules_state.select(Some((i + 1) % n)),
            KeyCode::PageUp => self.rules_state.select(Some(i.saturating_sub(10))),
            KeyCode::PageDown => self.rules_state.select(Some((i + 10).min(n - 1))),
            KeyCode::Char('/') => self.rules_input = RulesInput::Search,
            KeyCode::Char('t') | KeyCode::Enter => self.rules_input = RulesInput::Try,
            _ => {}
        }
    }

    /// Returns true if the key was consumed by a text box.
    fn on_rules_text_key(&mut self, k: KeyEvent) -> bool {
        let buf = match self.rules_input {
            RulesInput::Search => &mut self.rules_query,
            RulesInput::Try => &mut self.try_line,
            RulesInput::List => return false,
        };
        match k.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Down | KeyCode::Up => {
                self.rules_input = RulesInput::List;
                return k.code == KeyCode::Esc || k.code == KeyCode::Enter;
            }
            KeyCode::Tab | KeyCode::BackTab => return false,
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => buf.clear(),
            KeyCode::Char(c) => buf.push(c),
            _ => return true,
        }
        if self.rules_input == RulesInput::Search {
            self.rules_state.select(Some(0));
        }
        true
    }

    // -----------------------------------------------------------------------
    // Drawing

    fn draw(&mut self, f: &mut Frame) {
        let theme = self.theme();
        let area = f.area();
        let big = area.height >= 30 && area.width >= 60;
        let [head, tabs, body, foot] =
            Layout::vertical([Constraint::Length(if big { 5 } else { 2 }), Constraint::Length(1), Constraint::Min(8), Constraint::Length(2)]).areas(area);
        self.draw_title(f, head, &theme, big);
        self.draw_tabs(f, tabs, &theme);
        match self.tab {
            Tab::Ai => self.draw_ai(f, body, &theme),
            Tab::Look => self.draw_look(f, body, &theme),
            Tab::Rules => self.draw_rules(f, body, &theme),
            Tab::About => self.draw_about(f, body, &theme),
        }
        self.draw_footer(f, foot, &theme);
        if self.picker.is_some() {
            self.draw_picker(f, area, &theme);
        }
    }

    /// Gradient title with a light sweep passing over it every few seconds.
    fn title_line(&self, text: &str, theme: &Theme) -> Line<'static> {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len().max(1) as f32;
        let motion = !self.cfg.ui.reduce_motion;
        let pos = (self.frame as f32 * 0.35) % (n + 12.0) - 6.0;
        Line::from(
            chars
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let t = i as f32 / (n - 1.0).max(1.0);
                    let base = if t < 0.5 { theme.known.lerp(theme.analyzing, t * 2.0) } else { theme.analyzing.lerp(theme.caution, (t - 0.5) * 2.0) };
                    let glow = if motion { (1.0 - (i as f32 - pos).abs() / 3.0).max(0.0) } else { 0.0 };
                    let col = base.lerp(Rgb(255, 255, 255), glow * 0.7);
                    Span::styled(c.to_string(), Style::default().fg(rgb(col)).add_modifier(Modifier::BOLD))
                })
                .collect::<Vec<_>>(),
        )
    }

    fn draw_title(&self, f: &mut Frame, area: Rect, theme: &Theme, big: bool) {
        if big {
            let [art, sub] = Layout::vertical([Constraint::Length(4), Constraint::Length(1)]).areas(area);
            let bt = BigText::builder().pixel_size(PixelSize::Quadrant).lines(vec![self.title_line("BeforeRunIt", theme)]).centered().build();
            f.render_widget(bt, art);
            f.render_widget(
                Paragraph::new(Line::from(Span::styled("Know what a command does before you press Enter", Style::default().fg(rgb(theme.dim)))))
                    .alignment(Alignment::Center),
                sub,
            );
        } else {
            let mut l = self.title_line("BeforeRunIt", theme);
            l.spans.push(Span::styled("  settings", Style::default().fg(rgb(theme.dim))));
            f.render_widget(Paragraph::new(l).alignment(Alignment::Center), area);
        }
    }

    fn draw_tabs(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        let mut spans = Vec::new();
        for (i, (t, icon, name)) in TABS.iter().enumerate() {
            let on = *t == self.tab;
            let st = if on {
                Style::default().fg(rgb(theme.badge_fg)).bg(rgb(theme.analyzing)).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(rgb(theme.dim))
            };
            let icon = if self.cfg.ui.icons == IconSet::Nerd { format!("{icon} ") } else { String::new() };
            spans.push(Span::styled(format!(" {}{} {} ", icon, i + 1, name), st));
            spans.push(Span::raw(" "));
        }
        f.render_widget(Paragraph::new(Line::from(spans)).alignment(Alignment::Center), area);
    }

    fn block<'a>(&self, title: &'a str, theme: &Theme) -> Block<'a> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(rgb(theme.dim)))
            .title(Span::styled(format!(" {title} "), Style::default().fg(rgb(theme.text)).add_modifier(Modifier::BOLD)))
    }

    fn field_line(&self, theme: &Theme, i: usize, name: &str, value: Line<'static>, hint: &str) -> Line<'static> {
        let focused = self.focus == i;
        let marker = if focused { "\u{258c} " } else { "  " };
        let name_style = if focused { Style::default().fg(rgb(theme.analyzing)).add_modifier(Modifier::BOLD) } else { Style::default().fg(rgb(theme.dim)) };
        let mut spans = vec![Span::styled(marker, Style::default().fg(rgb(theme.analyzing))), Span::styled(format!("{name:<14}"), name_style)];
        if focused && self.edit.is_some() {
            let buf = &self.edit.as_ref().unwrap().buf;
            let shown = if i == 2 { "\u{2022}".repeat(buf.chars().count()) } else { buf.clone() };
            let cursor = if self.frame / 8 % 2 == 0 { "\u{2588}" } else { " " };
            spans.push(Span::styled(shown, Style::default().fg(rgb(theme.text)).add_modifier(Modifier::UNDERLINED)));
            spans.push(Span::styled(cursor, Style::default().fg(rgb(theme.analyzing))));
        } else {
            spans.extend(value.spans);
            if focused && !hint.is_empty() {
                spans.push(Span::styled(format!("   {hint}"), Style::default().fg(rgb(theme.dim)).add_modifier(Modifier::ITALIC)));
            }
        }
        Line::from(spans)
    }

    fn val(&self, theme: &Theme, s: &str) -> Line<'static> {
        Line::from(Span::styled(s.to_string(), Style::default().fg(rgb(theme.text))))
    }

    fn choice(&self, theme: &Theme, all: &[&str], cur: &str) -> Line<'static> {
        let mut spans = Vec::new();
        for a in all {
            if *a == cur {
                spans.push(Span::styled(format!(" {a} "), Style::default().fg(rgb(theme.badge_fg)).bg(rgb(theme.analyzing)).add_modifier(Modifier::BOLD)));
            } else {
                spans.push(Span::styled(format!(" {a} "), Style::default().fg(rgb(theme.dim))));
            }
        }
        Line::from(spans)
    }

    /// One value with arrows, for lists too long to show side by side.
    fn cycle(&self, theme: &Theme, all: &[&str], cur: &str) -> Line<'static> {
        let pos = all.iter().position(|a| *a == cur).map(|i| format!("  {}/{}", i + 1, all.len())).unwrap_or_default();
        let arrow = Style::default().fg(rgb(theme.analyzing));
        Line::from(vec![
            Span::styled("\u{2039} ", arrow),
            Span::styled(format!(" {cur} "), Style::default().fg(rgb(theme.badge_fg)).bg(rgb(theme.analyzing)).add_modifier(Modifier::BOLD)),
            Span::styled(" \u{203a}", arrow),
            Span::styled(pos, Style::default().fg(rgb(theme.dim))),
        ])
    }

    fn button(&self, theme: &Theme, i: usize, label: &str) -> Line<'static> {
        let focused = self.focus == i;
        let busy = self.busy.as_ref().filter(|(w, _)| label.starts_with(&w[..4]));
        let text = match busy {
            Some((w, t)) => format!(" {} {w}... ", SPIN[(t.elapsed().as_millis() / 80) as usize % SPIN.len()]),
            None => format!(" {label} "),
        };
        let st = if focused {
            Style::default().fg(rgb(theme.badge_fg)).bg(rgb(theme.analyzing)).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(rgb(theme.analyzing)).add_modifier(Modifier::BOLD)
        };
        let marker = if focused { "\u{258c} " } else { "  " };
        Line::from(vec![Span::styled(marker, Style::default().fg(rgb(theme.analyzing))), Span::styled(text, st)])
    }

    fn draw_ai(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        let [left, right] = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(area);
        let ai = &self.cfg.ai;
        let preset = PRESETS.iter().find(|p| p.id == ai.provider);
        let provider_names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
        let key_val = match (&self.key_input, &self.key_stored) {
            (Some(k), _) if k.is_empty() => Line::from(Span::styled("will be removed", Style::default().fg(rgb(theme.caution)))),
            (Some(k), _) => Line::from(vec![
                Span::styled(keys::mask(k), Style::default().fg(rgb(theme.text))),
                Span::styled("  (unsaved)", Style::default().fg(rgb(theme.caution))),
            ]),
            (None, Some((k, w))) => Line::from(vec![
                Span::styled(keys::mask(k), Style::default().fg(rgb(theme.text))),
                Span::styled(
                    format!(
                        "  {}",
                        match w {
                            keys::Where::Keyring => "keyring",
                            keys::Where::Env => "env",
                            keys::Where::File => "0600 file",
                        }
                    ),
                    Style::default().fg(rgb(theme.dim)),
                ),
            ]),
            (None, None) => Line::from(Span::styled(
                if preset.is_some_and(|p| !p.needs_key) { "not needed" } else { "not set" },
                Style::default().fg(rgb(theme.dim)),
            )),
        };
        let model_val = if ai.model.is_empty() {
            Line::from(Span::styled("not set", Style::default().fg(rgb(theme.dim))))
        } else {
            self.val(theme, &ai.model)
        };
        let lines = vec![
            Line::raw(""),
            self.field_line(theme, 0, "Provider", self.cycle(theme, &provider_names, preset.map(|p| p.name).unwrap_or("Custom")), "Left/Right"),
            self.field_line(theme, 1, "Base URL", self.val(theme, if ai.base_url.is_empty() { "not set" } else { &ai.base_url }), "Enter to edit"),
            self.field_line(theme, 2, "API key", key_val, "Enter to type, Del to remove"),
            self.field_line(theme, 3, "Model", model_val, if self.models.is_empty() { "Enter to edit, f to fetch list" } else { "Enter to pick, e to type" }),
            self.field_line(theme, 4, "Review", self.choice(theme, &["review-all", "unknown-only", "off"], ai.mode.as_str()), "Left/Right"),
            self.field_line(theme, 5, "Timeout", self.val(theme, &format!("{} ms", ai.timeout_ms)), "Enter to edit"),
            Line::raw(""),
            self.button(theme, 6, "Fetch models"),
            self.button(theme, 7, "Test connection"),
        ];
        f.render_widget(Paragraph::new(lines).block(self.block("AI provider", theme)), left);

        let mut info: Vec<Line> = Vec::new();
        let dim = Style::default().fg(rgb(theme.dim));
        let txt = Style::default().fg(rgb(theme.text));
        match &self.last_test {
            Some(Ok((ms, v))) => {
                info.push(Line::from(vec![
                    Span::styled("\u{f05e0} ", Style::default().fg(rgb(theme.known))),
                    Span::styled(format!("Connected in {ms} ms"), txt.add_modifier(Modifier::BOLD)),
                ]));
                info.push(Line::from(vec![
                    Span::styled("Sample  ", dim),
                    Span::styled("rm -rf build", txt),
                ]));
                info.push(Line::from(vec![
                    Span::styled("Verdict ", dim),
                    Span::styled(label(v.level), Style::default().fg(rgb(theme.level_color(v.level))).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" {:.2}  {}", v.confidence, v.reason), txt),
                ]));
                info.push(Line::raw(""));
            }
            Some(Err(e)) => {
                info.push(Line::from(vec![Span::styled("\u{f0159} ", Style::default().fg(rgb(theme.danger))), Span::styled(e.clone(), txt)]));
                info.push(Line::raw(""));
            }
            None => {}
        }
        let effective = ai.effective_mode();
        info.push(Line::from(vec![
            Span::styled("Status  ", dim),
            Span::styled(
                if effective == AiMode::Off { "AI off: rules only".to_string() } else { format!("AI {}", effective.as_str()) },
                Style::default().fg(rgb(if effective == AiMode::Off { theme.dim } else { theme.known })),
            ),
        ]));
        info.push(Line::raw(""));
        for l in [
            "The AI only ever sees the command line with secrets masked, the folder name, git state, root or not, and whether target files exist. Never file contents.",
            "",
            "It can raise a risk at any time. It can lower one level only when very sure, and never below a hard rule.",
            "",
            "Local and free: install Ollama, then pick the Ollama preset and fetch models.",
        ] {
            info.push(Line::styled(l, dim));
        }
        f.render_widget(Paragraph::new(info).wrap(Wrap { trim: false }).block(self.block("Details", theme)), right);
    }

    fn sample_views(&self) -> Vec<(&'static str, View)> {
        let mut out = Vec::new();
        let mk = |cmd: &str, ai: AiStatus| {
            let a = analyze(cmd);
            build_view(&a, &ai, DisplayMode::Always)
        };
        if let Some(v) = mk("git status", AiStatus::Off) {
            out.push(("git status", v));
        }
        if let Some(v) = mk("chmod -R 777 public", AiStatus::Off) {
            out.push(("chmod -R 777 public", v));
        }
        if let Some(v) = mk("rm -rf build", AiStatus::Off) {
            out.push(("rm -rf build", v));
        }
        if let Some(v) = mk("frobnicate --all", AiStatus::Pending) {
            out.push(("frobnicate --all", v));
        }
        let verdict = AiVerdict { level: Level::Caution, confidence: 0.9, reason: "Typo of sudo, pipes text into a shell".into() };
        let merged = Merged { level: Level::Caution, changed: true, show_reason: true, ai_is_primary: true };
        if let Some(mut v) = mk("echo hi | sud bash", AiStatus::Done(merged, verdict)) {
            v.changed_frames = Some(self.frame % 60);
            out.push(("echo hi | sud bash", v));
        }
        out
    }

    fn draw_look(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        let [top, bottom] = Layout::vertical([Constraint::Length(9), Constraint::Min(6)]).areas(area);
        let ui = &self.cfg.ui;
        let names = self.theme_names();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let onoff = |b: bool| if b { "on" } else { "off" };
        let lines = vec![
            Line::raw(""),
            self.field_line(theme, 0, "Theme", self.choice(theme, &name_refs, &ui.theme), ""),
            self.field_line(
                theme,
                1,
                "Icons",
                self.choice(
                    theme,
                    &["nerd", "unicode", "ascii"],
                    match ui.icons {
                        IconSet::Nerd => "nerd",
                        IconSet::Unicode => "unicode",
                        IconSet::Ascii => "ascii",
                    },
                ),
                "nerd needs a Nerd Font",
            ),
            self.field_line(theme, 2, "Show line", self.choice(theme, &["always", "risky-only", "off"], ui.display.as_str()), "Alt-B toggles per shell"),
            self.field_line(theme, 3, "Reduce motion", self.choice(theme, &["off", "on"], onoff(ui.reduce_motion)), "no sweep or glow"),
            self.field_line(theme, 4, "Screen reader", self.choice(theme, &["off", "on"], onoff(ui.screen_reader)), "plain words, no icons"),
        ];
        f.render_widget(Paragraph::new(lines).block(self.block("Appearance", theme)), top);

        let inner_w = bottom.width.saturating_sub(4) as usize;
        let o = self.opts(theme, inner_w);
        let mut lines = vec![Line::raw("")];
        for (cmd, v) in self.sample_views() {
            lines.push(Line::from(vec![
                Span::styled("  $ ", Style::default().fg(rgb(theme.dim))),
                Span::styled(cmd.to_string(), Style::default().fg(rgb(theme.text)).add_modifier(Modifier::BOLD)),
            ]));
            let mut l = to_line(&render::render(&v, &o));
            l.spans.insert(0, Span::raw("  "));
            lines.push(l);
            lines.push(Line::raw(""));
        }
        f.render_widget(Paragraph::new(lines).block(self.block("Live preview", theme)), bottom);
    }

    fn draw_rules(&mut self, f: &mut Frame, area: Rect, theme: &Theme) {
        let [main, tryb] = Layout::vertical([Constraint::Min(6), Constraint::Length(4)]).areas(area);
        let [left, right] = Layout::horizontal([Constraint::Length(34), Constraint::Min(30)]).areas(main);
        let list = self.filtered_rules();
        let n = list.len();
        if self.rules_state.selected().unwrap_or(0) >= n && n > 0 {
            self.rules_state.select(Some(n - 1));
        }
        let ic = render::icons(self.cfg.ui.icons);
        let items: Vec<ListItem> = list
            .iter()
            .map(|c| {
                let l = c.default.level;
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{} ", ic.level(l)), Style::default().fg(rgb(theme.level_color(l)))),
                    Span::styled(format!("{:<14}", c.name), Style::default().fg(rgb(theme.text))),
                    Span::styled(c.category.to_string(), Style::default().fg(rgb(theme.dim))),
                ]))
            })
            .collect();
        let search_title = if self.rules_input == RulesInput::Search {
            format!("/{}\u{2588}", self.rules_query)
        } else if self.rules_query.is_empty() {
            format!("{} commands  / to search", n)
        } else {
            format!("/{}  {} found", self.rules_query, n)
        };
        let lst = List::new(items)
            .block(self.block(&search_title, theme))
            .highlight_style(Style::default().bg(rgb(theme.dim.darken(0.4))).add_modifier(Modifier::BOLD))
            .highlight_symbol("\u{258c}");
        f.render_stateful_widget(lst, left, &mut self.rules_state);

        let mut d: Vec<Line> = Vec::new();
        let dim = Style::default().fg(rgb(theme.dim));
        let txt = Style::default().fg(rgb(theme.text));
        if let Some(c) = list.get(self.rules_state.selected().unwrap_or(0)) {
            d.push(Line::from(vec![Span::styled(c.name.to_string(), txt.add_modifier(Modifier::BOLD)), Span::styled(format!("   {}", c.category), dim)]));
            d.push(Line::raw(""));
            let lv = |l: Level| Span::styled(format!("{:<8}", label(l)), Style::default().fg(rgb(theme.level_color(l))).add_modifier(Modifier::BOLD));
            d.push(Line::from(vec![Span::styled("default  ", dim), lv(c.default.level), Span::styled(c.default.text.to_string(), txt)]));
            let flag = |f: &&str| if f.len() == 1 { format!("-{f}") } else { format!("--{f}") };
            for r in c.rules.iter().take(40) {
                let mut when = vec![c.name.to_string()];
                if !r.sub.is_empty() {
                    when.push(r.sub.join(" "));
                }
                if !r.flags_all.is_empty() {
                    when.push(r.flags_all.iter().map(flag).collect::<Vec<_>>().join(" "));
                }
                if !r.flags_any.is_empty() {
                    when.push(format!("with {}", r.flags_any.iter().map(flag).collect::<Vec<_>>().join(" or ")));
                }
                if !r.args_any.is_empty() {
                    when.push(format!("arg {}", r.args_any.join(" or ")));
                }
                let hard = if r.verdict.hard { " (hard)" } else { "" };
                d.push(Line::raw(""));
                d.push(Line::from(vec![Span::styled("         ", dim), lv(r.verdict.level), Span::styled(format!("{}{hard}", r.verdict.text), txt)]));
                d.push(Line::from(vec![Span::styled("  when   ", dim), Span::styled(when.join(" "), dim)]));
            }
        }
        d.push(Line::raw(""));
        d.push(Line::styled(format!("Add your own rules in {}", store::rules_dir().display()), dim));
        f.render_widget(Paragraph::new(d).wrap(Wrap { trim: false }).block(self.block("Rule", theme)), right);

        let o = self.opts(theme, tryb.width.saturating_sub(4) as usize);
        let a = analyze(&self.try_line);
        let ghost = build_view(&a, &AiStatus::Off, DisplayMode::Always).map(|v| to_line(&render::render(&v, &o))).unwrap_or_default();
        let cursor = if self.rules_input == RulesInput::Try && self.frame / 8 % 2 == 0 { "\u{2588}" } else { "" };
        let lines = vec![
            Line::from(vec![
                Span::styled("$ ", dim),
                Span::styled(self.try_line.clone(), txt.add_modifier(Modifier::BOLD)),
                Span::styled(cursor, Style::default().fg(rgb(theme.analyzing))),
            ]),
            ghost,
        ];
        let title = if self.rules_input == RulesInput::Try { "Try a command  Esc when done" } else { "Try a command  t to type" };
        f.render_widget(Paragraph::new(lines).block(self.block(title, theme)), tryb);
    }

    fn draw_about(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        let dim = Style::default().fg(rgb(theme.dim));
        let txt = Style::default().fg(rgb(theme.text));
        let font = crate::fonts::detected();
        let user_rules = self.rules_all.len().saturating_sub(rules::builtin_count());
        let mut lines = vec![
            Line::raw(""),
            Line::from(vec![Span::styled("Version     ", dim), Span::styled(store::VERSION.to_string(), txt)]),
            Line::from(vec![
                Span::styled("Rule book   ", dim),
                Span::styled(format!("{} built-in commands, {} from rules.d", rules::builtin_count(), user_rules), txt),
            ]),
            Line::from(vec![Span::styled("Config      ", dim), Span::styled(store::config_file().display().to_string(), txt)]),
            Line::from(vec![
                Span::styled("Daemon      ", dim),
                Span::styled(if beforerunit_core::proto::ping().is_some() { "running" } else { "not running (starts on demand)" }.to_string(), txt),
            ]),
            Line::from(vec![
                Span::styled("Font        ", dim),
                match &font {
                    Some(name) => Span::styled(name.clone(), Style::default().fg(rgb(theme.known))),
                    None => Span::styled("no Nerd Font found. Run: beforerunit fonts install", Style::default().fg(rgb(theme.caution))),
                },
            ]),
            Line::raw(""),
            Line::styled("Icons check (should be four symbols):", dim),
        ];
        let ic = render::icons(IconSet::Nerd);
        lines.push(Line::from(
            [(Level::Known, ic.known), (Level::Caution, ic.caution), (Level::Danger, ic.danger), (Level::Unknown, ic.unknown)]
                .iter()
                .map(|(l, i)| Span::styled(format!("  {i}  {}  ", label(*l)), Style::default().fg(rgb(theme.level_color(*l)))))
                .collect::<Vec<_>>(),
        ));
        lines.push(Line::raw(""));
        lines.push(Line::styled("Open source under MIT or Apache-2.0.", dim));
        lines.push(Line::styled("https://github.com/beforerunit/beforerunit", Style::default().fg(rgb(theme.analyzing))));
        let up = self.started.elapsed().as_secs();
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!("Session {}m{:02}s", up / 60, up % 60), dim));
        f.render_widget(Paragraph::new(lines).block(self.block("About", theme)), area);
    }

    fn draw_picker(&mut self, f: &mut Frame, area: Rect, theme: &Theme) {
        let w = 50.min(area.width.saturating_sub(4));
        let h = (self.models.len() as u16 + 2).clamp(5, area.height.saturating_sub(6));
        let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
        f.render_widget(Clear, r);
        let items: Vec<ListItem> = self
            .models
            .iter()
            .map(|m| {
                let cur = *m == self.cfg.ai.model;
                ListItem::new(Line::from(vec![
                    Span::styled(if cur { "\u{2713} " } else { "  " }, Style::default().fg(rgb(theme.known))),
                    Span::styled(m.clone(), Style::default().fg(rgb(theme.text))),
                ]))
            })
            .collect();
        let lst = List::new(items)
            .block(self.block("Pick a model  Enter / Esc", theme))
            .highlight_style(Style::default().bg(rgb(theme.analyzing)).fg(rgb(theme.badge_fg)).add_modifier(Modifier::BOLD));
        if let Some(st) = &mut self.picker {
            f.render_stateful_widget(lst, r, st);
        }
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        let [status, keys] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        if let Some((msg, level, at)) = &self.status {
            if at.elapsed() < Duration::from_secs(8) {
                let c = theme.level_color(*level);
                // Fade in from white: a short glow when the message appears.
                let t = (at.elapsed().as_millis() as f32 / 400.0).min(1.0);
                let col = if self.cfg.ui.reduce_motion { c } else { Rgb(255, 255, 255).lerp(c, t) };
                f.render_widget(Paragraph::new(Line::styled(msg.clone(), Style::default().fg(rgb(col)))).alignment(Alignment::Center), status);
            }
        } else if let Some((w, t)) = &self.busy {
            let s = SPIN[(t.elapsed().as_millis() / 80) as usize % SPIN.len()];
            f.render_widget(Paragraph::new(Line::styled(format!("{s} {w}..."), Style::default().fg(rgb(theme.analyzing)))).alignment(Alignment::Center), status);
        }
        let dirty = if self.dirty() { "  \u{2022} unsaved" } else { "" };
        let hints = match self.tab {
            Tab::Rules => "Tab section  \u{2191}\u{2193} browse  / search  t try  Ctrl-S save  q quit",
            _ => "Tab section  \u{2191}\u{2193} move  Enter/\u{2190}\u{2192} change  f fetch  t test  Ctrl-S save  q quit",
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(hints, Style::default().fg(rgb(theme.dim))),
                Span::styled(dirty, Style::default().fg(rgb(theme.caution))),
            ]))
            .alignment(Alignment::Center),
            keys,
        );
    }
}
