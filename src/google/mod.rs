//! The Google Tasks boundary: OAuth, the HTTP client, and request bodies.
//! Both interfaces talk to Google only through `TasksApi`.

pub mod auth;
mod client;
mod http;
mod requests;

use std::{fmt, time::Duration};

use crate::model::{Task, TaskList};

pub use client::Client;
pub use requests::{CreateTask, ListPatch, MoveTask, TaskPatch};

#[derive(Debug)]
pub enum Error {
    AuthExpired,
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

/// The Google Tasks operations the app uses. `Client` is the real
/// implementation; tests use fakes.
pub trait TasksApi {
    fn account_fingerprint(&self) -> String;
    fn tasklists(&mut self) -> Result<Vec<TaskList>>;
    /// Every task in a list, including completed and hidden ones.
    fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>>;
    /// Patch the given fields of a task; returns it as Google stored it.
    fn patch(
        &mut self,
        list_id: &str,
        task_id: &str,
        patch: &TaskPatch,
        etag: Option<&str>,
    ) -> Result<Task>;
    /// Add a task to the top of a list, under `parent` when given.
    fn create(&mut self, list_id: &str, task: &CreateTask, parent: Option<&str>) -> Result<Task>;
    /// Delete a task. Google deletes its subtasks along with it.
    fn delete(&mut self, list_id: &str, task_id: &str, etag: Option<&str>) -> Result<()>;
    /// Move a task within its list or into another one.
    fn move_task(&mut self, list_id: &str, task_id: &str, movement: &MoveTask) -> Result<Task>;
    fn create_list(&mut self, title: &str) -> Result<TaskList>;
    fn patch_list(
        &mut self,
        list_id: &str,
        patch: &ListPatch,
        etag: Option<&str>,
    ) -> Result<TaskList>;
    /// Delete a task list. Google deletes every task in it.
    fn delete_list(&mut self, list_id: &str) -> Result<()>;
}
