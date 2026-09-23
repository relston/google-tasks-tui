//! Minimal Google Tasks v1 client.

use std::{fmt, thread, time::Duration};

use chrono::NaiveDate;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use crate::auth::{pct, Auth};

const BASE: &str = "https://tasks.googleapis.com/tasks/v1";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TaskList {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub etag: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Task {
    pub id: String,
    pub etag: String,
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
    pub assignment_info: Option<AssignmentInfo>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Link {
    pub description: String,
    pub link: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AssignmentInfo {
    pub link_to_task: String,
    pub surface_type: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTask {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<Option<String>>,
}

#[derive(Clone, Debug)]
pub struct MoveTask {
    pub destination_list: String,
    pub parent: Option<String>,
    pub previous: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ListPatch {
    pub title: String,
}

#[derive(Debug)]
pub enum Error {
    AuthExpired,
    AssignedTask(String),
    Conflict,
    RateLimited { retry_after: Option<Duration> },
    Timeout,
    Http { status: u16, message: String },
    Transport(String),
    Decode(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthExpired => f.write_str("sign-in expired"),
            Self::AssignedTask(message) => f.write_str(message),
            Self::Conflict => f.write_str("task changed in Google; refresh and try again"),
            Self::RateLimited { retry_after } => match retry_after {
                Some(delay) => write!(f, "Google rate limit; retry after {}s", delay.as_secs()),
                None => f.write_str("Google rate limit"),
            },
            Self::Timeout => f.write_str("Google request timed out"),
            Self::Http { status, message } => write!(f, "Google HTTP {status}: {message}"),
            Self::Transport(message) | Self::Decode(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

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

    fn request(
        &mut self,
        safe_to_retry: bool,
        mut send: impl FnMut(
            &ureq::Agent,
            &str,
        ) -> std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let agent = self.auth.agent.clone();
        for attempt in 0..3 {
            let token = self.token()?;
            let mut response = send(&agent, &token).map_err(map_ureq)?;
            let status = response.status().as_u16();
            if status == 401 && attempt == 0 {
                self.auth.force_refresh();
                continue;
            }
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs);
            if should_retry(status, safe_to_retry, attempt) {
                thread::sleep(retry_after.unwrap_or(Duration::from_millis(200)).min(
                    Duration::from_secs(2),
                ));
                continue;
            }
            if (200..300).contains(&status) {
                return Ok(response);
            }
            let message = response
                .body_mut()
                .read_to_string()
                .ok()
                .and_then(|body| {
                    serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|value| {
                            value
                                .pointer("/error/message")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                        .or_else(|| (!body.is_empty()).then_some(body))
                })
                .unwrap_or_else(|| "request failed".into());
            return Err(match status {
                401 => Error::AuthExpired,
                409 | 412 => Error::Conflict,
                429 => Error::RateLimited { retry_after },
                status => Error::Http { status, message },
            });
        }
        unreachable!("request retry loop always returns")
    }

    pub fn tasklists(&mut self) -> Result<Vec<TaskList>> {
        self.get_all(&format!("{BASE}/users/@me/lists"), &[])
    }

    /// Every task in a list, including completed and hidden ones.
    pub fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>> {
        self.get_all(
            &format!("{BASE}/lists/{}/tasks", pct(list_id)),
            &[
                ("showCompleted", "true"),
                ("showHidden", "true"),
                ("showAssigned", "true"),
            ],
        )
    }

    /// Patch the given fields of a task; returns it as Google stored it.
    pub fn patch(
        &mut self,
        list_id: &str,
        task_id: &str,
        patch: &TaskPatch,
        etag: Option<&str>,
    ) -> Result<Task> {
        let url = format!("{BASE}/lists/{}/tasks/{}", pct(list_id), pct(task_id));
        let etag = etag.filter(|etag| !etag.is_empty()).map(str::to_string);
        let task = self
            .request(false, |agent, token| {
                let mut request = agent
                    .patch(&url)
                    .header("Authorization", format!("Bearer {token}"));
                if let Some(etag) = &etag {
                    request = request.header("If-Match", etag);
                }
                request.send_json(patch)
            })?
            .body_mut()
            .read_json()
            .map_err(map_ureq)?;
        Ok(task)
    }

    /// Add a task to the top of a list, under `parent` when given.
    pub fn create(
        &mut self,
        list_id: &str,
        task: &CreateTask,
        parent: Option<&str>,
    ) -> Result<Task> {
        let url = format!("{BASE}/lists/{}/tasks", pct(list_id));
        let parent = parent.map(str::to_string);
        self.request(false, |agent, token| {
                let mut request = agent
                    .post(&url)
                    .header("Authorization", format!("Bearer {token}"));
                if let Some(parent) = &parent {
                    request = request.query("parent", parent);
                }
                request.send_json(task)
            })?
            .body_mut()
            .read_json()
            .map_err(map_ureq)
    }

    /// Delete a task. Google deletes its subtasks along with it.
    pub fn delete(
        &mut self,
        list_id: &str,
        task_id: &str,
        etag: Option<&str>,
    ) -> Result<()> {
        let url = format!("{BASE}/lists/{}/tasks/{}", pct(list_id), pct(task_id));
        let etag = etag.filter(|etag| !etag.is_empty()).map(str::to_string);
        self.request(false, |agent, token| {
            let mut request = agent
                .delete(&url)
                .header("Authorization", format!("Bearer {token}"));
            if let Some(etag) = &etag {
                request = request.header("If-Match", etag);
            }
            request.call()
        })?;
        Ok(())
    }

    /// Move a task into another list. Omit `parent` to make it top-level.
    pub fn move_task(
        &mut self,
        list_id: &str,
        task_id: &str,
        movement: &MoveTask,
    ) -> Result<Task> {
        let url = format!(
            "{BASE}/lists/{}/tasks/{}/move",
            pct(list_id),
            pct(task_id)
        );
        self.request(false, |agent, token| {
                let mut request = agent
                    .post(&url)
                    .header("Authorization", format!("Bearer {token}"))
                    .query("destinationTasklist", &movement.destination_list);
                if let Some(parent) = &movement.parent {
                    request = request.query("parent", parent);
                }
                if let Some(previous) = &movement.previous {
                    request = request.query("previous", previous);
                }
                request.send_empty()
            })?
            .body_mut()
            .read_json()
            .map_err(map_ureq)
    }

    /// Create a task list.
    pub fn create_list(&mut self, title: &str) -> Result<TaskList> {
        let patch = ListPatch {
            title: title.to_string(),
        };
        self.request(false, |agent, token| {
                agent
                    .post(format!("{BASE}/users/@me/lists"))
                    .header("Authorization", format!("Bearer {token}"))
                    .send_json(&patch)
            })?
            .body_mut()
            .read_json()
            .map_err(map_ureq)
    }

    /// Patch the given fields of a task list; returns it as Google stored it.
    pub fn patch_list(
        &mut self,
        list_id: &str,
        patch: &ListPatch,
        etag: Option<&str>,
    ) -> Result<TaskList> {
        let url = format!("{BASE}/users/@me/lists/{}", pct(list_id));
        let etag = etag.filter(|etag| !etag.is_empty()).map(str::to_string);
        self.request(false, |agent, token| {
                let mut request = agent
                    .patch(&url)
                    .header("Authorization", format!("Bearer {token}"));
                if let Some(etag) = &etag {
                    request = request.header("If-Match", etag);
                }
                request.send_json(patch)
            })?
            .body_mut()
            .read_json()
            .map_err(map_ureq)
    }

    /// Delete a task list. Google deletes every task in it.
    pub fn delete_list(&mut self, list_id: &str) -> Result<()> {
        let url = format!("{BASE}/users/@me/lists/{}", pct(list_id));
        self.request(false, |agent, token| {
            agent
                .delete(&url)
                .header("Authorization", format!("Bearer {token}"))
                .call()
        })?;
        Ok(())
    }

    /// Follow `nextPageToken` until the collection is exhausted.
    fn get_all<T: DeserializeOwned>(&mut self, url: &str, query: &[(&str, &str)]) -> Result<Vec<T>> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let page: Page<T> = self
                .request(true, |agent, token| {
                    let mut request = agent
                        .get(url)
                        .header("Authorization", format!("Bearer {token}"))
                        .query("maxResults", "100");
                    for (key, value) in query {
                        request = request.query(*key, *value);
                    }
                    if let Some(page_token) = &page_token {
                        request = request.query("pageToken", page_token);
                    }
                    request.call()
                })?
                .body_mut()
                .read_json()
                .map_err(map_ureq)?;
            out.extend(page.items);
            match page.next_page_token {
                Some(p) => page_token = Some(p),
                None => return Ok(out),
            }
        }
    }

    fn token(&mut self) -> Result<String> {
        self.auth.token().map_err(|error| {
            if error.downcast_ref::<crate::auth::SignInExpired>().is_some() {
                Error::AuthExpired
            } else {
                Error::Transport(error.to_string())
            }
        })
    }
}

fn map_ureq(error: ureq::Error) -> Error {
    match error {
        ureq::Error::StatusCode(401) => Error::AuthExpired,
        ureq::Error::StatusCode(409 | 412) => Error::Conflict,
        ureq::Error::StatusCode(429) => Error::RateLimited { retry_after: None },
        ureq::Error::StatusCode(status) => Error::Http {
            status,
            message: "request failed".into(),
        },
        ureq::Error::Timeout(_) => Error::Timeout,
        ureq::Error::Json(error) => Error::Decode(error.to_string()),
        error => Error::Transport(error.to_string()),
    }
}

fn should_retry(status: u16, safe_to_retry: bool, attempt: usize) -> bool {
    safe_to_retry && attempt < 2 && (status == 429 || (500..600).contains(&status))
}

/// The Google Tasks operations the CLI (and tests) need. `Client` is the
/// real implementation; tests use a fake.
pub trait TasksApi {
    fn account_fingerprint(&self) -> String;
    fn tasklists(&mut self) -> Result<Vec<TaskList>>;
    fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>>;
    fn patch(
        &mut self,
        list_id: &str,
        task_id: &str,
        patch: &TaskPatch,
        etag: Option<&str>,
    ) -> Result<Task>;
    fn create(
        &mut self,
        list_id: &str,
        task: &CreateTask,
        parent: Option<&str>,
    ) -> Result<Task>;
    fn delete(
        &mut self,
        list_id: &str,
        task_id: &str,
        etag: Option<&str>,
    ) -> Result<()>;
    fn move_task(&mut self, list_id: &str, task_id: &str, movement: &MoveTask) -> Result<Task>;
    fn create_list(&mut self, title: &str) -> Result<TaskList>;
    fn patch_list(
        &mut self,
        list_id: &str,
        patch: &ListPatch,
        etag: Option<&str>,
    ) -> Result<TaskList>;
    fn delete_list(&mut self, list_id: &str) -> Result<()>;
}

impl TasksApi for Client {
    fn account_fingerprint(&self) -> String {
        self.auth.fingerprint()
    }

    fn tasklists(&mut self) -> Result<Vec<TaskList>> {
        Client::tasklists(self)
    }

    fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>> {
        Client::tasks(self, list_id)
    }

    fn patch(
        &mut self,
        list_id: &str,
        task_id: &str,
        patch: &TaskPatch,
        etag: Option<&str>,
    ) -> Result<Task> {
        Client::patch(self, list_id, task_id, patch, etag)
    }

    fn create(
        &mut self,
        list_id: &str,
        task: &CreateTask,
        parent: Option<&str>,
    ) -> Result<Task> {
        Client::create(self, list_id, task, parent)
    }

    fn delete(
        &mut self,
        list_id: &str,
        task_id: &str,
        etag: Option<&str>,
    ) -> Result<()> {
        Client::delete(self, list_id, task_id, etag)
    }

    fn move_task(&mut self, list_id: &str, task_id: &str, movement: &MoveTask) -> Result<Task> {
        Client::move_task(self, list_id, task_id, movement)
    }

    fn create_list(&mut self, title: &str) -> Result<TaskList> {
        Client::create_list(self, title)
    }

    fn patch_list(
        &mut self,
        list_id: &str,
        patch: &ListPatch,
        etag: Option<&str>,
    ) -> Result<TaskList> {
        Client::patch_list(self, list_id, patch, etag)
    }

    fn delete_list(&mut self, list_id: &str) -> Result<()> {
        Client::delete_list(self, list_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typed_task_requests_serialize_only_requested_fields() {
        let patch = TaskPatch {
            status: Some("needsAction".into()),
            completed: Some(None),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(patch).unwrap(),
            json!({ "status": "needsAction", "completed": null })
        );

        let create = CreateTask {
            title: "New".into(),
            notes: Some("Notes".into()),
            due: None,
        };
        assert_eq!(
            serde_json::to_value(create).unwrap(),
            json!({ "title": "New", "notes": "Notes" })
        );
    }

    #[test]
    fn tasks_retain_etag_and_assignment_context() {
        let task: Task = serde_json::from_value(json!({
            "id": "task",
            "etag": "\"v2\"",
            "title": "Assigned",
            "assignmentInfo": {
                "linkToTask": "https://docs.google.com/document/d/1",
                "surfaceType": "DOCUMENT"
            }
        }))
        .unwrap();

        assert_eq!(task.etag, "\"v2\"");
        let assignment = task.assignment_info.unwrap();
        assert_eq!(assignment.surface_type, "DOCUMENT");
        assert!(assignment.link_to_task.starts_with("https://docs.google.com/"));
    }

    #[test]
    fn only_safe_transient_requests_are_retried() {
        assert!(should_retry(429, true, 0));
        assert!(should_retry(503, true, 1));
        assert!(!should_retry(503, true, 2));
        assert!(!should_retry(503, false, 0));
        assert!(!should_retry(400, true, 0));
    }
}
