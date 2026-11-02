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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveTask {
    /// `None` moves within the current list.
    pub destination_list: Option<String>,
    pub parent: Option<String>,
    pub previous: Option<String>,
}

impl MoveTask {
    /// Requests that move `ids` (a task, then its subtasks) into `destination`,
    /// preserving sibling order under the root.
    pub fn tree(destination: &str, ids: &[String]) -> Vec<(String, MoveTask)> {
        let Some(root) = ids.first() else {
            return Vec::new();
        };
        ids.iter()
            .enumerate()
            .map(|(index, id)| {
                (
                    id.clone(),
                    MoveTask {
                        destination_list: Some(destination.to_string()),
                        parent: (index > 0).then(|| root.clone()),
                        previous: (index > 1).then(|| ids[index - 1].clone()),
                    },
                )
            })
            .collect()
    }
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

    #[test]
    fn move_tree_preserves_subtask_sibling_order() {
        let ids = vec!["parent".into(), "first".into(), "second".into()];
        let requests = MoveTask::tree("destination", &ids);

        assert_eq!(requests[0].1.parent, None);
        assert_eq!(requests[0].1.previous, None);
        assert_eq!(
            requests[0].1.destination_list.as_deref(),
            Some("destination")
        );
        assert_eq!(requests[1].1.parent.as_deref(), Some("parent"));
        assert_eq!(requests[1].1.previous, None);
        assert_eq!(requests[2].1.parent.as_deref(), Some("parent"));
        assert_eq!(requests[2].1.previous.as_deref(), Some("first"));
    }
}
