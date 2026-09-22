//! Minimal Google Tasks v1 client.

use chrono::NaiveDate;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use crate::auth::{pct, Auth, Result};

const BASE: &str = "https://tasks.googleapis.com/tasks/v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskList {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub notes: Option<String>,
    pub status: String,
    /// RFC 3339, but only the date part is meaningful.
    pub due: Option<String>,
    pub completed: Option<String>,
    pub updated: String,
    pub parent: Option<String>,
    pub position: String,
    pub hidden: bool,
    pub deleted: bool,
    pub web_view_link: Option<String>,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Link {
    pub description: String,
    pub link: String,
}

impl Task {
    pub fn is_done(&self) -> bool {
        self.status == "completed"
    }

    pub fn due_date(&self) -> Option<NaiveDate> {
        let s = self.due.as_deref()?.get(..10)?;
        NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
    next_page_token: Option<String>,
}

pub struct Client {
    auth: Auth,
}

impl Client {
    pub fn new(auth: Auth) -> Self {
        Self { auth }
    }

    pub fn tasklists(&mut self) -> Result<Vec<TaskList>> {
        self.get_all(&format!("{BASE}/users/@me/lists"), &[])
    }

    /// Every task in a list, including completed and hidden ones.
    pub fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>> {
        self.get_all(
            &format!("{BASE}/lists/{}/tasks", pct(list_id)),
            &[("showCompleted", "true"), ("showHidden", "true")],
        )
    }

    /// Patch the given fields of a task; returns it as Google stored it.
    pub fn patch(&mut self, list_id: &str, task_id: &str, fields: &Value) -> Result<Task> {
        let token = self.auth.token()?;
        let task = self
            .auth
            .agent
            .patch(format!("{BASE}/lists/{}/tasks/{}", pct(list_id), pct(task_id)))
            .header("Authorization", format!("Bearer {token}"))
            .send_json(fields)?
            .body_mut()
            .read_json()?;
        Ok(task)
    }

    /// Add a task to the top of a list, under `parent` when given.
    pub fn create(&mut self, list_id: &str, title: &str, parent: Option<&str>) -> Result<Task> {
        self.insert(list_id, &serde_json::json!({ "title": title }), parent)
    }

    fn insert(&mut self, list_id: &str, body: &Value, parent: Option<&str>) -> Result<Task> {
        let token = self.auth.token()?;
        let mut req = self
            .auth
            .agent
            .post(format!("{BASE}/lists/{}/tasks", pct(list_id)))
            .header("Authorization", format!("Bearer {token}"));
        if let Some(p) = parent {
            req = req.query("parent", p);
        }
        Ok(req.send_json(body)?.body_mut().read_json()?)
    }

    /// Delete a task. Google deletes its subtasks along with it.
    pub fn delete(&mut self, list_id: &str, task_id: &str) -> Result<()> {
        let token = self.auth.token()?;
        self.auth
            .agent
            .delete(format!("{BASE}/lists/{}/tasks/{}", pct(list_id), pct(task_id)))
            .header("Authorization", format!("Bearer {token}"))
            .call()?;
        Ok(())
    }

    /// Move a task into another list. Omit `parent` to make it top-level.
    pub fn relocate(
        &mut self,
        list_id: &str,
        task_id: &str,
        dest_list: &str,
        parent: Option<&str>,
    ) -> Result<Task> {
        let token = self.auth.token()?;
        let mut req = self
            .auth
            .agent
            .post(format!(
                "{BASE}/lists/{}/tasks/{}/move",
                pct(list_id),
                pct(task_id)
            ))
            .header("Authorization", format!("Bearer {token}"))
            .query("destinationTasklist", dest_list);
        if let Some(p) = parent {
            req = req.query("parent", p);
        }
        Ok(req.send_empty()?.body_mut().read_json()?)
    }

    /// Create a task list.
    pub fn create_list(&mut self, title: &str) -> Result<TaskList> {
        let token = self.auth.token()?;
        Ok(self
            .auth
            .agent
            .post(format!("{BASE}/users/@me/lists"))
            .header("Authorization", format!("Bearer {token}"))
            .send_json(serde_json::json!({ "title": title }))?
            .body_mut()
            .read_json()?)
    }

    /// Patch the given fields of a task list; returns it as Google stored it.
    pub fn patch_list(&mut self, list_id: &str, fields: &Value) -> Result<TaskList> {
        let token = self.auth.token()?;
        Ok(self
            .auth
            .agent
            .patch(format!("{BASE}/users/@me/lists/{}", pct(list_id)))
            .header("Authorization", format!("Bearer {token}"))
            .send_json(fields)?
            .body_mut()
            .read_json()?)
    }

    /// Delete a task list. Google deletes every task in it.
    pub fn delete_list(&mut self, list_id: &str) -> Result<()> {
        let token = self.auth.token()?;
        self.auth
            .agent
            .delete(format!("{BASE}/users/@me/lists/{}", pct(list_id)))
            .header("Authorization", format!("Bearer {token}"))
            .call()?;
        Ok(())
    }

    /// Follow `nextPageToken` until the collection is exhausted.
    fn get_all<T: DeserializeOwned>(&mut self, url: &str, query: &[(&str, &str)]) -> Result<Vec<T>> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let token = self.auth.token()?;
            let mut req = self
                .auth
                .agent
                .get(url)
                .header("Authorization", format!("Bearer {token}"))
                .query("maxResults", "100");
            for (k, v) in query {
                req = req.query(*k, *v);
            }
            if let Some(p) = &page_token {
                req = req.query("pageToken", p);
            }
            let page: Page<T> = req.call()?.body_mut().read_json()?;
            out.extend(page.items);
            match page.next_page_token {
                Some(p) => page_token = Some(p),
                None => return Ok(out),
            }
        }
    }
}

/// The Google Tasks operations the CLI (and tests) need. `Client` is the
/// real implementation; tests use a fake.
pub trait TasksApi {
    fn tasklists(&mut self) -> Result<Vec<TaskList>>;
    fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>>;
    fn patch(&mut self, list_id: &str, task_id: &str, fields: &Value) -> Result<Task>;
    fn create(&mut self, list_id: &str, body: &Value, parent: Option<&str>) -> Result<Task>;
    fn delete(&mut self, list_id: &str, task_id: &str) -> Result<()>;
    fn create_list(&mut self, title: &str) -> Result<TaskList>;
    fn patch_list(&mut self, list_id: &str, fields: &Value) -> Result<TaskList>;
    fn delete_list(&mut self, list_id: &str) -> Result<()>;
}

impl TasksApi for Client {
    fn tasklists(&mut self) -> Result<Vec<TaskList>> {
        Client::tasklists(self)
    }

    fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>> {
        Client::tasks(self, list_id)
    }

    fn patch(&mut self, list_id: &str, task_id: &str, fields: &Value) -> Result<Task> {
        Client::patch(self, list_id, task_id, fields)
    }

    fn create(&mut self, list_id: &str, body: &Value, parent: Option<&str>) -> Result<Task> {
        self.insert(list_id, body, parent)
    }

    fn delete(&mut self, list_id: &str, task_id: &str) -> Result<()> {
        Client::delete(self, list_id, task_id)
    }

    fn create_list(&mut self, title: &str) -> Result<TaskList> {
        Client::create_list(self, title)
    }

    fn patch_list(&mut self, list_id: &str, fields: &Value) -> Result<TaskList> {
        Client::patch_list(self, list_id, fields)
    }

    fn delete_list(&mut self, list_id: &str) -> Result<()> {
        Client::delete_list(self, list_id)
    }
}
