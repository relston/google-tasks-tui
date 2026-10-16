//! Google Tasks data as the app stores and shows it, and the text rules
//! every interface shares.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TaskList {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub etag: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    #[default]
    NeedsAction,
    Completed,
}

impl Status {
    /// Google's spelling, shared by the CLI and Markdown output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeedsAction => "needsAction",
            Self::Completed => "completed",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Task {
    pub id: String,
    pub etag: String,
    pub title: String,
    pub notes: Option<String>,
    pub status: Status,
    #[serde(with = "due_date")]
    pub due: Option<NaiveDate>,
    #[serde(with = "timestamp")]
    pub completed: Option<DateTime<Utc>>,
    #[serde(with = "timestamp")]
    pub updated: Option<DateTime<Utc>>,
    pub parent: Option<String>,
    pub position: String,
    pub hidden: bool,
    pub deleted: bool,
    pub web_view_link: Option<String>,
    pub links: Vec<Link>,
    pub assignment_info: Option<AssignmentInfo>,
}

impl Task {
    pub fn is_done(&self) -> bool {
        self.status == Status::Completed
    }
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

/// Google sends due dates as midnight UTC timestamps; only the date matters.
pub(crate) mod due_date {
    use chrono::NaiveDate;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        date: &Option<NaiveDate>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match date {
            Some(date) => serializer.serialize_some(&format!("{date}T00:00:00.000Z")),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<NaiveDate>, D::Error> {
        let raw = Option::<String>::deserialize(deserializer)?;
        Ok(raw.and_then(|raw| NaiveDate::parse_from_str(raw.get(..10)?, "%Y-%m-%d").ok()))
    }
}

/// RFC 3339 timestamps in Google's millisecond form. An unreadable value is
/// treated as missing rather than failing the whole list.
mod timestamp {
    use chrono::{DateTime, SecondsFormat, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        time: &Option<DateTime<Utc>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match time {
            Some(time) => {
                serializer.serialize_some(&time.to_rfc3339_opts(SecondsFormat::Millis, true))
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<DateTime<Utc>>, D::Error> {
        let raw = Option::<String>::deserialize(deserializer)?;
        Ok(raw
            .and_then(|raw| DateTime::parse_from_rfc3339(&raw).ok())
            .map(|time| time.with_timezone(&Utc)))
    }
}

pub fn normalize_title(title: &str) -> Result<String, &'static str> {
    let title = single_line(title).trim().to_string();
    if title.is_empty() {
        Err("a task needs a title")
    } else {
        Ok(title)
    }
}

pub fn display_title(task: &Task) -> String {
    normalize_title(&task.title).unwrap_or_else(|_| "(untitled)".into())
}

/// Control characters and line breaks become spaces.
pub(crate) fn single_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() || character == '\n' {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// HTTP(S) URLs typed into notes, in first-seen order without duplicates.
pub fn urls_in(notes: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < notes.len() {
        let rest = &notes[index..];
        let scheme = if rest.starts_with("https://") {
            "https://"
        } else if rest.starts_with("http://") {
            "http://"
        } else {
            index += rest.chars().next().map(char::len_utf8).unwrap_or(1);
            continue;
        };
        let end = rest
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '<' | '>' | '"' | '\'')
            })
            .unwrap_or(rest.len());
        let candidate = trim_url_punctuation(&rest[..end]);
        if candidate.len() > scheme.len()
            && url::Url::parse(candidate).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            })
            && !output.iter().any(|url| url == candidate)
        {
            output.push(candidate.to_string());
        }
        index += end.max(scheme.len());
    }
    output
}

fn trim_url_punctuation(mut value: &str) -> &str {
    value = value.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    while value.ends_with(')') && value.matches(')').count() > value.matches('(').count() {
        value = &value[..value.len() - 1];
    }
    while value.ends_with(']') && value.matches(']').count() > value.matches('[').count() {
        value = &value[..value.len() - 1];
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn task_status_and_dates_round_trip_google_formats() {
        let raw = json!({
            "id": "t",
            "status": "completed",
            "due": "2026-09-25T00:00:00.000Z",
            "completed": "2026-09-20T15:04:05.000Z",
            "updated": "2026-09-21T08:00:00.000Z"
        });
        let task: Task = serde_json::from_value(raw).unwrap();
        assert_eq!(task.status, Status::Completed);
        assert!(task.is_done());
        assert_eq!(task.due, NaiveDate::from_ymd_opt(2026, 9, 25));
        assert_eq!(
            task.completed,
            Some(Utc.with_ymd_and_hms(2026, 9, 20, 15, 4, 5).unwrap())
        );

        let written = serde_json::to_value(&task).unwrap();
        assert_eq!(written["status"], "completed");
        assert_eq!(written["due"], "2026-09-25T00:00:00.000Z");
        assert_eq!(written["completed"], "2026-09-20T15:04:05.000Z");
        assert_eq!(written["updated"], "2026-09-21T08:00:00.000Z");

        let open: Task =
            serde_json::from_value(json!({ "id": "o", "status": "needsAction", "due": "soon" }))
                .unwrap();
        assert_eq!(open.status, Status::NeedsAction);
        assert!(!open.is_done());
        assert_eq!(open.due, None);
        assert_eq!(Status::default(), Status::NeedsAction);
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
    fn titles_are_single_line_and_never_blank() {
        assert_eq!(normalize_title("  a\nb\tc  ").unwrap(), "a b c");
        assert!(normalize_title(" \n\t ").is_err());
        assert_eq!(display_title(&Task::default()), "(untitled)");
    }

    #[test]
    fn urls_in_finds_http_and_https_and_skips_the_rest() {
        assert_eq!(urls_in(""), Vec::<String>::new());
        assert_eq!(urls_in("no links here"), Vec::<String>::new());
        assert_eq!(
            urls_in("see https://example.com/a"),
            vec!["https://example.com/a"]
        );
        assert_eq!(
            urls_in("http://a.test https://b.test/x"),
            vec!["http://a.test", "https://b.test/x"]
        );
        assert_eq!(
            urls_in("read [PR](https://github.com/org/repo/pull/1)."),
            vec!["https://github.com/org/repo/pull/1"]
        );
        assert_eq!(urls_in("visit https://ex.com."), vec!["https://ex.com"]);
        assert_eq!(urls_in("www.example.com"), Vec::<String>::new());
        assert_eq!(
            urls_in("https://dup.test then https://dup.test again"),
            vec!["https://dup.test"]
        );
    }

    #[test]
    fn url_extraction_preserves_balanced_parentheses() {
        assert_eq!(
            urls_in("see https://example.test/a_(b) and [x](https://other.test/x)."),
            ["https://example.test/a_(b)", "https://other.test/x"]
        );
        assert!(urls_in("https:// no-host.test ftp://example.test").is_empty());
    }
}
