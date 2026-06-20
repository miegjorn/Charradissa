use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub org: OrgConfig,
    pub backend: BackendConfig,
    pub concierge: ConciergeConfig,
    pub approval: ApprovalConfig,
    pub tasks: TasksConfig,
    pub projects: ProjectsConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrgConfig {
    pub name: String,
    pub homeserver: String,
    #[serde(default = "default_server_name")]
    pub server_name: String,
}

fn default_server_name() -> String { "occitane.guilhem".into() }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BackendConfig {
    #[serde(rename = "type")]
    pub backend_type: String, // "matrix" | "irc"
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConciergeConfig {
    #[serde(default = "default_archival_interval")]
    pub archival_interval_hours: u64,
    #[serde(default = "default_convergence_interval")]
    pub convergence_interval_hours: u64,
    #[serde(default = "default_daily_budget")]
    pub daily_token_budget: u32,
}

fn default_archival_interval() -> u64 { 24 }
fn default_convergence_interval() -> u64 { 6 }
fn default_daily_budget() -> u32 { 50_000 }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApprovalConfig {
    #[serde(default = "default_timeout")]
    pub timeout_minutes: u64,
}

fn default_timeout() -> u64 { 60 }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TasksConfig {
    #[serde(rename = "type")]
    pub tasks_type: String, // "jira" | "none"
    pub base_url: Option<String>,
    pub project_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProjectsConfig {
    #[serde(default = "default_true")]
    pub autodiscover: bool,
}

fn default_true() -> bool { true }

impl Config {
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }
}
