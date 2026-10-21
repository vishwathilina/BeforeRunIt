//! `beforerunit explain <command>`: the ghost line plus the details behind it.

use beforerunit_ai::{redact, Client, ReviewRequest};
use beforerunit_core::config::AiMode;
use beforerunit_core::ghost::{build_view, AiStatus};
use beforerunit_core::merge::merge;
use beforerunit_core::render::{self, label, ColorDepth};
use beforerunit_core::{analyze, Level};

use crate::ui::{paint, term_cols, Ui};

pub fn run(line: &str, json: bool, no_ai: bool) -> i32 {
    let ui = Ui::load();
    let a = analyze(line);
    let cfg = ui.config();
    let use_ai = !no_ai && cfg.ai.effective_mode() != AiMode::Off && !a.is_empty();
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    // SAFETY: geteuid has no preconditions.
    let root = a.root || unsafe { libc::geteuid() } == 0;

    let mut ai_status = AiStatus::Off;
    let mut ai_err = None;
    if use_ai {
        if !json {
            eprintln!("{}", paint::dim(&format!("Asking {} ...", cfg.ai.model)));
        }
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
        let res = rt.block_on(async {
            let client = Client::new(crate::context::endpoint(&cfg.ai, crate::keys::api_key()))?;
            let ctx = crate::context::gather(&cwd, root, &a.targets).await;
            let cmd = redact(line);
            client.review(&ReviewRequest { command: &cmd, tier1_level: a.level, tier1_text: &a.text, context: &ctx }).await
        });
        match res {
            Ok(v) => ai_status = AiStatus::Done(merge(&a, &v, cfg.ai.min_confidence_to_lower), v),
            Err(e) => {
                ai_err = Some(e.explain());
                ai_status = AiStatus::Error(e.short().into());
            }
        }
    }

    if json {
        let (ai_level, ai_conf, ai_reason, final_level) = match &ai_status {
            AiStatus::Done(m, v) => (Some(v.level.as_str()), Some((v.confidence as f64 * 100.0).round() / 100.0), Some(v.reason.clone()), m.level),
            _ => (None, None, None, a.level),
        };
        let v = serde_json::json!({
            "command": line,
            "level": final_level.as_str(),
            "tier1": { "level": a.level.as_str(), "text": a.text, "hard": a.hard, "root": a.root, "complex": a.complex, "cmd": a.cmd, "targets": a.targets },
            "ai": { "level": ai_level, "confidence": ai_conf, "reason": ai_reason, "error": ai_err },
        });
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return exit_code(final_level);
    }

    let depth = if paint::enabled() { ColorDepth::from_env() } else { ColorDepth::None };
    let cols = term_cols();
    let view = build_view(&a, &ai_status, beforerunit_core::config::DisplayMode::Always);
    let t = &ui.theme;
    println!("{} {}", paint::fg(t.dim, "$"), paint::bold(line));
    if let Some(v) = &view {
        let s = ui.render(v, depth, 0, cols, false);
        if paint::enabled() {
            println!("{}", render::to_ansi(&s, depth));
        } else {
            println!("{}", s.plain());
        }
    }
    let row = |k: &str, v: String| println!("   {} {v}", paint::fg(t.dim, &format!("{k:<9}")));
    row("Command", a.cmd.map(String::from).unwrap_or_else(|| "not in the rule book".into()));
    let mut risk = paint::fg(t.level_color(a.level), label(a.level));
    if a.hard {
        risk.push_str(&paint::dim("  hard rule, AI cannot lower it"));
    }
    row("Rules", risk);
    if !a.targets.is_empty() {
        let cwdp = std::path::Path::new(&cwd);
        let list: Vec<String> = a
            .targets
            .iter()
            .map(|x| {
                if cwdp.join(x).symlink_metadata().is_ok() {
                    format!("{x} {}", paint::dim("(exists)"))
                } else {
                    x.clone()
                }
            })
            .collect();
        row("Targets", list.join(", "));
    }
    if a.root {
        row("Root", paint::fg(t.caution, "runs with root rights"));
    }
    if a.complex {
        row("Shape", "complex: pipeline or substitution".into());
    }
    match &ai_status {
        AiStatus::Done(m, v) => {
            let mut s = format!("{} {:.2}  {}", paint::fg(t.level_color(v.level), label(v.level)), v.confidence, v.reason);
            if m.changed {
                s.push_str(&paint::dim(&format!("  (final: {})", label(m.level))));
            }
            row("AI", s);
        }
        AiStatus::Error(_) => row("AI", paint::fg(t.danger, ai_err.as_deref().unwrap_or("failed"))),
        _ if no_ai => row("AI", paint::dim("skipped")),
        _ => row("AI", paint::dim("not configured (beforerunit setup)")),
    }
    let final_level = match &ai_status {
        AiStatus::Done(m, _) => m.level,
        _ => a.level,
    };
    exit_code(final_level)
}

/// Scriptable: 0 known/idle, 1 unknown, 2 caution, 3 danger.
fn exit_code(l: Level) -> i32 {
    match l {
        Level::Idle | Level::Known => 0,
        Level::Unknown => 1,
        Level::Caution => 2,
        Level::Danger => 3,
    }
}
