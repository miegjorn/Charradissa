use async_trait::async_trait;
use charradissa_core::error::{CharradissaError, Result};
use charradissa_core::task::TaskManager;
use charradissa_core::types::*;

pub struct JiraTaskManager {
    client: reqwest::Client,
    base_url: String,
    project_key: String,
    api_token: String,
    email: String,
}

impl JiraTaskManager {
    pub fn new(base_url: String, project_key: String, api_token: String, email: String) -> Self {
        Self { client: reqwest::Client::new(), base_url, project_key, api_token, email }
    }

    fn auth(&self) -> String {
        use base64::Engine;
        let creds = format!("{}:{}", self.email, self.api_token);
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(creds.as_bytes()))
    }
}

#[async_trait]
impl TaskManager for JiraTaskManager {
    async fn create_task(&self, _project: &ProjectId, opts: &TaskOptions) -> Result<TaskId> {
        let url = format!("{}/rest/api/3/issue", self.base_url);
        let body = serde_json::json!({
            "fields": {
                "project": { "key": self.project_key },
                "summary": opts.title,
                "description": {
                    "type": "doc",
                    "version": 1,
                    "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": opts.description }] }]
                },
                "issuetype": { "name": "Task" }
            }
        });
        let resp = self.client.post(&url)
            .header("Authorization", self.auth())
            .header("Content-Type", "application/json")
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Tool(e.to_string()))?;
        let json: serde_json::Value = resp.json().await
            .map_err(|e| CharradissaError::Tool(e.to_string()))?;
        let id = json["id"].as_str()
            .ok_or_else(|| CharradissaError::Tool("no id in Jira response".into()))?;
        Ok(TaskId::new(id))
    }

    async fn assign_task(&self, task: &TaskId, assignee: &Assignee) -> Result<()> {
        tracing::info!("assign task {} to {:?}", task.as_str(), assignee);
        Ok(())
    }

    async fn update_status(&self, task: &TaskId, status: TaskStatus) -> Result<()> {
        tracing::info!("update task {} -> {:?}", task.as_str(), status);
        Ok(())
    }

    async fn get_task(&self, _task: &TaskId) -> Result<Task> {
        Err(CharradissaError::Tool("get_task: not implemented".into()))
    }

    async fn list_open(&self, _project: &ProjectId) -> Result<Vec<Task>> {
        Ok(vec![])
    }
}
