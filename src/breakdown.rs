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
    pub category: String,
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
#[derive(Default, Serialize)]
pub(crate) struct Category {
    pub name: String,
    pub calls: usize,
    pub input: usize,
    pub result: usize,
}
#[derive(Serialize)]
pub(crate) struct Breakdown {
    pub tools: Vec<Tool>,
    pub categories: Vec<Category>,
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
            call.category = classify(&call.tool, &call.summary);
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
        let mut categories = BTreeMap::<String, Category>::new();
        for call in &calls {
            let category = categories
                .entry(call.category.clone())
                .or_insert_with(|| Category {
                    name: call.category.clone(),
                    ..Category::default()
                });
            category.calls += 1;
            category.input = category.input.saturating_add(call.input.unwrap_or(0));
            category.result = category.result.saturating_add(call.result.unwrap_or(0));
        }
        let mut categories: Vec<_> = categories.into_values().collect();
        categories.sort_by_key(|c| std::cmp::Reverse(c.input.saturating_add(c.result)));
        let mut turns: Vec<_> = turns.into_values().collect();
        turns.sort_by_key(|t| std::cmp::Reverse(t.tokens));
        Self {
            tools,
            categories,
            calls,
            turns,
            total,
        }
    }
}

/// Group each call by its sorted, distinct executable names. Arguments do not
/// contribute names; shell separators inside quotes and escaped separators are literal.
fn classify(tool: &str, command: &str) -> String {
    let mut names = std::collections::BTreeSet::new();
    let mut word = String::new();
    let mut words = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        if comment {
            if c != '\n' {
                continue;
            }
            comment = false;
        }
        if escaped {
            if c != '\n' {
                word.push(c);
            }
            escaped = false;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
        } else if c == '#' && word.is_empty() {
            comment = true;
        } else if matches!(c, '|' | '&' | ';' | '\n' | '(' | ')') {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            command_name(&words, &mut names);
            words.clear();
        } else if matches!(c, '>' | '<') {
            // A descriptor duplication such as 2>&1 is not a new command.
            if word.chars().all(|c| c.is_ascii_digit()) {
                word.clear();
            } else if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            while chars.peek().is_some_and(|c| matches!(c, '>' | '<' | '&')) {
                chars.next();
            }
            // Everything after the executable is already treated as arguments.
        } else if c.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    command_name(&words, &mut names);
    if names.is_empty() {
        if command.trim().is_empty() && !tool.is_empty() {
            tool.into()
        } else {
            "Unknown".into()
        }
    } else {
        names.into_iter().collect::<Vec<_>>().join(" & ")
    }
}

fn command_name(words: &[String], names: &mut std::collections::BTreeSet<String>) {
    for word in words {
        // Assignment prefixes and common shell wrappers are not executables.
        if word.split_once('=').is_some_and(|(key, _)| {
            !key.is_empty()
                && key.chars().enumerate().all(|(i, c)| {
                    c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                })
        }) || matches!(word.as_str(), "sudo" | "env" | "command" | "exec")
        {
            continue;
        }
        let name = word.rsplit('/').next().unwrap_or(word);
        if !name.is_empty() {
            names.insert(name.into());
        }
        break;
    }
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn command_combinations_are_sorted_and_unique() {
        for (command, expected) in [
            ("rg foo src", "rg"),
            ("head -n 20 file", "head"),
            ("rg foo src | head", "head & rg"),
            ("head file && rg foo src", "head & rg"),
            ("cat a && cat b", "cat"),
            ("/usr/bin/rg foo; cat file", "cat & rg"),
            ("rg 'foo|bar; baz' src", "rg"),
            ("rg \"foo && bar\" src | head", "head & rg"),
            (r"rg foo\|bar src", "rg"),
            ("rg foo src # cat ignored\nhead file", "head & rg"),
            ("LANG=C sudo /usr/bin/rg foo", "rg"),
            ("cat file 2>&1 | head", "cat & head"),
            ("python3 script.py", "python3"),
        ] {
            assert_eq!(classify("Bash", command), expected, "{command}");
        }
        assert_eq!(classify("read_file", ""), "read_file");
        assert_eq!(classify("", ""), "Unknown");
    }
}
