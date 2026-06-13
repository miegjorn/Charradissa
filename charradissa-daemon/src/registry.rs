use std::collections::HashMap;
use charradissa_core::types::{ProjectId, UserId};

pub struct AgentRegistry {
    project_agents: HashMap<ProjectId, UserId>,
    org_agent: Option<UserId>,
    concierge: Option<UserId>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self { project_agents: HashMap::new(), org_agent: None, concierge: None }
    }

    pub fn register_project_agent(&mut self, project: ProjectId, user_id: UserId) {
        self.project_agents.insert(project, user_id);
    }

    pub fn register_org_agent(&mut self, user_id: UserId) {
        self.org_agent = Some(user_id);
    }

    pub fn register_concierge(&mut self, user_id: UserId) {
        self.concierge = Some(user_id);
    }

    pub fn project_agent(&self, project: &ProjectId) -> Option<&UserId> {
        self.project_agents.get(project)
    }
}
