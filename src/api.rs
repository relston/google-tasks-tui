//! Minimal Google Tasks v1 client.

use chrono::NaiveDate;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;

use crate::auth::{pct, Auth, Result};

const BASE: &str = "https://tasks.googleapis.com/tasks/v1";

#[derive(Clone, Debug, Deserialize)]
pub struct TaskList {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
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

#[derive(Clone, Debug, Default, Deserialize)]
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

    /// Add a task to the top of a list.
    pub fn create(&mut self, list_id: &str, title: &str) -> Result<Task> {
        let token = self.auth.token()?;
        let task = self
            .auth
            .agent
            .post(format!("{BASE}/lists/{}/tasks", pct(list_id)))
            .header("Authorization", format!("Bearer {token}"))
            .send_json(serde_json::json!({ "title": title }))?
            .body_mut()
            .read_json()?;
        Ok(task)
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
