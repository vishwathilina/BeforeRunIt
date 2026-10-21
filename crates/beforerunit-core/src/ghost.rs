//! Turns a Tier 1 analysis plus the AI state into what the ghost line shows.

use crate::config::DisplayMode;
use crate::engine::Analysis;
use crate::merge::{AiVerdict, Merged};
use crate::render::{AiTag, View};
use crate::schema::Level;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum AiStatus {
    /// AI disabled, or not needed in `unknown-only` mode.
    #[default]
    Off,
    Pending,
    Done(Merged, AiVerdict),
    Error(String),
}

pub const TEXT_ANALYZING: &str = "Analyzing command...";
pub const TEXT_ANALYZING_COMPLEX: &str = "Analyzing complex command...";
pub const TEXT_NO_AI: &str = "Not in the rule book. Check before running.";

/// Builds the view, or `None` if nothing should be shown.
pub fn build_view(a: &Analysis, ai: &AiStatus, display: DisplayMode) -> Option<View> {
    if display == DisplayMode::Off || a.is_empty() {
        return None;
    }
    let mut v = View {
        level: a.level,
        text: a.text.clone(),
        target_span: a.target_span,
        root: a.root,
        ..Default::default()
    };
    match ai {
        AiStatus::Off => {
            if a.level == Level::Unknown {
                v.text = TEXT_NO_AI.into();
            }
        }
        AiStatus::Pending => {
            if a.level == Level::Unknown {
                v.analyzing = true;
                v.text = if a.complex { TEXT_ANALYZING_COMPLEX } else { TEXT_ANALYZING }.into();
                v.target_span = None;
            } else if a.level != Level::Idle {
                v.ai = AiTag::Pending;
            }
        }
        AiStatus::Done(m, verdict) => {
            v.level = m.level;
            if m.ai_is_primary {
                v.text = verdict.reason.clone();
                v.target_span = None;
                v.ai = AiTag::Agree;
            } else if m.show_reason && !verdict.reason.is_empty() {
                v.ai = AiTag::Reason(verdict.reason.clone());
            } else {
                v.ai = AiTag::Agree;
            }
        }
        AiStatus::Error(e) => {
            if a.level == Level::Unknown {
                v.text = TEXT_NO_AI.into();
            }
            v.ai = AiTag::Error(e.clone());
        }
    }
    if display == DisplayMode::RiskyOnly && v.level < Level::Caution {
        return None;
    }
    Some(v)
}
