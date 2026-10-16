//! Markdown for CLI output and the TUI's copy command. Terminal controls are
//! stripped and Markdown structure is escaped by context.

use crate::{
    board,
    model::{display_title, single_line, Task, TaskList},
};

pub fn md_lists(lists: &[TaskList]) -> String {
    lists
        .iter()
        .map(|list| {
            format!(
                "- {} ({})\n",
                markdown_text(&single_line(&list.title)),
                code_span(&list.id)
            )
        })
        .collect()
}

pub fn md_task_section(list: &TaskList, tasks: &[Task]) -> String {
    let mut output = format!(
        "# {} ({})\n",
        markdown_text(&single_line(&list.title)),
        code_span(&list.id)
    );
    let rows = board::ordered(tasks.to_vec());
    if !rows.is_empty() {
        output.push('\n');
        for row in rows {
            let task = row.task;
            let mark = if task.is_done() { "x" } else { " " };
            let indent = if row.depth > 0 { "  " } else { "" };
            output.push_str(&format!(
                "{indent}- [{mark}] {} ({})\n",
                markdown_text(&single_line(&task.title)),
                code_span(&task.id)
            ));
            if let Some(notes) = task
                .notes
                .as_deref()
                .map(multiline)
                .map(|notes| notes.trim().to_string())
                .filter(|notes| !notes.is_empty())
            {
                for line in notes.lines() {
                    output.push_str(&format!("{indent}  {line}\n"));
                }
            }
        }
    }
    output
}

pub fn md_task(task: &Task) -> String {
    let mut output = format!(
        "# {}\n\n- id: {}\n- status: {}",
        markdown_text(&single_line(&task.title)),
        code_span(&task.id),
        task.status.as_str()
    );
    if let Some(due) = task.due {
        output.push_str(&format!("\n- due: {due}"));
    }
    if let Some(parent) = &task.parent {
        output.push_str(&format!("\n- parent: {}", code_span(parent)));
    }
    if let Some(notes) = task
        .notes
        .as_deref()
        .map(multiline)
        .map(|notes| notes.trim().to_string())
        .filter(|notes| !notes.is_empty())
    {
        output.push_str("\n\n");
        output.push_str(&notes);
    }
    output.push('\n');
    output
}

pub fn md_yank(task: &Task) -> String {
    let mut output = format!("# {}\n", markdown_text(&display_title(task)));
    if let Some(due) = task.due {
        output.push_str(&format!("\nDue: {due}\n"));
    }
    if let Some(notes) = task
        .notes
        .as_deref()
        .map(multiline)
        .map(|notes| notes.trim().to_string())
        .filter(|notes| !notes.is_empty())
    {
        output.push_str(&format!("\n{notes}\n"));
    }
    let mut any = false;
    for link in task.links.iter().filter(|link| !link.link.is_empty()) {
        if !any {
            output.push('\n');
            any = true;
        }
        let label = if link.description.trim().is_empty() {
            link.link.as_str()
        } else {
            link.description.trim()
        };
        output.push_str(&format!(
            "- [{}]({})\n",
            markdown_text(&single_line(label)),
            link_destination(&link.link)
        ));
    }
    output
}

fn multiline(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() && character != '\n' {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn markdown_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '+' | '-' | '!' | '|'
        ) {
            output.push('\\');
        }
        output.push(character);
    }
    output
}

fn code_span(value: &str) -> String {
    let value = single_line(value);
    let longest = value
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    if value.starts_with('`') || value.ends_with('`') {
        format!("{fence} {value} {fence}")
    } else {
        format!("{fence}{value}{fence}")
    }
}

fn link_destination(value: &str) -> String {
    let value = single_line(value)
        .replace(' ', "%20")
        .replace('<', "%3C")
        .replace('>', "%3E");
    format!("<{value}>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Link;
    use chrono::NaiveDate;

    #[test]
    fn markdown_escapes_structure_and_terminal_controls() {
        let task = Task {
            id: "id`tick".into(),
            title: "# heading\r\u{1b}[31m".into(),
            notes: Some("line\r\nnext".into()),
            links: vec![Link {
                description: "[source]".into(),
                link: "https://example.test/a_(b)".into(),
            }],
            ..Default::default()
        };
        let markdown = md_yank(&task);
        assert!(markdown.starts_with("# \\# heading  \\[31m"));
        assert!(markdown.contains("- [\\[source\\]](<https://example.test/a_(b)>)"));
        assert!(!markdown.contains('\r'));
        assert!(!markdown.contains('\u{1b}'));
        assert!(md_task(&task).contains("``id`tick``"));
    }

    fn task(id: &str, title: &str) -> Task {
        Task {
            id: id.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn md_yank_is_the_paste_shape() {
        // Nothing but a title, even when Google sends a link to the task.
        let bare = Task {
            web_view_link: Some("https://tasks.google.com/t/9".into()),
            ..task("t1", "Buy milk")
        };
        assert_eq!(md_yank(&bare), "# Buy milk\n");
        assert_eq!(md_yank(&Task::default()), "# (untitled)\n");

        // A description-less link labels itself with its URL, and a title with
        // brackets cannot close the link early.
        let t = Task {
            links: vec![
                Link {
                    description: "  ".into(),
                    link: "https://bare.test/x".into(),
                },
                Link {
                    description: "a [b] c".into(),
                    link: "https://named.test".into(),
                },
                Link {
                    description: "dropped".into(),
                    link: String::new(),
                },
            ],
            // A link back to the task itself is not wanted.
            web_view_link: Some("https://tasks.google.com/t/1".into()),
            ..task("t2", "Linked")
        };
        assert_eq!(
            md_yank(&t),
            "# Linked\n\n\
             - [https://bare.test/x](<https://bare.test/x>)\n\
             - [a \\[b\\] c](<https://named.test>)\n"
        );

        // Due and notes, in that order, each its own block.
        let t = Task {
            due: NaiveDate::from_ymd_opt(2026, 9, 25),
            notes: Some("  line one\nline two  ".into()),
            ..task("t3", "Both")
        };
        assert_eq!(
            md_yank(&t),
            "# Both\n\nDue: 2026-09-25\n\nline one\nline two\n"
        );
    }
}
