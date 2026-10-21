//! `beforerunit test-ai`: one sample review against the configured endpoint.

use beforerunit_ai::Client;
use beforerunit_core::render::label;

use crate::ui::{paint, Ui};

pub fn run() -> i32 {
    let ui = Ui::load();
    let cfg = &ui.config().ai;
    let t = &ui.theme;
    if cfg.base_url.is_empty() || cfg.model.is_empty() {
        println!("AI is not configured. Run {} to pick a provider.", paint::bold("beforerunit setup"));
        return 1;
    }
    let key = crate::keys::get();
    println!("{} {}", paint::fg(t.dim, "Endpoint"), cfg.base_url);
    println!("{} {}", paint::fg(t.dim, "Model   "), cfg.model);
    println!(
        "{} {}",
        paint::fg(t.dim, "API key "),
        match &key {
            Some((k, w)) => format!("{} ({})", crate::keys::mask(k), w.describe()),
            None => "none".into(),
        }
    );
    println!("{}", paint::dim("Sending: rm -rf build"));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
    let report = rt.block_on(async {
        let c = Client::new(crate::context::endpoint(cfg, key.map(|(k, _)| k)))?;
        Ok::<_, beforerunit_ai::AiError>(c.test().await)
    });
    match report {
        Ok(r) => match r.verdict {
            Ok(v) => {
                println!(
                    "{} {} in {} ms: {} {:.2} \"{}\"",
                    paint::fg(t.known, "\u{f05e0}"),
                    paint::bold("Connected"),
                    r.latency_ms,
                    paint::fg(t.level_color(v.level), label(v.level)),
                    v.confidence,
                    v.reason
                );
                if r.latency_ms > 2500 {
                    println!("{}", paint::dim("Slow reply. A smaller model or a local provider (Ollama) feels instant."));
                }
                0
            }
            Err(e) => {
                println!("{} {}", paint::fg(t.danger, "\u{f0159}"), e.explain());
                if !r.raw.is_empty() {
                    println!("{} {}", paint::dim("Raw reply:"), r.raw.chars().take(300).collect::<String>());
                }
                1
            }
        },
        Err(e) => {
            println!("{} {}", paint::fg(t.danger, "\u{f0159}"), e.explain());
            1
        }
    }
}
