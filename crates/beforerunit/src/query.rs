//! `beforerunit query`: one-shot Tier 1 analysis for shells that cannot keep
//! a connection (fish, plain bash) and the zsh fallback when the daemon is down.

use beforerunit_core::config::DisplayMode;
use beforerunit_core::ghost::{build_view, AiStatus};
use beforerunit_core::painter::{DrawArgs, Painter};
use beforerunit_core::proto::Caps;
use beforerunit_core::render;
use beforerunit_core::store::{self, Compiled};
use beforerunit_core::Engine;

use crate::Shell;

#[derive(clap::Args)]
pub struct QueryArgs {
    #[arg(long, value_enum, default_value_t = Shell::Bash)]
    shell: Shell,
    #[arg(long, default_value_t = 80)]
    cols: usize,
    /// Cursor position in characters
    #[arg(long, default_value_t = usize::MAX)]
    point: usize,
    /// Visible width of the last prompt line
    #[arg(long, default_value_t = 0)]
    pw: usize,
    #[arg(long, default_value = "")]
    sid: String,
    /// Display override from the toggle key: always, risky-only, off
    #[arg(long, default_value = "")]
    mode: String,
    #[arg(long, default_value = "")]
    cwd: String,
    #[arg(long, default_value = "")]
    euid: String,
    #[arg(long, default_value = "")]
    tty: String,
    #[arg(long, default_value = "")]
    pid: String,
    /// Color depth code: t (truecolor), 2 (256), 1 (16), n (none); i = italic
    #[arg(long, default_value = "")]
    caps: String,
    /// The cursor is at the start of the input (bash bind -x), not at the point
    #[arg(long)]
    from_start: bool,
    /// A ghost line from an earlier call is on screen
    #[arg(long)]
    drawn: bool,
    /// Erase the ghost line instead of drawing
    #[arg(long)]
    clear: bool,
    /// Print only the styled line without cursor movement (ble.sh info area)
    #[arg(long)]
    line_only: bool,
    #[arg(allow_hyphen_values = true, default_value = "")]
    line: String,
}

fn recompile() -> Option<Compiled> {
    let c = store::compile();
    let _ = store::save(&c);
    Some(c)
}

pub fn run(a: QueryArgs) -> i32 {
    use std::io::Write;
    let out = match a.shell {
        Shell::Zsh => zsh(&a),
        Shell::Bash | Shell::Fish => {
            let mut p = Painter::new(Some(recompile));
            if a.drawn {
                p.assume_drawn("");
            }
            let args = DrawArgs {
                line: &a.line,
                point: a.point.min(a.line.chars().count()),
                cols: a.cols,
                prompt_width: a.pw,
                sid: &a.sid,
                mode: &a.mode,
                cwd: &a.cwd,
                euid: &a.euid,
                tty: &a.tty,
                pid: &a.pid,
                from_line_start: a.from_start,
                caps: &a.caps,
            };
            if a.line_only {
                let (_, down) = beforerunit_core::painter::rows_below(a.pw, &a.line, args.point, a.cols);
                p.line(&args, down)
            } else if a.clear {
                p.clear(&args)
            } else {
                p.draw(&args)
            }
        }
    };
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(out.as_bytes());
    let _ = o.flush();
    0
}

/// `text TAB specs` for POSTDISPLAY and region_highlight, no AI.
fn zsh(a: &QueryArgs) -> String {
    let ui = crate::ui::Ui::load();
    let caps = if a.caps.is_empty() { Caps { depth: render::ColorDepth::from_env(), italic: true } } else { Caps::parse(&a.caps) };
    let display = DisplayMode::parse(&a.mode).unwrap_or(ui.config().ui.display);
    let mut engine = Engine::new();
    let an = engine.analyze(&a.line);
    match build_view(an, &AiStatus::Off, display) {
        Some(v) => {
            let styled = ui.render(&v, caps.depth, 0, a.cols, false);
            let (text, specs) = render::to_zsh(&styled, caps.depth, caps.italic);
            format!("{}\t{}", beforerunit_core::proto::escape(&text), beforerunit_core::proto::escape(&specs))
        }
        None => "\t".into(),
    }
}
