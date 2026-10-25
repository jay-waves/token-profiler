use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Default, Clone, Serialize, Deserialize, Debug, PartialEq)]
pub(crate) struct Usage {
    pub input: u64,
    pub cached: Option<u64>,
    pub output: u64,
    pub reasoning: Option<u64>,
}
impl Usage {
    pub(crate) fn from_value(v: &Value) -> Option<Self> {
        let u = Self {
            input: v["input_tokens"].as_u64()?,
            cached: counter(
                v,
                "cached_input_tokens",
                "/input_tokens_details/cached_tokens",
            )?,
            output: v["output_tokens"].as_u64()?,
            reasoning: counter(
                v,
                "reasoning_output_tokens",
                "/output_tokens_details/reasoning_tokens",
            )?,
        };
        (u.cached.is_none_or(|n| n <= u.input) && u.reasoning.is_none_or(|n| n <= u.output))
            .then_some(u)
    }
    pub(crate) fn delta(&self, base: &Self) -> Option<Self> {
        let input = self.input.checked_sub(base.input)?;
        let output = self.output.checked_sub(base.output)?;
        let cached = delta_optional(self.cached, base.cached)?;
        let reasoning = delta_optional(self.reasoning, base.reasoning)?;
        (cached.is_none_or(|n| n <= input) && reasoning.is_none_or(|n| n <= output)).then_some(
            Self {
                input,
                cached,
                output,
                reasoning,
            },
        )
    }
    pub(crate) fn high_water<'a>(values: impl IntoIterator<Item = &'a Self>) -> Option<Self> {
        values.into_iter().fold(None::<Self>, |acc, v| {
            Some(match acc {
                None => v.clone(),
                Some(mut max) => {
                    max.input = max.input.max(v.input);
                    max.output = max.output.max(v.output);
                    max.cached = option_max(max.cached, v.cached);
                    max.reasoning = option_max(max.reasoning, v.reasoning);
                    max
                }
            })
        })
    }
}
fn counter(v: &Value, top: &str, nested: &str) -> Option<Option<u64>> {
    match v
        .get(top)
        .filter(|x| !x.is_null())
        .or_else(|| v.pointer(nested))
    {
        None | Some(Value::Null) => Some(None),
        Some(n) => n.as_u64().map(Some),
    }
}
fn delta_optional(a: Option<u64>, b: Option<u64>) -> Option<Option<u64>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(Some(a.checked_sub(b)?)),
        _ => Some(None),
    }
}
fn option_max(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Event {
    #[serde(skip)]
    pub id: usize,
    pub timestamp_ms: u128,
    pub kind: String,
    pub turn: String,
    pub call: String,
    pub tool: String,
    pub source: Option<String>,
    pub bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}
impl Event {
    /// Content-size heuristic, not a tokenizer or billed usage. Structured payload
    /// sizes include JSON overhead; repeated history processing is not counted.
    pub(crate) fn estimated_tokens(&self) -> usize {
        self.bytes.div_ceil(4)
    }
    pub(crate) fn from_hook(v: &Value) -> Option<Self> {
        let kind = field(v, "hook_event_name");
        let (source, value) = match kind {
            "UserPromptSubmit" => ("user_input", &v["prompt"]),
            "PreToolUse" => ("tool_input", &v["tool_input"]),
            "PostToolUse" => ("tool_result", &v["tool_response"]),
            _ => {
                return Some(Self {
                    id: 0,
                    timestamp_ms: now(),
                    kind: kind.into(),
                    turn: field(v, "turn_id").into(),
                    call: field(v, "tool_use_id").into(),
                    tool: field(v, "tool_name").into(),
                    source: None,
                    bytes: 0,
                    summary: None,
                });
            }
        };
        if value.is_null() {
            return None;
        }
        let bytes = content_size(value);
        Some(Self {
            id: 0,
            timestamp_ms: now(),
            kind: kind.into(),
            turn: field(v, "turn_id").into(),
            call: field(v, "tool_use_id").into(),
            tool: field(v, "tool_name").into(),
            source: Some(source.into()),
            bytes,
            summary: match source {
                "user_input" => value.as_str().map(excerpt),
                "tool_input" => input_summary(value),
                "tool_result" => input_summary(&v["tool_input"]),
                _ => None,
            },
        })
    }
}
pub(crate) fn content_size(value: &Value) -> usize {
    value
        .as_str()
        .map(str::len)
        .unwrap_or_else(|| value.to_string().len())
}
pub(crate) fn field<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
pub(crate) fn now() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Bounded local labels make hotspots identifiable without retaining full payloads.
fn excerpt(text: &str) -> String {
    let mut chars = text
        .split_whitespace()
        .flat_map(|word| word.chars().chain(std::iter::once(' ')));
    let mut out: String = chars.by_ref().take(160).collect();
    if chars.next().is_some() {
        out = out.chars().take(159).collect();
        out.push('…');
    }
    out.trim_end().to_owned()
}
fn input_summary(value: &Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let text = ["command", "cmd", "code", "query", "pattern", "path", "url"]
        .iter()
        .find_map(|key| value[*key].as_str())
        .or_else(|| value.as_str());
    Some(match text {
        Some(text) => excerpt(text),
        None => excerpt(&value.to_string()),
    })
}
