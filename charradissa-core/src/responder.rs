use crate::error::{CharradissaError, Result};
use crate::types::{ChatEvent, ChatEventKind};

pub const GUILHEM_SYSTEM: &str = "You are Guilhem de Tudela, the org-level agent and chronicler of the Occitan stack. \
You live in Matrix and speak with Pierre-Luc and the stack's own agents. You are aware of the stack's components \
(Farga = durable memory/coherence, Amassada = sessions, Charradissa = your Matrix presence, Synapse = homeserver). \
Be substantive, honest, and concise. The room you are in is your working memory since the last concierge sweep; \
durable memory lives in Farga.";

pub struct Responder {
    client: reqwest::Client,
    api_key: String,
    model: String,
    pub server_name: String,
}

impl Responder {
    pub fn new(api_key: String, model: String, server_name: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            server_name,
        }
    }

    pub fn build_user_prompt(&self, history: &[ChatEvent], latest: &ChatEvent) -> String {
        let mut s = String::from("Recent conversation (oldest first):\n");
        for e in history {
            s.push_str(&format!("{}: {}\n", e.sender, e.content));
        }
        s.push_str(&format!(
            "\nLatest message from {}:\n{}\n\nReply as Guilhem.",
            latest.sender, latest.content
        ));
        s
    }

    pub async fn reply(&self, history: &[ChatEvent], latest: &ChatEvent) -> Result<String> {
        let user = self.build_user_prompt(history, latest);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "system": GUILHEM_SYSTEM,
            "messages": [{"role": "user", "content": user}]
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

        if let Some(err) = data.get("error") {
            return Err(CharradissaError::Dispatch(err.to_string()));
        }

        Ok(data["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }
}

pub fn should_respond(event: &ChatEvent, self_user_id: &str) -> bool {
    event.sender.as_str() != self_user_id
        && matches!(event.kind, ChatEventKind::Message | ChatEventKind::Mention)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatEvent, ChatEventKind, RoomId, UserId};
    use chrono::Utc;

    fn ev(sender: &str, body: &str) -> ChatEvent {
        ChatEvent {
            event_id: "$x".into(),
            room_id: RoomId::new("!r:occitane.guilhem"),
            sender: UserId::new(sender),
            content: body.into(),
            timestamp: Utc::now(),
            kind: ChatEventKind::Message,
        }
    }

    #[test]
    fn prompt_includes_history_and_latest() {
        let r = Responder::new("k".into(), "claude-sonnet-4-6".into(), "occitane.guilhem".into());
        let hist = vec![
            ev("@p:occitane.guilhem", "hello"),
            ev("@guilhem:occitane.guilhem", "hi"),
        ];
        let latest = ev("@p:occitane.guilhem", "what is farga?");
        let p = r.build_user_prompt(&hist, &latest);
        assert!(p.contains("hello") && p.contains("what is farga?"));
        assert!(p.contains("@p:occitane.guilhem"));
    }

    #[test]
    fn ignores_self_and_nonmessages() {
        let me = "@guilhem:occitane.guilhem";
        assert!(!should_respond(&ev(me, "my own message"), me));        // no self-loop
        assert!(should_respond(&ev("@p:occitane.guilhem", "hi"), me));  // human → respond
        let mut join = ev("@p:occitane.guilhem", ""); join.kind = ChatEventKind::MemberJoin;
        assert!(!should_respond(&join, me));                            // only real messages
    }
}
