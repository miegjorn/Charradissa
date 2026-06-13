// placeholder — filled in Task 5
use serde::{Deserialize, Serialize};
use crate::types::RoomId;

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const MAX_TOOL_ROUNDS: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Approve { id: String },
    Reject { id: String, reason: String },
    Session { canvas_id: String, goal: String },
    Invite { address: String },
    Call,
}

pub fn parse_slash_command(text: &str) -> Option<SlashCommand> {
    let text = text.trim();
    if !text.starts_with('/') { return None; }

    let parts: Vec<&str> = text[1..].splitn(3, ' ').collect();
    match parts[0] {
        "approve" => {
            let id = parts.get(1)?.to_string();
            Some(SlashCommand::Approve { id })
        }
        "reject" => {
            let id = parts.get(1)?.to_string();
            let reason = parts.get(2).copied().unwrap_or("no reason given").to_string();
            Some(SlashCommand::Reject { id, reason })
        }
        "session" => {
            let canvas_id = parts.get(1)?.to_string();
            let goal = parts.get(2)
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_else(|| "unspecified goal".into());
            Some(SlashCommand::Session { canvas_id, goal })
        }
        "invite" => {
            let address = parts.get(1)?.to_string();
            Some(SlashCommand::Invite { address })
        }
        "call" => Some(SlashCommand::Call),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub requires_approval: bool,
    pub approval_category: Option<String>,
}

pub struct ToolLoopConfig {
    pub model: String,
    pub system_prompt: String,
    pub context: String,
    pub room_id: RoomId,
    pub max_rounds: u32,
}

impl Default for ToolLoopConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.into(),
            system_prompt: String::new(),
            context: String::new(),
            room_id: RoomId::new(""),
            max_rounds: MAX_TOOL_ROUNDS,
        }
    }
}

pub fn requires_approval(tool_name: &str) -> (bool, Option<String>) {
    match tool_name {
        t if t.starts_with("git_") || t.starts_with("pr_") || t.starts_with("code_") => {
            (true, Some("code".into()))
        }
        t if t.starts_with("db_") || t.starts_with("sql_") => {
            (true, Some("db".into()))
        }
        t if t.starts_with("infra_") || t.starts_with("aws_") || t.starts_with("tf_") => {
            (true, Some("infra".into()))
        }
        _ => (false, None),
    }
}
