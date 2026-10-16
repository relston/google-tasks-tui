//! Typed request bodies. Each serializes only the fields it was given.

use chrono::NaiveDate;
use serde::Serialize;

use crate::model::{due_date, Status};

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTask {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", with = "due_date")]
    pub due: Option<NaiveDate>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", with = "due_date")]
    pub due: Option<NaiveDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    /// Sends `"completed": null`, which Google needs to forget a completion.
    #[serde(
        rename = "completed",
        skip_serializing_if = "std::ops::Not::not",
        serialize_with = "null"
    )]
    pub clear_completed: bool,
}

impl TaskPatch {
    /// Tick or untick a task. Unticking also clears the completion time.
    pub fn status(status: Status) -> Self {
        Self {
            status: Some(status),
            clear_completed: status == Status::NeedsAction,
            ..Self::default()
        }
    }
}

fn null<S: serde::Serializer>(_: &bool, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_none()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typed_task_requests_serialize_only_requested_fields() {
        assert_eq!(
            serde_json::to_value(TaskPatch::status(Status::NeedsAction)).unwrap(),
            json!({ "status": "needsAction", "completed": null })
        );
        assert_eq!(
            serde_json::to_value(TaskPatch::status(Status::Completed)).unwrap(),
            json!({ "status": "completed" })
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
        let dated = CreateTask {
            title: "Dated".into(),
            notes: None,
            due: NaiveDate::from_ymd_opt(2026, 9, 21),
        };
        assert_eq!(
            serde_json::to_value(dated).unwrap(),
            json!({ "title": "Dated", "due": "2026-09-21T00:00:00.000Z" })
        );
    }
}
