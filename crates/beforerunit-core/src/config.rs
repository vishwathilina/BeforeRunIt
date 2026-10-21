//! User configuration (`~/.config/beforerunit/config.toml`). Parsing happens
//! only in `compile`; the hot path reads the precompiled cache.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AiMode {
    Off,
    UnknownOnly,
    #[default]
    ReviewAll,
}

impl AiMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AiMode::Off => "off",
            AiMode::UnknownOnly => "unknown-only",
            AiMode::ReviewAll => "review-all",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayMode {
    #[default]
    Always,
    RiskyOnly,
    Off,
}

impl DisplayMode {
    pub fn as_str(self) -> &'static str {
        match self {
            DisplayMode::Always => "always",
            DisplayMode::RiskyOnly => "risky-only",
            DisplayMode::Off => "off",
        }
    }

    pub fn next(self) -> DisplayMode {
        match self {
            DisplayMode::Always => DisplayMode::RiskyOnly,
            DisplayMode::RiskyOnly => DisplayMode::Off,
            DisplayMode::Off => DisplayMode::Always,
        }
    }

    pub fn parse(s: &str) -> Option<DisplayMode> {
        Some(match s {
            "always" => DisplayMode::Always,
            "risky-only" | "risky" => DisplayMode::RiskyOnly,
            "off" => DisplayMode::Off,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconSet {
    #[default]
    Nerd,
    Unicode,
    Ascii,
}

fn d_true() -> bool {
    true
}
fn d_timeout() -> u64 {
    4000
}
fn d_debounce() -> u64 {
    300
}
fn d_conf() -> f32 {
    0.8
}
fn d_theme() -> String {
    "midnight".into()
}
fn d_toggle() -> String {
    "\\eb".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct AiConfig {
    #[serde(default = "d_true")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: AiMode,
    /// Preset name shown in the setup app: ollama, groq, openrouter, lmstudio, custom.
    #[serde(default)]
    pub provider: String,
    /// OpenAI-compatible base URL, e.g. `http://localhost:11434/v1`. Empty means AI off.
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "d_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "d_debounce")]
    pub debounce_ms: u64,
    #[serde(default = "d_conf")]
    pub min_confidence_to_lower: f32,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            enabled: true,
            mode: AiMode::ReviewAll,
            provider: String::new(),
            base_url: String::new(),
            model: String::new(),
            timeout_ms: d_timeout(),
            debounce_ms: d_debounce(),
            min_confidence_to_lower: d_conf(),
        }
    }
}

impl AiConfig {
    /// AI is opt-in: nothing is sent until an endpoint and model are configured.
    pub fn effective_mode(&self) -> AiMode {
        if !self.enabled || self.base_url.trim().is_empty() || self.model.trim().is_empty() {
            AiMode::Off
        } else {
            self.mode
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct UiConfig {
    #[serde(default)]
    pub display: DisplayMode,
    #[serde(default)]
    pub icons: IconSet,
    #[serde(default = "d_theme")]
    pub theme: String,
    #[serde(default)]
    pub reduce_motion: bool,
    #[serde(default)]
    pub screen_reader: bool,
    /// Readline-style key sequence for the session toggle (default Alt-B).
    #[serde(default = "d_toggle")]
    pub toggle_key: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            display: DisplayMode::Always,
            icons: IconSet::Nerd,
            theme: d_theme(),
            reduce_motion: false,
            screen_reader: false,
            toggle_key: d_toggle(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Config {
    #[serde(default)]
    pub ai: AiConfig,
    #[serde(default)]
    pub ui: UiConfig,
}

/// A user theme (`~/.config/beforerunit/themes/*.toml`). Colors are `#rrggbb`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ThemeDef {
    pub name: String,
    pub known: String,
    pub caution: String,
    pub danger: String,
    pub unknown: String,
    #[serde(default)]
    pub analyzing: String,
    #[serde(default)]
    pub badge_fg: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub dim: String,
    #[serde(default)]
    pub highlight: String,
}
