use crate::attention::AttentionItem;
use crate::RelatedContent;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct SynthesisInput {
    pub anchor_description: String,
    pub session_types: Vec<String>,
    pub key_turns: Vec<TurnContent>,
    pub related_content: Vec<RelatedContent>,
    pub attention_items: Vec<AttentionItem>,
    pub commits: Vec<String>,
    pub decisions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnContent {
    pub turn_index: usize,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicResponse {
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentBlock {
    pub text: String,
}

pub async fn synthesize_recap(input: SynthesisInput) -> Result<crate::RecapOutput> {
    let api_key = std::env::var("EDDA_LLM_API_KEY");

    match api_key {
        Ok(key) if !key.is_empty() => synthesize_with_llm(&key, input).await,
        _ => synthesize_with_template(input),
    }
}

async fn synthesize_with_llm(api_key: &str, input: SynthesisInput) -> Result<crate::RecapOutput> {
    let client = Client::new();

    let prompt = build_prompt(&input);

    let request = AnthropicRequest {
        model: "claude-3-5-haiku-20241022".to_string(),
        max_tokens: 1024,
        messages: vec![Message {
            role: "user".to_string(),
            content: prompt,
        }],
    };

    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .with_context(|| "Failed to call Anthropic API")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(%status, %body, "LLM API failed");
        return synthesize_with_template(input);
    }

    let api_response: AnthropicResponse = response
        .json()
        .await
        .with_context(|| "Failed to parse Anthropic response")?;

    let text = api_response
        .content
        .first()
        .map(|block| block.text.as_str())
        .unwrap_or("");

    parse_llm_output(text)
}

fn synthesize_with_template(input: SynthesisInput) -> Result<crate::RecapOutput> {
    let mut net_result = String::new();
    // Net result: summarize sessions
    if !input.session_types.is_empty() {
        net_result = format!("Sessions: {}", input.session_types.join(", "));
    }

    if !input.commits.is_empty() {
        net_result.push_str(&format!("\nCommits: {}", input.commits.len()));
    }

    if !input.decisions.is_empty() {
        net_result.push_str(&format!("\nDecisions: {}", input.decisions.len()));
    }

    // Needs you: attention items
    let needs_you = if !input.attention_items.is_empty() {
        input
            .attention_items
            .iter()
            .map(|item| format!("• {}", item.description))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        "No blockers detected".to_string()
    };

    // Decision context: decisions
    let decision_context = if !input.decisions.is_empty() {
        input.decisions.join("\n")
    } else {
        "No decisions recorded".to_string()
    };

    // Relations: related content
    let relations = if !input.related_content.is_empty() {
        input
            .related_content
            .iter()
            .map(|rc| format!("• {} (from {})", rc.snippet, rc.source))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        "No related historical content found".to_string()
    };

    Ok(crate::RecapOutput {
        net_result,
        needs_you,
        decision_context,
        relations,
    })
}

fn build_prompt(input: &SynthesisInput) -> String {
    format!(
        r#"你是开发认知助手。请分析以下开发活动并输出四层结构。

## Anchor
{}

## 行为分类
{}

## 关键 Transcript 段落
{}

## 历史关联
{}

## Ledger Events
Commits: {}
Decisions: {}

## 需要你关注的事项
{}

请输出（每层2-3句话，简洁有力）：
1. 淨結果 — 最終什麼留下了（不是過程）
2. 需要你 — 你不介入就不會前進的事
3. 決策脈絡 — 做了什麼決定、否決了什麼、為什麼
4. 關聯 — 跟過去或其他 repo 的連結

格式：
## 淨結果
[内容]

## 需要你
[内容]

## 決策脈絡
[内容]

## 關聯
[内容]"#,
        input.anchor_description,
        input.session_types.join(", "),
        input
            .key_turns
            .iter()
            .map(|t| format!("Turn {}: {}", t.turn_index, t.content))
            .collect::<Vec<_>>()
            .join("\n"),
        input
            .related_content
            .iter()
            .map(|rc| format!("• {}", rc.snippet))
            .collect::<Vec<_>>()
            .join("\n"),
        input.commits.len(),
        input.decisions.len(),
        input
            .attention_items
            .iter()
            .map(|a| format!("• {}", a.description))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn parse_llm_output(text: &str) -> Result<crate::RecapOutput> {
    let mut net_result = String::new();
    let mut needs_you = String::new();
    let mut decision_context = String::new();
    let mut relations = String::new();

    let lines: Vec<&str> = text.lines().collect();
    let mut current_section = None;

    for line in lines {
        let line = line.trim();

        if line.starts_with("## 淨結果") || line.starts_with("## 净结果") {
            current_section = Some("net_result");
        } else if line.starts_with("## 需要你") {
            current_section = Some("needs_you");
        } else if line.starts_with("## 決策脈絡") || line.starts_with("## 决策脉络") {
            current_section = Some("decision_context");
        } else if line.starts_with("## 關聯") || line.starts_with("## 关联") {
            current_section = Some("relations");
        } else if !line.is_empty() {
            match current_section {
                Some("net_result") => {
                    if !net_result.is_empty() {
                        net_result.push('\n');
                    }
                    net_result.push_str(line);
                }
                Some("needs_you") => {
                    if !needs_you.is_empty() {
                        needs_you.push('\n');
                    }
                    needs_you.push_str(line);
                }
                Some("decision_context") => {
                    if !decision_context.is_empty() {
                        decision_context.push('\n');
                    }
                    decision_context.push_str(line);
                }
                Some("relations") => {
                    if !relations.is_empty() {
                        relations.push('\n');
                    }
                    relations.push_str(line);
                }
                _ => {}
            }
        }
    }

    Ok(crate::RecapOutput {
        net_result,
        needs_you,
        decision_context,
        relations,
    })
}
