// placeholder — filled in Task 2
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoomId(String);

impl RoomId {
    pub fn new(s: &str) -> Self { Self(s.to_string()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for RoomId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(String);

impl UserId {
    pub fn new(s: &str) -> Self { Self(s.to_string()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpaceId(String);

impl SpaceId {
    pub fn new(s: &str) -> Self { Self(s.to_string()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn new(s: &str) -> Self { Self(s.to_string()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(String);

impl TaskId {
    pub fn new(s: &str) -> Self { Self(s.to_string()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompositionAddress {
    Role { role: String, stance_override: Option<String> },
    Composed { project: String, facet: String, stance: String },
}

impl std::fmt::Display for CompositionAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Role { role, stance_override } => {
                if let Some(s) = stance_override {
                    write!(f, "fondament/{}/{}", role, s)
                } else {
                    write!(f, "fondament/{}", role)
                }
            }
            Self::Composed { project, facet, stance } => {
                write!(f, "{}/{}+{}", project, facet, stance)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEvent {
    pub event_id: String,
    pub room_id: RoomId,
    pub sender: UserId,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub kind: ChatEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChatEventKind {
    Message,
    SlashCommand { command: String, args: String },
    Mention,
    Reaction,
    MemberJoin,
    MemberLeave,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomOptions {
    pub alias: String,
    pub name: String,
    pub topic: Option<String>,
    pub invite: Vec<UserId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub assignee: Option<Assignee>,
    pub project: ProjectId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Open, InProgress, InReview, Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Assignee {
    Human(UserId),
    Agent(CompositionAddress),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskOptions {
    pub title: String,
    pub description: String,
    pub assignee: Option<Assignee>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectHealth {
    pub project: ProjectId,
    pub open_tasks: Vec<Task>,
    pub active_session: Option<String>,
    pub last_concierge_sweep: Option<DateTime<Utc>>,
}
