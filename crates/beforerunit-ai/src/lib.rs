//! Tier 2: AI risk review over any OpenAI-compatible `/chat/completions`
//! endpoint (Ollama, Groq, OpenRouter, LM Studio, vLLM, llama.cpp, ...).

pub mod redact;

use std::time::{Duration, Instant};

use beforerunit_core::merge::AiVerdict;
use beforerunit_core::Level;
use futures_util::StreamExt;
use serde_json::{json, Value};

pub use redact::redact;

#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub needs_key: bool,
    pub suggested_model: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset { id: "ollama", name: "Ollama (local)", base_url: "http://localhost:11434/v1", needs_key: false, suggested_model: "qwen2.5:0.5b" },
    Preset { id: "groq", name: "Groq", base_url: "https://api.groq.com/openai/v1", needs_key: true, suggested_model: "llama-3.1-8b-instant" },
    Preset { id: "openrouter", name: "OpenRouter", base_url: "https://openrouter.ai/api/v1", needs_key: true, suggested_model: "qwen/qwen-2.5-7b-instruct" },
    Preset { id: "lmstudio", name: "LM Studio (local)", base_url: "http://localhost:1234/v1", needs_key: false, suggested_model: "qwen2.5-0.5b-instruct" },
    Preset { id: "custom", name: "Custom", base_url: "", needs_key: false, suggested_model: "" },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
}

/// Light, privacy-preserving facts about where the command will run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Context {
    pub cwd_name: String,
    pub git_repo: bool,
    pub git_dirty: Option<bool>,
    pub root: bool,
    /// (target, exists)
    pub targets: Vec<(String, bool)>,
}

#[derive(Debug, Clone)]
pub struct ReviewRequest<'a> {
    /// Already redacted.
    pub command: &'a str,
    pub tier1_level: Level,
    pub tier1_text: &'a str,
    pub context: &'a Context,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiError {
    Http(u16, String),
    Timeout,
    Connect(String),
    BadResponse(String),
}

impl AiError {
    /// Plain-English explanation for the setup app and doctor.
    pub fn explain(&self) -> String {
        match self {
            AiError::Http(401, _) | AiError::Http(403, _) => "The server rejected the API key (401/403). Check the key.".into(),
            AiError::Http(404, _) => "Endpoint or model not found (404). Check the base URL and model name.".into(),
            AiError::Http(429, _) => "Rate limited (429). Wait a moment or use a different model.".into(),
            AiError::Http(code, body) => format!("Server error {code}: {}", body.chars().take(120).collect::<String>()),
            AiError::Timeout => "No answer before the timeout. Try a smaller or local model.".into(),
            AiError::Connect(e) => format!("Cannot reach the server. Is it running? ({e})"),
            AiError::BadResponse(e) => format!("The model did not reply with valid JSON: {e}"),
        }
    }

    /// A few words for the ghost line.
    pub fn short(&self) -> &'static str {
        match self {
            AiError::Http(401, _) | AiError::Http(403, _) => "AI key rejected",
            AiError::Http(429, _) => "AI rate limited",
            AiError::Http(_, _) => "AI server error",
            AiError::Timeout => "AI timed out",
            AiError::Connect(_) => "AI offline",
            AiError::BadResponse(_) => "AI reply unreadable",
        }
    }
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.explain())
    }
}

impl std::error::Error for AiError {}

pub const SYSTEM_PROMPT: &str = "You are BeforeRunIt, a reviewer that judges the real-world risk of a Linux shell command BEFORE it runs.
Reply with ONLY one JSON object and nothing else:
{\"risk\":\"safe|caution|danger\",\"confidence\":0.0-1.0,\"reason\":\"...\"}
- danger: data loss, irreversible changes, security exposure, system or account breakage, running untrusted code.
- caution: changes state but is recoverable, needs care, or depends on context.
- safe: read-only or harmless.
- reason: plain English for a beginner, at most 10 words, the concrete consequence. No emoji, no markdown, no quotes around the command.
- Use the context (git state, root, whether targets exist) when it changes the risk.";

pub fn build_user_prompt(r: &ReviewRequest) -> String {
    let c = r.context;
    let mut ctx = Vec::new();
    if !c.cwd_name.is_empty() {
        ctx.push(format!("folder \"{}\"", c.cwd_name));
    }
    if c.git_repo {
        ctx.push(match c.git_dirty {
            Some(true) => "git repo with uncommitted changes".to_string(),
            Some(false) => "git repo, clean".to_string(),
            None => "git repo".to_string(),
        });
    }
    ctx.push(if c.root { "running as root".into() } else { "normal user".into() });
    for (t, exists) in c.targets.iter().take(4) {
        ctx.push(format!("target {t} {}", if *exists { "exists" } else { "does not exist" }));
    }
    let tier1 = match r.tier1_level {
        Level::Unknown | Level::Idle => "none (command not in the rule book)".to_string(),
        l => format!("{} - {}", l.as_str(), r.tier1_text),
    };
    format!("Command: {}\nRule-based guess: {tier1}\nContext: {}", r.command, ctx.join("; "))
}

/// Finds the first complete top-level JSON object in `s`.
pub fn extract_json(s: &str) -> Option<&str> {
    let s = match (s.find("<think>"), s.find("</think>")) {
        (Some(_), Some(e)) => &s[e + 8..],
        (Some(_), None) => return None,
        _ => s,
    };
    let start = s.find('{')?;
    let b = s.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, &c) in b.iter().enumerate().skip(start) {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn clean_reason(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_control())
        .filter(|c| {
            let u = *c as u32;
            !(0x1F000..=0x1FAFF).contains(&u) && !(0x2600..=0x27BF).contains(&u) && u != 0xFE0F && u != 0x200D
        })
        .map(|c| if matches!(c, '*' | '`' | '#') { ' ' } else { c })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let mut out = words.iter().take(10).copied().collect::<Vec<_>>().join(" ");
    out = out.trim_matches(|c: char| c == '"' || c == '\'').to_string();
    if words.len() > 10 && !out.ends_with('.') {
        out.push_str("...");
    }
    if let Some(first) = out.chars().next() {
        if first.is_lowercase() {
            out = first.to_uppercase().collect::<String>() + &out[first.len_utf8()..];
        }
    }
    out
}

/// Validates the model's JSON reply into a verdict.
pub fn parse_verdict(raw: &str) -> Result<AiVerdict, AiError> {
    let j = extract_json(raw).ok_or_else(|| AiError::BadResponse(raw.chars().take(80).collect()))?;
    let v: Value = serde_json::from_str(j).map_err(|e| AiError::BadResponse(e.to_string()))?;
    let risk = v.get("risk").or_else(|| v.get("level")).and_then(Value::as_str).unwrap_or("");
    let level = match Level::parse(risk) {
        Some(Level::Known) => Level::Known,
        Some(Level::Danger) => Level::Danger,
        Some(Level::Caution) => Level::Caution,
        _ => return Err(AiError::BadResponse(format!("unknown risk {risk:?}"))),
    };
    let confidence = v
        .get("confidence")
        .and_then(|c| c.as_f64().or_else(|| c.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0.5)
        .clamp(0.0, 1.0) as f32;
    let reason = clean_reason(v.get("reason").or_else(|| v.get("summary")).and_then(Value::as_str).unwrap_or(""));
    Ok(AiVerdict { level, confidence, reason })
}

pub struct Client {
    http: reqwest::Client,
    ep: Endpoint,
}

#[derive(Debug, Clone)]
pub struct TestReport {
    pub latency_ms: u128,
    pub raw: String,
    pub verdict: Result<AiVerdict, AiError>,
}

fn map_err(e: reqwest::Error) -> AiError {
    if e.is_timeout() {
        AiError::Timeout
    } else if e.is_connect() {
        AiError::Connect(e.without_url().to_string())
    } else if let Some(s) = e.status() {
        AiError::Http(s.as_u16(), e.to_string())
    } else {
        AiError::Connect(e.without_url().to_string())
    }
}

impl Client {
    pub fn new(ep: Endpoint) -> Result<Client, AiError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("beforerunit/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(3))
            .pool_idle_timeout(Duration::from_secs(120))
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()
            .map_err(|e| AiError::Connect(e.to_string()))?;
        Ok(Client { http, ep })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.ep
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.ep.base_url.trim_end_matches('/'), path)
    }

    fn req(&self, b: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let b = b.header("HTTP-Referer", "https://github.com/beforerunit/beforerunit").header("X-Title", "BeforeRunIt");
        match &self.ep.api_key {
            Some(k) if !k.is_empty() => b.bearer_auth(k),
            _ => b,
        }
    }

    /// Opens the pooled connection early so the first review is fast.
    pub async fn warm(&self) {
        let _ = self.req(self.http.get(self.url("models"))).timeout(Duration::from_secs(3)).send().await;
    }

    pub async fn list_models(&self) -> Result<Vec<String>, AiError> {
        let resp = self.req(self.http.get(self.url("models"))).timeout(self.ep.timeout.max(Duration::from_secs(5))).send().await.map_err(map_err)?;
        let status = resp.status();
        let body = resp.text().await.map_err(map_err)?;
        if !status.is_success() {
            return Err(AiError::Http(status.as_u16(), body));
        }
        let v: Value = serde_json::from_str(&body).map_err(|e| AiError::BadResponse(e.to_string()))?;
        let mut out: Vec<String> = v
            .get("data")
            .or_else(|| v.get("models"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|m| m.get("id").or_else(|| m.get("name")).and_then(Value::as_str).map(String::from)).collect())
            .unwrap_or_default();
        out.sort();
        Ok(out)
    }

    fn body(&self, user: &str, stream: bool) -> Value {
        json!({
            "model": self.ep.model,
            "temperature": 0,
            "max_tokens": 80,
            "stream": stream,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": user},
            ],
        })
    }

    /// Streams the completion and returns as soon as the JSON object closes.
    pub async fn complete_raw(&self, user: &str) -> Result<String, AiError> {
        let fut = async {
            let resp = self.req(self.http.post(self.url("chat/completions"))).json(&self.body(user, true)).send().await.map_err(map_err)?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(AiError::Http(status.as_u16(), body));
            }
            let is_sse = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("event-stream"));
            if !is_sse {
                let v: Value = resp.json().await.map_err(map_err)?;
                return Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string());
            }
            let mut content = String::new();
            let mut buf = String::new();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(map_err)?;
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(nl) = buf.find('\n') {
                    let line: String = buf.drain(..=nl).collect();
                    let line = line.trim();
                    let Some(data) = line.strip_prefix("data:") else { continue };
                    let data = data.trim();
                    if data == "[DONE]" {
                        return Ok(content);
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(d) = v["choices"][0]["delta"]["content"].as_str() {
                            content.push_str(d);
                            if extract_json(&content).is_some() {
                                return Ok(content);
                            }
                        }
                    }
                }
            }
            Ok(content)
        };
        match tokio::time::timeout(self.ep.timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(AiError::Timeout),
        }
    }

    pub async fn review(&self, r: &ReviewRequest<'_>) -> Result<AiVerdict, AiError> {
        let raw = self.complete_raw(&build_user_prompt(r)).await?;
        parse_verdict(&raw)
    }

    /// Sends a sample command and reports latency, raw reply and parsed risk.
    pub async fn test(&self) -> TestReport {
        let ctx = Context { cwd_name: "project".into(), git_repo: true, git_dirty: Some(true), root: false, targets: vec![("build".into(), true)] };
        let req = ReviewRequest { command: "rm -rf build", tier1_level: Level::Danger, tier1_text: "Deletes \"build\" forever. Skips Trash.", context: &ctx };
        let start = Instant::now();
        let raw = self.complete_raw(&build_user_prompt(&req)).await;
        let latency_ms = start.elapsed().as_millis();
        match raw {
            Ok(raw) => {
                let verdict = parse_verdict(&raw);
                TestReport { latency_ms, raw, verdict }
            }
            Err(e) => TestReport { latency_ms, raw: String::new(), verdict: Err(e) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_wrapped_json() {
        let v = parse_verdict(r#"{"risk":"danger","confidence":0.92,"reason":"deletes the build folder forever"}"#).unwrap();
        assert_eq!(v.level, Level::Danger);
        assert_eq!(v.reason, "Deletes the build folder forever");
        let v = parse_verdict("Sure! ```json\n{\"risk\": \"safe\", \"confidence\": \"0.7\", \"reason\": \"Only lists files.\"}\n```").unwrap();
        assert_eq!(v.level, Level::Known);
        assert!((v.confidence - 0.7).abs() < 1e-6);
        let v = parse_verdict("<think>{hmm}</think>{\"risk\":\"caution\",\"reason\":\"a b c d e f g h i j k l m\"}").unwrap();
        assert_eq!(v.level, Level::Caution);
        assert!(v.reason.split_whitespace().count() <= 10);
        assert!(parse_verdict("no json here").is_err());
        assert!(parse_verdict(r#"{"risk":"spicy"}"#).is_err());
    }

    #[test]
    fn strips_emoji() {
        let v = parse_verdict("{\"risk\":\"danger\",\"reason\":\"\u{1F525} wipes disk \u{26A0}\u{FE0F}\"}").unwrap();
        assert_eq!(v.reason, "Wipes disk");
    }

    #[test]
    fn prompt_includes_context() {
        let ctx = Context { cwd_name: "app".into(), git_repo: true, git_dirty: Some(true), root: true, targets: vec![("dist".into(), false)] };
        let p = build_user_prompt(&ReviewRequest { command: "git clean -fdx", tier1_level: Level::Caution, tier1_text: "x", context: &ctx });
        assert!(p.contains("uncommitted"));
        assert!(p.contains("running as root"));
        assert!(p.contains("dist does not exist"));
    }
}
