// placeholder — filled in Task 3
use async_trait::async_trait;
use crate::error::Result;
use crate::types::{Assignee, ProjectId, Task, TaskId, TaskOptions, TaskStatus};

#[async_trait]
pub trait TaskManager: Send + Sync {
    async fn create_task(&self, project: &ProjectId, opts: &TaskOptions) -> Result<TaskId>;
    async fn assign_task(&self, task: &TaskId, assignee: &Assignee) -> Result<()>;
    async fn update_status(&self, task: &TaskId, status: TaskStatus) -> Result<()>;
    async fn get_task(&self, task: &TaskId) -> Result<Task>;
    async fn list_open(&self, project: &ProjectId) -> Result<Vec<Task>>;
}
