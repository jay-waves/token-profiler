//! Pair hook inputs/results by turn and call ID, then aggregate visible content.
//! These figures describe introduced content; runtime billing is not attributable.
use crate::model::Event;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Default, Serialize)]
pub(crate) struct Call {
    pub event: usize,
    pub timestamp_ms: u128,
    pub turn: String,
    pub call: String,
    pub tool: String,
    pub summary: String,
    pub input: Option<usize>,
    pub result: Option<usize>,
}
impl Call {
    pub fn total(&self) -> usize {
        self.input
            .unwrap_or(0)
            .saturating_add(self.result.unwrap_or(0))
    }
}
#[derive(Default, Serialize)]
pub(crate) struct Tool {
    pub name: String,
    pub calls: usize,
    pub input: usize,
    pub result: usize,
}
#[derive(Default, Serialize)]
pub(crate) struct Turn {
    pub label: String,
    pub id: String,
    pub prompt: String,
    pub calls: usize,
    pub tokens: usize,
}
#[derive(Serialize)]
pub(crate) struct Breakdown {
    pub tools: Vec<Tool>,
    pub calls: Vec<Call>,
    pub turns: Vec<Turn>,
    pub total: usize,
}
impl Breakdown {
    pub fn new(events: &[Event]) -> Self {
        let mut calls = BTreeMap::<(String, String), Call>::new();
        let mut turns = BTreeMap::<String, Turn>::new();
        let mut active = "unscoped".to_owned();
        let mut total = 0usize;
        for (index, e) in events.iter().enumerate() {
            if e.kind == "UserPromptSubmit" {
                active = if !e.turn.is_empty() {
                    e.turn.clone()
                } else {
                    events[index + 1..]
                        .iter()
                        .take_while(|next| next.kind != "UserPromptSubmit")
                        .find(|next| !next.turn.is_empty())
                        .map(|next| next.turn.clone())
                        .unwrap_or_else(|| format!("prompt-E{}", e.id))
                };
            }
            let key = if e.turn.is_empty() {
                active.clone()
            } else {
                e.turn.clone()
            };
            if !e.turn.is_empty() {
                active = key.clone();
            }
            let Some(source) = e.source.as_deref() else {
                continue;
            };
            let next_label = format!("Turn {}", turns.len() + 1);
            let turn = turns.entry(key.clone()).or_insert_with(|| Turn {
                label: if key == "unscoped" {
                    "Unscoped".into()
                } else {
                    next_label
                },
                id: key.clone(),
                ..Turn::default()
            });
            let tokens = e.estimated_tokens();
            turn.tokens = turn.tokens.saturating_add(tokens);
            total = total.saturating_add(tokens);
            if source == "user_input" {
                if let Some(summary) = &e.summary {
                    turn.prompt = summary.clone();
                }
                continue;
            }
            if !matches!(source, "tool_input" | "tool_result") {
                continue;
            }
            // Missing call IDs cannot be safely paired, even with adjacent events.
            let call_key = if e.call.is_empty() {
                format!("event-E{}", e.id)
            } else {
                format!("call:{}", e.call)
            };
            let call = calls.entry((key, call_key)).or_insert_with(|| {
                turn.calls += 1;
                Call {
                    event: e.id,
                    timestamp_ms: e.timestamp_ms,
                    turn: turn.label.clone(),
                    call: e.call.clone(),
                    ..Call::default()
                }
            });
            if !e.tool.is_empty() {
                call.tool = e.tool.clone();
            }
            if let Some(summary) = &e.summary {
                call.summary = summary.clone();
            }
            if source == "tool_input" {
                call.input = Some(call.input.unwrap_or(0).saturating_add(tokens));
            } else {
                call.result = Some(call.result.unwrap_or(0).saturating_add(tokens));
            }
        }
        let mut tools = BTreeMap::<String, Tool>::new();
        for call in calls.values() {
            let name = if call.tool.is_empty() {
                "Unknown tool"
            } else {
                &call.tool
            };
            let tool = tools.entry(name.into()).or_insert_with(|| Tool {
                name: name.into(),
                ..Tool::default()
            });
            tool.calls += 1;
            tool.input = tool.input.saturating_add(call.input.unwrap_or(0));
            tool.result = tool.result.saturating_add(call.result.unwrap_or(0));
        }
        let mut tools: Vec<_> = tools.into_values().collect();
        tools.sort_by_key(|t| std::cmp::Reverse(t.input.saturating_add(t.result)));
        let mut calls: Vec<_> = calls.into_values().collect();
        calls.sort_by_key(|c| (std::cmp::Reverse(c.total()), c.event));
        let mut turns: Vec<_> = turns.into_values().collect();
        turns.sort_by_key(|t| std::cmp::Reverse(t.tokens));
        Self {
            tools,
            calls,
            turns,
            total,
        }
    }
}
