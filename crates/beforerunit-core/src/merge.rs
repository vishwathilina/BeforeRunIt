//! Combines the instant Tier 1 verdict with the AI review verdict.
//!
//! - The AI may raise the risk freely.
//! - The AI may lower it by at most one level, only with high confidence, and
//!   never below a hard Tier 1 rule.
//! - For commands Tier 1 does not know, the AI verdict is used as-is.

use crate::engine::Analysis;
use crate::schema::Level;

#[derive(Debug, Clone, PartialEq)]
pub struct AiVerdict {
    /// Known, Caution or Danger.
    pub level: Level,
    pub confidence: f32,
    /// Short reason, at most ~10 words.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    pub level: Level,
    /// The AI changed the level Tier 1 would have shown.
    pub changed: bool,
    /// Show the AI's reason next to the summary.
    pub show_reason: bool,
    /// Tier 1 had no summary, so the AI reason becomes the main text.
    pub ai_is_primary: bool,
}

pub fn merge(tier1: &Analysis, ai: &AiVerdict, min_confidence_to_lower: f32) -> Merged {
    let t = tier1.level;
    let a = match ai.level {
        Level::Idle | Level::Unknown => Level::Caution,
        l => l,
    };
    if matches!(t, Level::Unknown | Level::Idle) {
        return Merged { level: a, changed: true, show_reason: true, ai_is_primary: true };
    }
    if a > t {
        return Merged { level: a, changed: true, show_reason: true, ai_is_primary: false };
    }
    if a < t {
        if tier1.hard || ai.confidence < min_confidence_to_lower {
            return Merged { level: t, changed: false, show_reason: false, ai_is_primary: false };
        }
        let floor = t.lower();
        let level = if a < floor { floor } else { a };
        return Merged { level, changed: level != t, show_reason: true, ai_is_primary: false };
    }
    Merged { level: t, changed: false, show_reason: false, ai_is_primary: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t1(level: Level, hard: bool) -> Analysis {
        Analysis { level, hard, text: "x".into(), ..Default::default() }
    }

    fn ai(level: Level, confidence: f32) -> AiVerdict {
        AiVerdict { level, confidence, reason: "because".into() }
    }

    #[test]
    fn raises_freely() {
        let m = merge(&t1(Level::Caution, false), &ai(Level::Danger, 0.1), 0.8);
        assert_eq!(m.level, Level::Danger);
        assert!(m.show_reason);
    }

    #[test]
    fn lowers_one_level_with_confidence() {
        let m = merge(&t1(Level::Danger, false), &ai(Level::Known, 0.9), 0.8);
        assert_eq!(m.level, Level::Caution);
        let m = merge(&t1(Level::Danger, false), &ai(Level::Known, 0.5), 0.8);
        assert_eq!(m.level, Level::Danger);
    }

    #[test]
    fn never_lowers_hard() {
        let m = merge(&t1(Level::Danger, true), &ai(Level::Known, 1.0), 0.8);
        assert_eq!(m.level, Level::Danger);
        assert!(!m.changed);
    }

    #[test]
    fn unknown_takes_ai() {
        let m = merge(&t1(Level::Unknown, false), &ai(Level::Known, 0.3), 0.8);
        assert_eq!(m.level, Level::Known);
        assert!(m.ai_is_primary);
    }
}
