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
    /// `None` moves within the current list.
    pub destination_list: Option<String>,
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
    base_url: String,
}

impl Client {
    pub fn new(auth: Auth) -> Self {
        Self {
            auth,
            base_url: BASE.into(),
        }
    }

    #[cfg(test)]
    fn for_test(auth: Auth, base_url: String) -> Self {
        Self { auth, base_url }
    }

    fn request(
        &mut self,
        safe_to_retry: bool,
        mut send: impl FnMut(
            &ureq::Agent,
            &str,
        )
            -> std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
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
                thread::sleep(
                    retry_after
                        .unwrap_or(Duration::from_millis(200))
                        .min(Duration::from_secs(2)),
                );
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
        self.get_all(&format!("{}/users/@me/lists", self.base_url), &[])
    }

    /// Every task in a list, including completed and hidden ones.
    pub fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>> {
        self.get_all(
            &format!("{}/lists/{}/tasks", self.base_url, pct(list_id)),
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
        let url = format!(
            "{}/lists/{}/tasks/{}",
            self.base_url,
            pct(list_id),
            pct(task_id)
        );
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
        let url = format!("{}/lists/{}/tasks", self.base_url, pct(list_id));
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
    pub fn delete(&mut self, list_id: &str, task_id: &str, etag: Option<&str>) -> Result<()> {
        let url = format!(
            "{}/lists/{}/tasks/{}",
            self.base_url,
            pct(list_id),
            pct(task_id)
        );
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
    pub fn move_task(&mut self, list_id: &str, task_id: &str, movement: &MoveTask) -> Result<Task> {
        let url = format!(
            "{}/lists/{}/tasks/{}/move",
            self.base_url,
            pct(list_id),
            pct(task_id)
        );
        self.request(false, |agent, token| {
            let mut request = agent
                .post(&url)
                .header("Authorization", format!("Bearer {token}"));
            if let Some(destination) = &movement.destination_list {
                request = request.query("destinationTasklist", destination);
            }
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
        let url = format!("{}/users/@me/lists", self.base_url);
        self.request(false, |agent, token| {
            agent
                .post(&url)
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
        let url = format!("{}/users/@me/lists/{}", self.base_url, pct(list_id));
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
        let url = format!("{}/users/@me/lists/{}", self.base_url, pct(list_id));
        self.request(false, |agent, token| {
            agent
                .delete(&url)
                .header("Authorization", format!("Bearer {token}"))
                .call()
        })?;
        Ok(())
    }

    /// Follow `nextPageToken` until the collection is exhausted.
    fn get_all<T: DeserializeOwned>(
        &mut self,
        url: &str,
        query: &[(&str, &str)],
    ) -> Result<Vec<T>> {
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
    fn create(&mut self, list_id: &str, task: &CreateTask, parent: Option<&str>) -> Result<Task>;
    fn delete(&mut self, list_id: &str, task_id: &str, etag: Option<&str>) -> Result<()>;
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

    fn create(&mut self, list_id: &str, task: &CreateTask, parent: Option<&str>) -> Result<Task> {
        Client::create(self, list_id, task, parent)
    }

    fn delete(&mut self, list_id: &str, task_id: &str, etag: Option<&str>) -> Result<()> {
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
    use std::{
        collections::HashMap,
        io::{Read, Write},
        net::TcpListener,
        path::PathBuf,
        sync::mpsc,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Reply {
        status: u16,
        headers: Vec<(&'static str, &'static str)>,
        body: &'static str,
    }

    struct Request {
        method: String,
        target: String,
        headers: HashMap<String, String>,
        body: String,
    }

    fn agent() -> ureq::Agent {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(2)))
            .build();
        ureq::Agent::new_with_config(config)
    }

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gtt-api-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn scripted(
        replies: Vec<Reply>,
    ) -> (String, mpsc::Receiver<Request>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (requests, captured) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut raw = Vec::new();
                let header_end = loop {
                    let mut chunk = [0; 4096];
                    let read = stream.read(&mut chunk).unwrap();
                    assert!(read > 0, "connection closed before request headers");
                    raw.extend_from_slice(&chunk[..read]);
                    if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let head = String::from_utf8(raw[..header_end].to_vec()).unwrap();
                let mut lines = head.split("\r\n");
                let mut request_line = lines.next().unwrap().split_whitespace();
                let method = request_line.next().unwrap().to_string();
                let target = request_line.next().unwrap().to_string();
                let headers: HashMap<String, String> = lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
                    .collect();
                let content_length = headers
                    .get("content-length")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                while raw.len() - header_end < content_length {
                    let mut chunk = [0; 4096];
                    let read = stream.read(&mut chunk).unwrap();
                    assert!(read > 0, "connection closed before request body");
                    raw.extend_from_slice(&chunk[..read]);
                }
                requests
                    .send(Request {
                        method,
                        target,
                        headers,
                        body: String::from_utf8(
                            raw[header_end..header_end + content_length].to_vec(),
                        )
                        .unwrap(),
                    })
                    .unwrap();

                let reason = match reply.status {
                    200 => "OK",
                    401 => "Unauthorized",
                    429 => "Too Many Requests",
                    503 => "Service Unavailable",
                    _ => "Error",
                };
                write!(
                    stream,
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reason,
                    reply.body.len()
                )
                .unwrap();
                for (key, value) in reply.headers {
                    write!(stream, "{key}: {value}\r\n").unwrap();
                }
                write!(stream, "\r\n{}", reply.body).unwrap();
            }
        });
        (base, captured, handle)
    }

    fn test_client(base: &str, agent: ureq::Agent) -> Client {
        let token_path = temp_path("token").join("token.json");
        let auth = Auth::for_test(agent, format!("{base}/token"), token_path, "old-access");
        Client::for_test(auth, base.into())
    }

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
        assert!(assignment
            .link_to_task
            .starts_with("https://docs.google.com/"));
    }

    #[test]
    fn only_safe_transient_requests_are_retried() {
        assert!(should_retry(429, true, 0));
        assert!(should_retry(503, true, 1));
        assert!(!should_retry(503, true, 2));
        assert!(!should_retry(503, false, 0));
        assert!(!should_retry(400, true, 0));
    }

    #[test]
    fn tasks_use_encoded_paths_required_queries_and_pagination() {
        let (base, requests, server) = scripted(vec![
            Reply {
                status: 200,
                headers: vec![],
                body: r#"{"items":[{"id":"one"}],"nextPageToken":"next page/?"}"#,
            },
            Reply {
                status: 200,
                headers: vec![],
                body: r#"{"items":[{"id":"two"}]}"#,
            },
        ]);
        let mut client = test_client(&base, agent());

        let tasks = client.tasks("list /é").unwrap();

        assert_eq!(
            tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        let first = requests.recv().unwrap();
        let second = requests.recv().unwrap();
        assert_eq!(first.method, "GET");
        assert!(first.target.starts_with("/lists/list%20%2F%C3%A9/tasks?"));
        let first_url = url::Url::parse(&format!("{base}{}", first.target)).unwrap();
        let first_query: HashMap<_, _> = first_url.query_pairs().into_owned().collect();
        assert_eq!(
            first_query.get("maxResults").map(String::as_str),
            Some("100")
        );
        assert_eq!(
            first_query.get("showCompleted").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            first_query.get("showHidden").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            first_query.get("showAssigned").map(String::as_str),
            Some("true")
        );
        assert!(!first_query.contains_key("pageToken"));
        let second_url = url::Url::parse(&format!("{base}{}", second.target)).unwrap();
        let second_query: HashMap<_, _> = second_url.query_pairs().into_owned().collect();
        assert_eq!(
            second_query.get("pageToken").map(String::as_str),
            Some("next page/?")
        );
        server.join().unwrap();
    }

    #[test]
    fn writes_send_typed_json_encoded_values_and_conditional_etags() {
        let (base, requests, server) = scripted(vec![
            Reply {
                status: 200,
                headers: vec![],
                body: r#"{"id":"task /é","title":"Changed"}"#,
            },
            Reply {
                status: 200,
                headers: vec![],
                body: r#"{"id":"created","title":"New"}"#,
            },
        ]);
        let mut client = test_client(&base, agent());

        client
            .patch(
                "list /é",
                "task /é",
                &TaskPatch {
                    title: Some("Changed".into()),
                    completed: Some(None),
                    ..Default::default()
                },
                Some("\"etag value\""),
            )
            .unwrap();
        client
            .create(
                "list /é",
                &CreateTask {
                    title: "New".into(),
                    notes: Some("Notes".into()),
                    due: None,
                },
                Some("parent /é"),
            )
            .unwrap();

        let patch = requests.recv().unwrap();
        assert_eq!(patch.method, "PATCH");
        assert_eq!(
            patch.target,
            "/lists/list%20%2F%C3%A9/tasks/task%20%2F%C3%A9"
        );
        assert_eq!(
            patch.headers.get("if-match").map(String::as_str),
            Some("\"etag value\"")
        );
        assert_eq!(
            serde_json::from_str::<Value>(&patch.body).unwrap(),
            json!({"title": "Changed", "completed": null})
        );
        let create = requests.recv().unwrap();
        assert_eq!(create.method, "POST");
        let create_url = url::Url::parse(&format!("{base}{}", create.target)).unwrap();
        assert_eq!(
            create_url
                .query_pairs()
                .find(|(key, _)| key == "parent")
                .map(|(_, value)| value.into_owned()),
            Some("parent /é".into())
        );
        assert_eq!(
            serde_json::from_str::<Value>(&create.body).unwrap(),
            json!({"title": "New", "notes": "Notes"})
        );
        server.join().unwrap();
    }

    #[test]
    fn same_list_move_omits_destination_tasklist() {
        let (base, requests, server) = scripted(vec![Reply {
            status: 200,
            headers: vec![],
            body: r#"{"id":"child","parent":"parent"}"#,
        }]);
        let mut client = test_client(&base, agent());

        client
            .move_task(
                "list",
                "child",
                &MoveTask {
                    destination_list: None,
                    parent: Some("parent".into()),
                    previous: None,
                },
            )
            .unwrap();

        let request = requests.recv().unwrap();
        let url = url::Url::parse(&format!("{base}{}", request.target)).unwrap();
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("parent").map(String::as_str), Some("parent"));
        assert!(!query.contains_key("destinationTasklist"));
        server.join().unwrap();
    }

    #[test]
    fn one_unauthorized_response_refreshes_the_token_and_retries_once() {
        let (base, requests, server) = scripted(vec![
            Reply {
                status: 401,
                headers: vec![],
                body: r#"{"error":{"message":"expired"}}"#,
            },
            Reply {
                status: 200,
                headers: vec![],
                body: r#"{"access_token":"new-access","expires_in":3600}"#,
            },
            Reply {
                status: 200,
                headers: vec![],
                body: r#"{"items":[]}"#,
            },
        ]);
        let mut client = test_client(&base, agent());

        assert!(client.tasklists().unwrap().is_empty());

        let first = requests.recv().unwrap();
        let refresh = requests.recv().unwrap();
        let retry = requests.recv().unwrap();
        assert_eq!(
            first.headers.get("authorization").map(String::as_str),
            Some("Bearer old-access")
        );
        assert_eq!(refresh.target, "/token");
        assert!(refresh.body.contains("grant_type=refresh_token"));
        assert_eq!(
            retry.headers.get("authorization").map(String::as_str),
            Some("Bearer new-access")
        );
        server.join().unwrap();
    }

    #[test]
    fn safe_reads_retry_rate_limits_and_server_errors_but_writes_do_not() {
        let (base, requests, server) = scripted(vec![
            Reply {
                status: 429,
                headers: vec![("Retry-After", "0")],
                body: r#"{"error":{"message":"slow down"}}"#,
            },
            Reply {
                status: 503,
                headers: vec![],
                body: r#"{"error":{"message":"unavailable"}}"#,
            },
            Reply {
                status: 200,
                headers: vec![],
                body: r#"{"items":[]}"#,
            },
        ]);
        let mut client = test_client(&base, agent());
        assert!(client.tasklists().unwrap().is_empty());
        assert_eq!(requests.try_iter().count(), 3);
        server.join().unwrap();

        let (base, requests, server) = scripted(vec![Reply {
            status: 503,
            headers: vec![],
            body: r#"{"error":{"message":"unavailable"}}"#,
        }]);
        let mut client = test_client(&base, agent());
        let error = client
            .patch("list", "task", &TaskPatch::default(), None)
            .unwrap_err();
        assert!(matches!(error, Error::Http { status: 503, .. }));
        assert_eq!(requests.try_iter().count(), 1);
        server.join().unwrap();
    }

    #[test]
    fn malformed_bodies_transport_failures_and_timeouts_are_typed() {
        let (base, _requests, server) = scripted(vec![Reply {
            status: 200,
            headers: vec![],
            body: "not-json",
        }]);
        let mut malformed = test_client(&base, agent());
        assert!(matches!(malformed.tasklists(), Err(Error::Decode(_))));
        server.join().unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let unavailable = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let mut transport = test_client(&unavailable, agent());
        assert!(matches!(transport.tasklists(), Err(Error::Transport(_))));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let slow = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_recv_response(Some(Duration::from_millis(20)))
            .timeout_global(Some(Duration::from_millis(50)))
            .build();
        let mut timeout = test_client(&slow, ureq::Agent::new_with_config(config));
        assert!(matches!(timeout.tasklists(), Err(Error::Timeout)));
        server.join().unwrap();
    }
}
