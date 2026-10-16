use crate::{
    api::{Task, TaskList},
    board,
};

pub fn normalize_title(title: &str) -> Result<String, &'static str> {
    let title = inline(title).trim().to_string();
    if title.is_empty() {
        Err("a task needs a title")
    } else {
        Ok(title)
    }
}

pub fn display_title(task: &Task) -> String {
    normalize_title(&task.title).unwrap_or_else(|_| "(untitled)".into())
}

pub fn md_lists(lists: &[TaskList]) -> String {
    lists
        .iter()
        .map(|list| {
            format!(
                "- {} ({})\n",
                markdown_text(&inline(&list.title)),
                code_span(&list.id)
            )
        })
        .collect()
}

pub fn md_task_section(list: &TaskList, tasks: &[Task]) -> String {
    let mut output = format!(
        "# {} ({})\n",
        markdown_text(&inline(&list.title)),
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
                markdown_text(&inline(&task.title)),
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
        markdown_text(&inline(&task.title)),
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
            markdown_text(&inline(label)),
            link_destination(&link.link)
        ));
    }
    output
}

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

fn inline(value: &str) -> String {
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
    let value = inline(value);
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
    let value = inline(value)
        .replace(' ', "%20")
        .replace('<', "%3C")
        .replace('>', "%3E");
    format!("<{value}>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Link;

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

    #[test]
    fn url_extraction_preserves_balanced_parentheses() {
        assert_eq!(
            urls_in("see https://example.test/a_(b) and [x](https://other.test/x)."),
            ["https://example.test/a_(b)", "https://other.test/x"]
        );
        assert!(urls_in("https:// no-host.test ftp://example.test").is_empty());
    }
}
