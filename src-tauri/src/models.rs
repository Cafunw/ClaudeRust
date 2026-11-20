//! Model catalog: display names, thinking/effort rules and tool variants.

use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Thinking {
    /// Thinking is always on (Fable/Mythos, Opus 5.5). Only effort is adjustable.
    AlwaysOn,
    /// Adaptive by default; `{type:"disabled"}` turns it off (only at effort <= high).
    Disableable,
    /// Adaptive by default; off is `{type:"between_tools"}` (Sonnet 5.5), effort <= high.
    BetweenTools,
    /// Off unless `{type:"adaptive"}` is sent explicitly (Opus 4.6-4.8, Sonnet 4.6).
    Explicit,
    /// Legacy `budget_tokens` thinking (Haiku 4.5, Sonnet/Opus 4.5).
    Budget,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub thinking: Thinking,
    pub efforts: Vec<&'static str>,
    pub default_effort: Option<&'static str>,
    /// Send `display: "summarized"` (models whose default display is "omitted").
    pub display_param: bool,
    /// Supports web_search_20260209 / web_fetch_20260209.
    pub web_tools_v2: bool,
    /// Supports server-side refusal fallbacks (`fallbacks: "default"`).
    pub fallbacks: bool,
    /// Supports server-side compaction (compact_20260112).
    pub compaction: bool,
    pub max_output: u32,
    pub context: u32,
    pub vision: bool,
    /// Price per million tokens (input, output) for usage estimates.
    pub price: (f64, f64),
    pub featured: bool,
    pub legacy: bool,
}

const ALL_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const EFFORTS_46: [&str; 4] = ["low", "medium", "high", "max"];

fn base(id: &str, name: &str, desc: &str) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        name: name.into(),
        description: desc.into(),
        thinking: Thinking::AlwaysOn,
        efforts: ALL_EFFORTS.to_vec(),
        default_effort: Some("high"),
        display_param: true,
        web_tools_v2: true,
        fallbacks: false,
        compaction: true,
        max_output: 128_000,
        context: 1_000_000,
        vision: true,
        price: (5.0, 25.0),
        featured: false,
        legacy: false,
    }
}

/// Known models, in the order they appear in the model picker.
pub fn catalog() -> Vec<ModelInfo> {
    let mut v = vec![];

    let mut m = base("claude-opus-5-5", "Opus 5.5", "Most capable for complex work");
    m.default_effort = Some("medium");
    m.fallbacks = true;
    m.price = (4.0, 20.0);
    m.featured = true;
    v.push(m);

    let mut m = base("claude-sonnet-5-5", "Sonnet 5.5", "Smart, efficient model for everyday use");
    m.thinking = Thinking::BetweenTools;
    m.fallbacks = true;
    m.price = (2.0, 10.0);
    m.featured = true;
    v.push(m);

    let mut m = base("claude-haiku-5-5", "Haiku 5.5", "Fastest model for daily tasks");
    m.thinking = Thinking::Disableable;
    m.default_effort = Some("medium");
    m.web_tools_v2 = false;
    m.price = (0.10, 0.50);
    m.featured = true;
    v.push(m);

    let mut m = base("claude-fable-5-2", "Fable 5.2", "Most intelligent model for the hardest problems");
    m.fallbacks = true;
    m.price = (8.0, 40.0);
    m.featured = true;
    v.push(m);

    let mut m = base("claude-fable-5-1", "Fable 5.1", "Previous Fable model");
    m.fallbacks = true;
    m.price = (10.0, 50.0);
    m.legacy = true;
    v.push(m);

    let mut m = base("claude-fable-5", "Fable 5", "Earlier Fable model");
    m.price = (10.0, 50.0);
    m.legacy = true;
    v.push(m);

    let mut m = base("claude-opus-5", "Opus 5", "Previous Opus model");
    m.thinking = Thinking::Disableable;
    m.fallbacks = true;
    v.push(m);

    let mut m = base("claude-sonnet-5", "Sonnet 5", "Previous Sonnet model");
    m.thinking = Thinking::Disableable;
    m.price = (2.0, 10.0);
    v.push(m);

    for (id, name) in [("claude-opus-4-8", "Opus 4.8"), ("claude-opus-4-7", "Opus 4.7")] {
        let mut m = base(id, name, "Legacy model");
        m.thinking = Thinking::Explicit;
        m.legacy = true;
        v.push(m);
    }

    let mut m = base("claude-opus-4-6", "Opus 4.6", "Legacy model");
    m.thinking = Thinking::Explicit;
    m.efforts = EFFORTS_46.to_vec();
    m.display_param = false;
    m.legacy = true;
    v.push(m);

    let mut m = base("claude-sonnet-4-6", "Sonnet 4.6", "Legacy model");
    m.thinking = Thinking::Explicit;
    m.efforts = EFFORTS_46.to_vec();
    m.display_param = false;
    m.price = (3.0, 15.0);
    m.legacy = true;
    v.push(m);

    let mut m = base("claude-haiku-4-5", "Haiku 4.5", "Legacy model");
    m.thinking = Thinking::Budget;
    m.efforts = vec![];
    m.default_effort = None;
    m.display_param = false;
    m.web_tools_v2 = false;
    m.compaction = false;
    m.max_output = 64_000;
    m.context = 200_000;
    m.price = (1.0, 5.0);
    m.legacy = true;
    v.push(m);

    let mut m = base("claude-opus-4-5", "Opus 4.5", "Legacy model");
    m.thinking = Thinking::Budget;
    m.efforts = vec!["low", "medium", "high"];
    m.display_param = false;
    m.web_tools_v2 = false;
    m.compaction = false;
    m.max_output = 64_000;
    m.context = 200_000;
    m.legacy = true;
    v.push(m);

    v
}

/// Look up a model, falling back to family heuristics for ids we don't know yet.
pub fn info(id: &str) -> ModelInfo {
    if let Some(m) = catalog().into_iter().find(|m| m.id == id) {
        return m;
    }
    let lower = id.to_ascii_lowercase();
    let pretty = pretty_name(id);
    let mut m = base(id, &pretty, "");
    m.legacy = true;
    if lower.contains("fable") || lower.contains("mythos") {
        // same surface as Fable 5.x
    } else if lower.contains("opus-5") || lower.contains("opus-6") {
        m.default_effort = Some("medium");
    } else if lower.contains("sonnet-5") || lower.contains("sonnet-6") {
        m.thinking = Thinking::Disableable;
    } else if lower.contains("haiku-5") || lower.contains("haiku-6") {
        m.thinking = Thinking::Disableable;
        m.web_tools_v2 = false;
    } else if lower.contains("-4-5") || lower.contains("3-") {
        m.thinking = Thinking::Budget;
        m.efforts = vec![];
        m.default_effort = None;
        m.display_param = false;
        m.web_tools_v2 = false;
        m.compaction = false;
        m.max_output = 64_000;
    } else {
        m.thinking = Thinking::Explicit;
        m.display_param = false;
        m.efforts = vec![];
        m.default_effort = None;
    }
    m
}

pub fn pretty_name(id: &str) -> String {
    // claude-opus-5-5 -> Opus 5.5
    let s = id.trim_start_matches("claude-");
    let mut parts: Vec<&str> = s.split('-').collect();
    // drop date suffixes like 20251001
    parts.retain(|p| !(p.len() == 8 && p.chars().all(|c| c.is_ascii_digit())));
    if parts.is_empty() {
        return id.to_string();
    }
    let fam = parts[0];
    let mut name = String::new();
    let mut chars = fam.chars();
    if let Some(f) = chars.next() {
        name.push(f.to_ascii_uppercase());
        name.push_str(chars.as_str());
    }
    let ver: Vec<&str> = parts[1..].iter().copied().filter(|p| p.chars().all(|c| c.is_ascii_digit())).collect();
    if !ver.is_empty() {
        name.push(' ');
        name.push_str(&ver.join("."));
    }
    name
}

pub struct ThinkingParams {
    pub thinking: Option<Value>,
    pub effort: Option<String>,
    pub max_tokens: u32,
}

/// Build the `thinking` / `output_config.effort` request fields for a model.
pub fn thinking_params(m: &ModelInfo, thinking_on: bool, effort: Option<&str>, max_tokens: u32) -> ThinkingParams {
    let mut max_tokens = max_tokens.min(m.max_output);
    let mut effort: Option<String> = effort
        .filter(|e| m.efforts.contains(e))
        .map(|e| e.to_string());
    let adaptive = || {
        if m.display_param {
            json!({"type": "adaptive", "display": "summarized"})
        } else {
            json!({"type": "adaptive"})
        }
    };
    let clamp_high = |e: &mut Option<String>| {
        if matches!(e.as_deref(), Some("xhigh") | Some("max")) {
            *e = Some("high".into());
        }
    };
    let thinking = match m.thinking {
        Thinking::AlwaysOn => Some(adaptive()),
        Thinking::Disableable => {
            if thinking_on {
                Some(adaptive())
            } else {
                clamp_high(&mut effort);
                Some(json!({"type": "disabled"}))
            }
        }
        Thinking::BetweenTools => {
            if thinking_on {
                Some(adaptive())
            } else {
                clamp_high(&mut effort);
                Some(json!({"type": "between_tools"}))
            }
        }
        Thinking::Explicit => {
            if thinking_on {
                Some(adaptive())
            } else {
                None
            }
        }
        Thinking::Budget => {
            if thinking_on {
                max_tokens = max_tokens.max(8192);
                let budget = (max_tokens / 2).clamp(1024, 32_000);
                Some(json!({"type": "enabled", "budget_tokens": budget}))
            } else {
                None
            }
        }
    };
    ThinkingParams {
        thinking,
        effort,
        max_tokens,
    }
}

/// Web search / fetch server tool definitions for a model.
pub fn web_tools(m: &ModelInfo) -> Vec<Value> {
    if m.web_tools_v2 {
        vec![
            json!({"type": "web_search_20260209", "name": "web_search", "max_uses": 8}),
            json!({"type": "web_fetch_20260209", "name": "web_fetch", "max_uses": 8}),
        ]
    } else {
        vec![
            json!({"type": "web_search_20250305", "name": "web_search", "max_uses": 8}),
            json!({"type": "web_fetch_20250910", "name": "web_fetch", "max_uses": 8}),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(pretty_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(pretty_name("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(pretty_name("claude-sonnet-5"), "Sonnet 5");
    }

    #[test]
    fn thinking_rules() {
        let opus = info("claude-opus-5-5");
        let p = thinking_params(&opus, false, Some("max"), 32000);
        assert_eq!(p.thinking.unwrap()["type"], "adaptive");
        assert_eq!(p.effort.as_deref(), Some("max"));

        let sonnet = info("claude-sonnet-5-5");
        let p = thinking_params(&sonnet, false, Some("max"), 32000);
        assert_eq!(p.thinking.unwrap()["type"], "between_tools");
        assert_eq!(p.effort.as_deref(), Some("high"));

        let haiku45 = info("claude-haiku-4-5");
        let p = thinking_params(&haiku45, true, Some("high"), 4000);
        let t = p.thinking.unwrap();
        assert_eq!(t["type"], "enabled");
        assert!(t["budget_tokens"].as_u64().unwrap() < p.max_tokens as u64);
        assert!(p.effort.is_none());

        let o48 = info("claude-opus-4-8");
        assert!(thinking_params(&o48, false, None, 1000).thinking.is_none());
    }
}
