use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use serde::Deserialize;
use crate::backend::ChatBackend;
use crate::error::{CharradissaError, Result};
use crate::farga::FargaWriter;
use crate::types::{ProjectId, RoomId};
use super::analyzer::DigestSynthesis;
use super::concurrence::Urgency;
use super::governance::{GovernanceContribution, FargaLayer};
use super::milestone::MilestoneEvent;
use super::observation::DomainDigestEvent;
use super::system_agent::SystemAgent;
use super::upstream::UpstreamObserver;

const CROSS_DOMAIN_ROOM: &str = "#farcaster-cross-domain";
const BUFFER_CAP: usize = 30;

// ─── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DomainSnapshot {
    pub domain: String,
    /// Summaries of the most recent digests received from this domain.
    pub recent_digests: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CrossDomainSnapshot {
    pub domains: Vec<DomainSnapshot>,
}

#[derive(Debug, Clone)]
pub struct CrossDomainConnection {
    pub from_domain: String,
    pub to_domain: String,
    pub connection_type: String,
    pub summary: String,
    pub urgency: Urgency,
}

#[derive(Debug, Clone)]
pub struct CrossDomainEntry {
    pub from_domain: String,
    pub connection_summary: String,
    pub involved_domains: Vec<String>,
    pub urgency: Urgency,
    pub first_observed_at: DateTime<Utc>,
}

// ─── Analyzer trait ───────────────────────────────────────────────────────────

#[async_trait]
pub trait CrossDomainAnalyzer: Send + Sync {
    async fn analyze_cross_domain(
        &self,
        triggering_digest: &DomainDigestEvent,
        snapshot: &CrossDomainSnapshot,
    ) -> Result<(Vec<CrossDomainConnection>, u32)>;

    async fn synthesize_cross_domain_digest(
        &self,
        entries: &[CrossDomainEntry],
    ) -> Result<(DigestSynthesis, u32)>;
}

// ─── CrossDomainFarcaster ─────────────────────────────────────────────────────

pub struct CrossDomainFarcaster {
    pub(crate) backend: Arc<dyn ChatBackend>,
    pub(crate) farga: Arc<dyn FargaWriter>,
    pub(crate) analyzer: Arc<dyn CrossDomainAnalyzer>,
    /// Per-domain ring buffer of recent digests for snapshot building.
    domain_buffer: Mutex<HashMap<String, VecDeque<DomainDigestEvent>>>,
    pub entry_buffer: Mutex<Vec<CrossDomainEntry>>,
    pub(crate) daily_reactive_token_budget: u32,
    pub(crate) daily_digest_token_budget: u32,
    pub reactive_tokens_used: AtomicU32,
    pub digest_tokens_used: AtomicU32,
}

impl CrossDomainFarcaster {
    pub fn new(
        backend: Arc<dyn ChatBackend>,
        farga: Arc<dyn FargaWriter>,
        analyzer: Arc<dyn CrossDomainAnalyzer>,
    ) -> Self {
        Self {
            backend,
            farga,
            analyzer,
            domain_buffer: Mutex::new(HashMap::new()),
            entry_buffer: Mutex::new(Vec::new()),
            daily_reactive_token_budget: 30_000,
            daily_digest_token_budget: 15_000,
            reactive_tokens_used: AtomicU32::new(0),
            digest_tokens_used: AtomicU32::new(0),
        }
    }

    async fn build_snapshot(&self) -> CrossDomainSnapshot {
        let buf = self.domain_buffer.lock().await;
        let domains = buf.iter().map(|(domain, digests)| {
            DomainSnapshot {
                domain: domain.clone(),
                recent_digests: digests.iter().rev().take(3)
                    .map(|d| d.summary())
                    .collect(),
            }
        }).collect();
        CrossDomainSnapshot { domains }
    }

    async fn handle_domain_digest_inner(&self, event: &DomainDigestEvent) -> Result<()> {
        // Buffer the incoming digest
        {
            let mut buf = self.domain_buffer.lock().await;
            let ring = buf.entry(event.domain.clone()).or_default();
            if ring.len() >= BUFFER_CAP { ring.pop_front(); }
            ring.push_back(event.clone());
        }

        // Budget check — defer to next synthesis tick
        if self.reactive_tokens_used.load(Ordering::Relaxed) >= self.daily_reactive_token_budget {
            tracing::info!("cross-domain farcaster: reactive budget exhausted, deferring");
            self.entry_buffer.lock().await.push(CrossDomainEntry {
                from_domain: event.domain.clone(),
                connection_summary: format!("deferred (budget): {}", event.summary()),
                involved_domains: vec![event.domain.clone()],
                urgency: Urgency::Low,
                first_observed_at: Utc::now(),
            });
            return Ok(());
        }

        let snapshot = self.build_snapshot().await;

        let (connections, tokens) = match self.analyzer.analyze_cross_domain(event, &snapshot).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("cross-domain farcaster: analyze failed: {}", e);
                return Ok(());
            }
        };
        self.reactive_tokens_used.fetch_add(tokens, Ordering::Relaxed);

        let now = Utc::now();
        let room = RoomId::new(CROSS_DOMAIN_ROOM);
        for conn in &connections {
            if matches!(conn.urgency, Urgency::Medium | Urgency::High) {
                let msg = format!(
                    "[cross-domain] {} × {}: {}",
                    conn.from_domain, conn.to_domain, conn.summary
                );
                if let Err(e) = self.backend.send_message(&room, &msg).await {
                    tracing::warn!("cross-domain farcaster: whisper failed: {}", e);
                }
            }
        }

        {
            let mut buf = self.entry_buffer.lock().await;
            for conn in connections {
                buf.push(CrossDomainEntry {
                    from_domain: conn.from_domain.clone(),
                    connection_summary: conn.summary,
                    involved_domains: vec![conn.from_domain, conn.to_domain],
                    urgency: conn.urgency,
                    first_observed_at: now,
                });
            }
        }

        Ok(())
    }

    pub async fn run_tick(&self) -> Result<()> {
        let entries = {
            let mut buf = self.entry_buffer.lock().await;
            if buf.is_empty() { return Ok(()); }
            std::mem::take(&mut *buf)
        };

        if self.digest_tokens_used.load(Ordering::Relaxed) >= self.daily_digest_token_budget {
            tracing::info!("cross-domain farcaster: digest budget exhausted, deferring");
            self.entry_buffer.lock().await.extend(entries);
            return Ok(());
        }

        let (synthesis, tokens) = match self.analyzer.synthesize_cross_domain_digest(&entries).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("cross-domain farcaster: synthesize failed: {}", e);
                self.entry_buffer.lock().await.extend(entries);
                return Ok(());
            }
        };
        self.digest_tokens_used.fetch_add(tokens, Ordering::Relaxed);

        let room = RoomId::new(CROSS_DOMAIN_ROOM);
        if let Err(e) = self.backend.send_message(&room, &format_digest(&synthesis)).await {
            tracing::warn!("cross-domain farcaster: broadcast failed: {}", e);
        }

        if synthesis.farga_verdict == "submit" {
            let involved_projects: Vec<ProjectId> = {
                let mut seen = std::collections::HashSet::new();
                entries.iter()
                    .flat_map(|e| e.involved_domains.iter().map(|d| ProjectId::new(d)))
                    .filter(|p| seen.insert(p.clone()))
                    .collect()
            };

            let contribution = GovernanceContribution {
                title: synthesis.farga_title.clone().unwrap_or_default(),
                narrative: synthesis.farga_narrative.clone().unwrap_or_default(),
                lessons: synthesis.lessons.clone(),
                open_questions: synthesis.open_questions.clone(),
                involved_projects,
                concurrence: vec![],
                target_layer: FargaLayer::OrgLevel,
                first_observed_at: entries.iter()
                    .map(|e| e.first_observed_at).min()
                    .unwrap_or_else(Utc::now),
                last_observed_at: Utc::now(),
                event_count: entries.len() as u32,
                reversibility: None,
                impact: None,
            };

            if let Err(e) = self.farga.submit_governance_contribution(contribution).await {
                tracing::error!("cross-domain farcaster: org-level Farga submission failed: {}", e);
                self.entry_buffer.lock().await.extend(entries);
            }
        }

        Ok(())
    }
}

#[async_trait]
impl UpstreamObserver for CrossDomainFarcaster {
    async fn on_domain_digest(&self, event: DomainDigestEvent) -> Result<()> {
        self.handle_domain_digest_inner(&event).await
    }
}

#[async_trait]
impl SystemAgent for CrossDomainFarcaster {
    fn name(&self) -> &str { "cross-domain-farcaster" }
    async fn on_milestone(&self, _: &MilestoneEvent) -> Result<()> { Ok(()) }
    async fn tick(&self) -> Result<()> { self.run_tick().await }
}

fn format_digest(synthesis: &DigestSynthesis) -> String {
    let mut out = String::from("## Cross-Domain Digest\n\n");
    if !synthesis.connections.is_empty() {
        out.push_str("### Cross-Domain Connections\n");
        for c in &synthesis.connections { out.push_str(&format!("- {}\n", c)); }
        out.push('\n');
    }
    if !synthesis.lessons.is_empty() {
        out.push_str("### Organizational Lessons\n");
        for l in &synthesis.lessons { out.push_str(&format!("- {}\n", l)); }
        out.push('\n');
    }
    if !synthesis.open_questions.is_empty() {
        out.push_str("### Open Questions\n");
        for q in &synthesis.open_questions { out.push_str(&format!("- {}\n", q)); }
    }
    out
}

// ─── Claude implementation ────────────────────────────────────────────────────

const HAIKU_MODEL: &str = "claude-haiku-4-5-20251001";
const OPUS_MODEL: &str = "claude-opus-4-8";
const REACTIVE_MAX_TOKENS: u32 = 512;
const DIGEST_MAX_TOKENS: u32 = 4096;

const CROSS_DOMAIN_REACTIVE_SYSTEM: &str = "\
You are a cross-domain intelligence analyzer. You receive a triggering domain digest and a snapshot of all active domains. Identify cross-domain connections that are actionable and specific. Return ONLY a JSON array — no prose. If no meaningful connection exists, return [].

Each item: {\"from_domain\": str, \"to_domain\": str, \"connection_type\": str, \"summary\": str, \"urgency\": str}
connection_type: shared_dependency | solved_problem | conflict | convergence_opportunity | architectural_impact
urgency: low | medium | high";

const CROSS_DOMAIN_DIGEST_SYSTEM: &str = "\
You are a cross-domain intelligence synthesizer. Review these cross-domain connections observed over the past period. Return ONLY a JSON object — no prose.

Format: {\"connections\": [...str], \"lessons\": [...str], \"open_questions\": [...str], \"farga_verdict\": \"submit\"|\"skip\", \"farga_title\": str|null, \"farga_narrative\": str|null}
Set farga_verdict to \"submit\" only when lessons are substantive enough to contribute to organizational memory at the org-wide level. These are cross-domain patterns, not project-level ones.";

pub struct ClaudeCrossDomainAnalyzer {
    client: reqwest::Client,
    api_key: String,
}

impl ClaudeCrossDomainAnalyzer {
    pub fn new(api_key: String) -> Self {
        Self { client: reqwest::Client::new(), api_key }
    }

    async fn call_claude(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<(String, u32)> {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [{"role": "user", "content": user}]
        });

        let resp = self.client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
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

        let text = data["content"][0]["text"].as_str().unwrap_or("").to_string();
        let tokens = data["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
        Ok((text, tokens))
    }
}

#[derive(Deserialize)]
struct CrossDomainConnectionJson {
    from_domain: String,
    to_domain: String,
    connection_type: String,
    summary: String,
    urgency: String,
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

fn parse_urgency(s: &str) -> Urgency {
    match s.to_lowercase().as_str() {
        "high" => Urgency::High,
        "medium" => Urgency::Medium,
        _ => Urgency::Low,
    }
}

fn extract_json(text: &str) -> &str {
    if let Some(start) = text.find("```json") {
        let after = &text[start + 7..];
        if let Some(end) = after.find("```") { return after[..end].trim(); }
    }
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        if let Some(end) = after.find("```") { return after[..end].trim(); }
    }
    let start = text.find('[').or_else(|| text.find('{')).unwrap_or(0);
    &text[start..]
}

#[async_trait]
impl CrossDomainAnalyzer for ClaudeCrossDomainAnalyzer {
    async fn analyze_cross_domain(
        &self,
        triggering_digest: &DomainDigestEvent,
        snapshot: &CrossDomainSnapshot,
    ) -> Result<(Vec<CrossDomainConnection>, u32)> {
        let domain_texts: Vec<String> = snapshot.domains.iter().map(|d| {
            format!(
                "Domain: {}\nRecent digests:\n  {}",
                d.domain,
                d.recent_digests.join("\n  ")
            )
        }).collect();

        let user = format!(
            "Triggering domain digest: {}\nNarrative: {}\nLessons: {}\n\nAll domain snapshots:\n{}",
            triggering_digest.summary(),
            triggering_digest.narrative,
            triggering_digest.lessons.join("; "),
            domain_texts.join("\n\n")
        );

        let (text, tokens) = self.call_claude(HAIKU_MODEL, CROSS_DOMAIN_REACTIVE_SYSTEM, &user, REACTIVE_MAX_TOKENS).await?;
        let json_str = extract_json(&text);
        let items: Vec<CrossDomainConnectionJson> = serde_json::from_str(json_str)
            .map_err(|e| CharradissaError::Dispatch(format!("parse error: {}: {}", e, text)))?;

        let connections = items.into_iter().map(|item| CrossDomainConnection {
            from_domain: item.from_domain,
            to_domain: item.to_domain,
            connection_type: item.connection_type,
            summary: item.summary,
            urgency: parse_urgency(&item.urgency),
        }).collect();

        Ok((connections, tokens))
    }

    async fn synthesize_cross_domain_digest(
        &self,
        entries: &[CrossDomainEntry],
    ) -> Result<(DigestSynthesis, u32)> {
        let entries_text: Vec<String> = entries.iter().map(|e| {
            format!(
                "- [{} → {}] {} (urgency: {:?})",
                e.involved_domains.first().map(|s| s.as_str()).unwrap_or("?"),
                e.involved_domains.get(1).map(|s| s.as_str()).unwrap_or("?"),
                e.connection_summary,
                e.urgency,
            )
        }).collect();

        let user = format!("Cross-domain connections observed:\n{}", entries_text.join("\n"));
        let (text, tokens) = self.call_claude(OPUS_MODEL, CROSS_DOMAIN_DIGEST_SYSTEM, &user, DIGEST_MAX_TOKENS).await?;
        let json_str = extract_json(&text);
        let parsed: SynthesisJson = serde_json::from_str(json_str)
            .map_err(|e| CharradissaError::Dispatch(format!("parse error: {}: {}", e, text)))?;

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
