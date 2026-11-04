//! The Google Tasks v1 HTTP client.

use std::{thread, time::Duration};

use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;

use super::auth::{self, Auth};
use super::http::pct;
use super::{CreateTask, Error, ListPatch, MoveTask, Result, TaskPatch, TasksApi};
use crate::model::{Task, TaskList};

const BASE: &str = "https://tasks.googleapis.com/tasks/v1";

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
        self.auth.token().map_err(|error| match error {
            auth::Error::SignInExpired { .. } => Error::AuthExpired,
            error => Error::Transport(error.to_string()),
        })
    }
}

/// Statuses never arrive here: the agent reports them as responses, and
/// `Client::request` maps them.
fn map_ureq(error: ureq::Error) -> Error {
    match error {
        ureq::Error::Timeout(_) => Error::Timeout,
        ureq::Error::Json(error) => Error::Decode(error.to_string()),
        error => Error::Transport(error.to_string()),
    }
}

fn should_retry(status: u16, safe_to_retry: bool, attempt: usize) -> bool {
    safe_to_retry && attempt < 2 && (status == 429 || (500..600).contains(&status))
}

impl TasksApi for Client {
    fn account_fingerprint(&self) -> String {
        self.auth.fingerprint()
    }

    fn tasklists(&mut self) -> Result<Vec<TaskList>> {
        self.get_all(&format!("{}/users/@me/lists", self.base_url), &[])
    }

    fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>> {
        self.get_all(
            &format!("{}/lists/{}/tasks", self.base_url, pct(list_id)),
            &[
                ("showCompleted", "true"),
                ("showHidden", "true"),
                ("showAssigned", "true"),
            ],
        )
    }

    fn patch(
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

    fn create(&mut self, list_id: &str, task: &CreateTask, parent: Option<&str>) -> Result<Task> {
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

    fn delete(&mut self, list_id: &str, task_id: &str, etag: Option<&str>) -> Result<()> {
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

    fn clear_completed(&mut self, list_id: &str) -> Result<()> {
        let url = format!("{}/lists/{}/clear", self.base_url, pct(list_id));
        self.request(false, |agent, token| {
            agent
                .post(&url)
                .header("Authorization", format!("Bearer {token}"))
                .send_empty()
        })?;
        Ok(())
    }

    fn move_task(&mut self, list_id: &str, task_id: &str, movement: &MoveTask) -> Result<Task> {
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

    fn create_list(&mut self, title: &str) -> Result<TaskList> {
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

    fn patch_list(
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

    fn delete_list(&mut self, list_id: &str) -> Result<()> {
        let url = format!("{}/users/@me/lists/{}", self.base_url, pct(list_id));
        self.request(false, |agent, token| {
            agent
                .delete(&url)
                .header("Authorization", format!("Bearer {token}"))
                .call()
        })?;
        Ok(())
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
                    clear_completed: true,
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
    fn clear_completed_posts_to_the_list_clear_endpoint() {
        let (base, requests, server) = scripted(vec![Reply {
            status: 204,
            headers: vec![],
            body: "",
        }]);
        let mut client = test_client(&base, agent());

        client.clear_completed("list /é").unwrap();

        let request = requests.recv().unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/lists/list%20%2F%C3%A9/clear");
        assert!(request.body.is_empty());
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
