use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub org: OrgConfig,
    pub backend: BackendConfig,
    pub concierge: ConciergeConfig,
    pub approval: ApprovalConfig,
    pub tasks: TasksConfig,
    pub projects: ProjectsConfig,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub component_agents: Vec<ComponentAgentConfig>,
    #[serde(default)]
    pub provisioning: ProvisioningConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ComponentAgentConfig {
    /// Component name, e.g. "amassada". Used as a display label.
    pub name: String,
    /// The Matrix room ID this agent inhabits, e.g. "!abc123:occitane.guilhem".
    pub room_id: String,
    /// System prompt for this agent. Use the context field from its Fondament definition.
    pub system_prompt: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProvisioningConfig {
    #[serde(default = "default_provisioning_projects")]
    pub projects: Vec<String>,
    pub fondament_url: Option<String>,
}

fn default_provisioning_projects() -> Vec<String> {
    vec!["occitan".into()]
}

impl Default for ProvisioningConfig {
    fn default() -> Self {
        Self {
            projects: default_provisioning_projects(),
            fondament_url: None,
        }
    }
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

/// Per-room agent routing.  All rooms fall back to `default` (or the
/// GUILHEM_URL env var) when no explicit route is set.  Routes are keyed by
/// Matrix room ID (`!<opaque>:<server>`).
///
/// Example charradissa.toml fragment:
/// ```toml
/// [agents]
/// default = "http://guilhem.agents.svc.cluster.local:8080"
///
/// [agents.routes]
/// "!abc123:occitane.guilhem" = "http://amassada-agent.agents.svc.cluster.local:8080"
///
/// [[agents.project]]
/// type = "amassada_backed"
/// rooms = ["!proj-room:occitane.guilhem"]
/// project_id = "my-project"
/// endpoint = "http://amassada:7700/sessions/{room_id}/message"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AgentsConfig {
    /// Fallback agent URL.  If absent, GUILHEM_URL env var is used.
    pub default: Option<String>,
    /// room_id → agent URL overrides.
    #[serde(default)]
    pub routes: HashMap<String, String>,
    /// Project agent configurations — each entry covers one or more Matrix rooms
    /// backed by a specific Amassada project persona.
    #[serde(default)]
    pub project: Vec<ProjectAgentConfig>,
}

/// Configuration for an Amassada-backed project agent.
///
/// Each entry maps a set of Matrix rooms to an Amassada endpoint and a project id.
/// Charradissa resolves the project from this table and injects `project_id` into
/// the turn body it sends to Amassada so that Amassada can select the right persona.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProjectAgentConfig {
    /// Must be `"amassada_backed"`.
    #[serde(rename = "type")]
    pub agent_type: String,
    /// Matrix room IDs served by this project agent.
    pub rooms: Vec<String>,
    /// The project identifier transmitted to Amassada in every turn.
    pub project_id: String,
    /// Amassada endpoint template.  `{room_id}` is substituted at call time.
    pub endpoint: String,
}

impl Config {
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }
}

/// Default appservice webhook listen port.
pub const DEFAULT_LISTEN_PORT: &str = "8448";

/// Resolve the daemon's appservice webhook listen port from the environment.
///
/// Reads `CHARRADISSA_LISTEN_PORT`, falling back to [`DEFAULT_LISTEN_PORT`].
///
/// This variable is deliberately *not* named `CHARRADISSA_PORT`: the kubelet
/// injects a legacy service-link variable `CHARRADISSA_PORT=tcp://<ip>:<port>`
/// for the in-cluster `charradissa` Service, which collided with the daemon's
/// own port config (issue #10). The rename lets us drop the
/// `enableServiceLinks: false` workaround from the Helm chart.
pub fn listen_port() -> String {
    std::env::var("CHARRADISSA_LISTEN_PORT").unwrap_or_else(|_| DEFAULT_LISTEN_PORT.into())
}

/// Resolve the appservice HS token (the token Synapse sends to Charradissa).
///
/// Reads `MATRIX_HS_TOKEN`. If unset, falls back to `as_token` (backward-compat
/// with deployments that use a single shared token).
pub fn hs_token(as_token: &str) -> String {
    std::env::var("MATRIX_HS_TOKEN").unwrap_or_else(|_| as_token.to_string())
}
