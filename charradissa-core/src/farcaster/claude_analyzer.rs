use async_trait::async_trait;
use serde::Deserialize;
use crate::error::{CharradissaError, Result};
use crate::types::ProjectId;
use super::analyzer::{
    CrossSpaceConnection, CrossSpaceSnapshot, DigestEntry, DigestSynthesis, FarcasterAnalyzer,
};
use super::concurrence::Urgency;
use super::milestone::MilestoneEvent;

const HAIKU_MODEL: &str = "claude-haiku-4-5-20251001";
const OPUS_MODEL: &str = "claude-opus-4-8";
const REACTIVE_MAX_TOKENS: u32 = 512;
const DIGEST_MAX_TOKENS: u32 = 4096;

pub struct ClaudeFarcasterAnalyzer {
    client: reqwest::Client,
    anthropic_api_key: String,
    xai_api_key: Option<String>,
}

impl ClaudeFarcasterAnalyzer {
    pub fn new(anthropic_api_key: String, xai_api_key: Option<String>) -> Self {
        Self { 
            client: reqwest::Client::new(), 
            anthropic_api_key,
            xai_api_key 
        }
    }

    async fn call_llm(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<(String, u32)> {
        if model.starts_with("grok") || model.starts_with("xai") {
            let api_key = self.xai_api_key.as_ref()
                .ok_or_else(|| CharradissaError::Dispatch("XAI_API_KEY not set for grok model".into()))?;
            let body = serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user}
                ]
            });

            let resp = self.client
                .post("https://api.x.ai/v1/chat/completions")
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send().await
                .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

            let data: serde_json::Value = resp.json().await
                .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

            let text = data["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            // xAI doesn't return exact token counts in same way, approximate
            let tokens = (text.len() / 4) as u32; 
            return Ok((text, tokens));
        }

        // Anthropic path
        let body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [{"role": "user", "content": user}]
        });

        let resp = self.client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.anthropic_api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

        let data: serde_json::Value = resp.json().await
            .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

        if let Some(err) = data.get("error") {
            return Err(CharradissaError::Dispatch(err.to_string()));
        }

        let text = data["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let tokens = data["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;

        Ok((text, tokens))
    }
}

#[derive(Deserialize)]
struct ConnectionJson {
    from_project: String,
    to_project: String,
    connection_type: String,
    summary: String,
    urgency: String,
}

fn parse_urgency(s: &str) -> Urgency {
    match s.to_lowercase().as_str() {
        "high" => Urgency::High,
        "medium" => Urgency::Medium,
        _ => Urgency::Low,
    }
}

#[derive(Deserialize)]
struct SynthesisJson {
    connections: Vec<String>,
    lessons: Vec<String>,
    open_questions: Vec<String>,
    farga_verdict: String,
    farga_title: Option<String>,
    farga_narrative: Option<String>,
}

const REACTIVE_SYSTEM: &str = "\
You are a cross-project intelligence analyzer. You receive a triggering milestone event and a snapshot of active projects. Identify cross-space connections that are actionable and specific. Return ONLY a JSON array — no prose. If no meaningful connection exists, return [].

Each item: {\"from_project\": str, \"to_project\": str, \"connection_type\": str, \"summary\": str, \"urgency\": str}
connection_type: shared_dependency | solved_problem | conflict | convergence_opportunity
urgency: low | medium | high";

const DIGEST_SYSTEM: &str = "\
You are a cross-project intelligence synthesizer. Review these cross-space connections observed over the past period. Return ONLY a JSON object — no prose.

Format: {\"connections\": [...str], \"lessons\": [...str], \"open_questions\": [...str], \"farga_verdict\": \"submit\"|\"skip\", \"farga_title\": str|null, \"farga_narrative\": str|null}
Set farga_verdict to \"submit\" only when lessons are substantive enough to contribute to organizational memory.";

#[async_trait]
impl FarcasterAnalyzer for ClaudeFarcasterAnalyzer {
    async fn analyze_cross_space(
        &self,
        triggering_event: &MilestoneEvent,
        snapshot: &CrossSpaceSnapshot,
    ) -> Result<(Vec<CrossSpaceConnection>, u32)> {
        let snapshot_text: Vec<String> = snapshot.projects.iter().map(|p| {
            format!(
                "Project: {}\nGoal: {}\nOpen: {}\nRecent events:\n{}",
                p.project_id,
                p.mission_goal.as_deref().unwrap_or("(unknown)"),
                p.open_sub_objectives.join(", "),
                p.recent_events.join("\n  "),
            )
        }).collect();

        let user = format!(
            "Triggering event: {}\n\nProject snapshots:\n{}",
            triggering_event.summary(),
            snapshot_text.join("\n\n")
        );

        let (text, tokens) = self.call_llm(HAIKU_MODEL, REACTIVE_SYSTEM, &user, REACTIVE_MAX_TOKENS).await?;

        // Extract JSON array from response (may be wrapped in markdown code block)
        let json_str = extract_json(&text);
        let items: Vec<ConnectionJson> = serde_json::from_str(json_str)
            .map_err(|e| CharradissaError::Dispatch(format!("failed to parse reactive response: {}: {}", e, text)))?;

        let connections = items.into_iter().map(|item| CrossSpaceConnection {
            from_project: ProjectId::new(&item.from_project),
            to_project: ProjectId::new(&item.to_project),
            connection_type: item.connection_type,
            summary: item.summary,
            urgency: parse_urgency(&item.urgency),
        }).collect();

        Ok((connections, tokens))
    }

    async fn synthesize_digest(&self, entries: &[DigestEntry]) -> Result<(DigestSynthesis, u32)> {
        let entries_text: Vec<String> = entries.iter().map(|e| {
            format!(
                "- [{} → {}] {} (urgency: {:?}, whispered: {})",
                e.involved_projects.first().map(|p| p.as_str()).unwrap_or("?"),
                e.involved_projects.get(1).map(|p| p.as_str()).unwrap_or("?"),
                e.connection_summary,
                e.urgency,
                e.whispered_at.is_some()
            )
        }).collect();

        let user = format!("Connections observed:\n{}", entries_text.join("\n"));

        let (text, tokens) = self.call_llm(OPUS_MODEL, DIGEST_SYSTEM, &user, DIGEST_MAX_TOKENS).await?;

        let json_str = extract_json(&text);
        let parsed: SynthesisJson = serde_json::from_str(json_str)
            .map_err(|e| CharradissaError::Dispatch(format!("failed to parse digest response: {}: {}", e, text)))?;

        Ok((DigestSynthesis {
            connections: parsed.connections,
            lessons: parsed.lessons,
            open_questions: parsed.open_questions,
            farga_verdict: parsed.farga_verdict,
            farga_title: parsed.farga_title,
            farga_narrative: parsed.farga_narrative,
        }, tokens))
    }
}

fn extract_json(text: &str) -> &str {
    // Strip markdown code fences if present
    if let Some(start) = text.find("```json") {
        let after = &text[start + 7..];
        if let Some(end) = after.find("```") {
            return after[..end].trim();
        }
    }
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        if let Some(end) = after.find("```") {
            return after[..end].trim();
        }
    }
    // Find the first [ or { and return from there
    let start = text.find('[').or_else(|| text.find('{')).unwrap_or(0);
    &text[start..]
}
