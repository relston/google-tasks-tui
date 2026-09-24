//! One-shot CLI: markdown on stdout by default, diagnostics on stderr.

use std::io::{self, Write};

use clap::{Parser, Subcommand, ValueEnum};

use crate::api::{CreateTask, Error as ApiError, ListPatch, Task, TaskList, TaskPatch, TasksApi};
use crate::board::Board;
use crate::cache;
use crate::paths::Paths;
use crate::presentation::{md_lists, md_task, md_task_section, normalize_title};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// GitHub-flavoured markdown (the default).
    #[default]
    Md,
    Json,
}

#[derive(Parser, Debug)]
#[command(
    name = "google-tasks-tui",
    about = "Google Tasks TUI. Pass a subcommand to run a one-shot CLI instead."
)]
pub struct Args {
    /// Output format. `md` is the default; `json` is for scripts.
    #[arg(short = 'f', long, value_enum, default_value_t = Format::Md, global = true)]
    pub format: Format,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Print every task list.
    Lists,
    /// Print tasks in a list, or the whole board.
    Tasks {
        #[arg(short, long)]
        list: Option<String>,
    },
    /// Print one task.
    Get {
        task_id: String,
        #[arg(short, long)]
        list: Option<String>,
    },
    /// Create a task in a list.
    Create {
        #[arg(short, long)]
        list: String,
        #[arg(short, long)]
        title: String,
        #[arg(long)]
        notes: Option<String>,
        /// Due date as YYYY-MM-DD.
        #[arg(long)]
        due: Option<String>,
        #[arg(long)]
        parent: Option<String>,
    },
    /// Patch fields of a task. Only flags you pass are sent.
    Update {
        task_id: String,
        #[arg(short, long)]
        list: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        due: Option<String>,
        #[arg(long, value_parser = ["needsAction", "completed"])]
        status: Option<String>,
    },
    /// Mark a task completed.
    Done {
        task_id: String,
        #[arg(short, long)]
        list: Option<String>,
    },
    /// Mark a task not completed.
    Undone {
        task_id: String,
        #[arg(short, long)]
        list: Option<String>,
    },
    /// Delete a task (and its subtasks). Requires --yes.
    Delete {
        task_id: String,
        #[arg(short, long)]
        list: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Create, rename, or delete a task list.
    List {
        #[command(subcommand)]
        cmd: ListCmd,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum ListCmd {
    /// Create a task list.
    Create {
        #[arg(short, long)]
        title: String,
    },
    /// Rename a task list.
    Update {
        /// List id or unique title.
        list: String,
        #[arg(short, long)]
        title: String,
    },
    /// Delete a task list (and every task in it). Requires --yes.
    Delete {
        /// List id or unique title.
        list: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Usage(String),
    Fail(String),
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) => 2,
            Error::Fail(_) => 1,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Usage(m) | Error::Fail(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

pub fn execute_to(
    api: &mut impl TasksApi,
    command: Command,
    format: Format,
    out: &mut impl Write,
) -> Result<(), Error> {
    execute_to_with_paths(api, command, format, out, &Paths::from_env())
}

fn execute_to_with_paths(
    api: &mut impl TasksApi,
    command: Command,
    format: Format,
    out: &mut impl Write,
    paths: &Paths,
) -> Result<(), Error> {
    match command {
        Command::Lists => {
            let lists = tasklists(api)?;
            match format {
                Format::Json => write_json(out, &lists)?,
                Format::Md => write_str(out, &md_lists(&lists))?,
            }
        }
        Command::Tasks { list } => {
            if let Some(spec) = list {
                let lists = tasklists(api)?;
                let list = resolve_list(&lists, &spec)?;
                let tasks = tasks(api, &list.id)?;
                match format {
                    Format::Json => write_json(out, &tasks)?,
                    Format::Md => write_str(out, &md_task_section(list, &tasks))?,
                }
            } else {
                let board = load_board(api)?;
                match format {
                    Format::Json => {
                        let lists = board
                            .into_iter()
                            .map(|(list, tasks)| cache::CachedList { list, tasks })
                            .collect();
                        let board = Board::from_cache(cache::Cache { lists });
                        write_json(out, &board.snapshot())?;
                    }
                    Format::Md => {
                        let md = board
                            .iter()
                            .map(|(list, tasks)| md_task_section(list, tasks))
                            .collect::<Vec<_>>()
                            .join("\n");
                        write_str(out, &md)?;
                    }
                }
            }
        }
        Command::Get { task_id, list } => {
            let (_, task) = find_task(api, &task_id, list.as_deref())?;
            write_task(out, format, &task)?;
        }
        Command::Create {
            list,
            title,
            notes,
            due,
            parent,
        } => {
            let title = normalize_title(&title).map_err(|error| Error::Usage(error.into()))?;
            let lists = tasklists(api)?;
            let list = resolve_list(&lists, &list)?;
            if let Some(parent_id) = &parent {
                if tasks(api, &list.id)?
                    .into_iter()
                    .find(|task| task.id == *parent_id)
                    .is_some_and(|task| task.assignment_info.is_some())
                {
                    return Err(Error::Usage("assigned tasks cannot have subtasks".into()));
                }
            }
            let task = CreateTask {
                title,
                notes,
                due: due.map(|due| due_rfc3339(&due)).transpose()?,
            };
            let task = api
                .create(&list.id, &task, parent.as_deref())
                .map_err(api_fail)?;
            refresh_cache(api, paths, &list.id)?;
            write_task(out, format, &task)?;
        }
        Command::Update {
            task_id,
            list,
            title,
            notes,
            due,
            status,
        } => {
            let title = title
                .map(|title| normalize_title(&title))
                .transpose()
                .map_err(|error| Error::Usage(error.into()))?;
            if title.is_none() && notes.is_none() && due.is_none() && status.is_none() {
                return Err(Error::Usage(
                    "update needs at least one of --title, --notes, --due, --status".into(),
                ));
            }
            let (list_id, task) = find_task(api, &task_id, list.as_deref())?;
            if task.assignment_info.is_some() && notes.is_some() {
                return Err(Error::Usage(
                    "tasks assigned from Docs or Chat cannot have notes".into(),
                ));
            }
            let patch = TaskPatch {
                title,
                notes,
                due: due.map(|due| due_rfc3339(&due)).transpose()?,
                status,
                completed: None,
            };
            let task = api
                .patch(&list_id, &task.id, &patch, Some(&task.etag))
                .map_err(api_fail)?;
            refresh_cache(api, paths, &list_id)?;
            write_task(out, format, &task)?;
        }
        Command::Done { task_id, list } => {
            patch_status(
                api,
                out,
                format,
                paths,
                &task_id,
                list.as_deref(),
                "completed",
            )?;
        }
        Command::Undone { task_id, list } => {
            patch_status(
                api,
                out,
                format,
                paths,
                &task_id,
                list.as_deref(),
                "needsAction",
            )?;
        }
        Command::Delete { task_id, list, yes } => {
            if !yes {
                return Err(Error::Usage(
                    "delete requires --yes (Google also removes subtasks)".into(),
                ));
            }
            let (list_id, task) = find_task(api, &task_id, list.as_deref())?;
            api.delete(&list_id, &task.id, Some(&task.etag))
                .map_err(api_fail)?;
            refresh_cache(api, paths, &list_id)?;
        }
        Command::List { cmd } => match cmd {
            ListCmd::Create { title } => {
                let title = normalize_title(&title)
                    .map_err(|_| Error::Usage("a list needs a title".into()))?;
                let list = api.create_list(&title).map_err(api_fail)?;
                cache_upsert_list(api, paths, list.clone())?;
                write_list(out, format, &list)?;
            }
            ListCmd::Update { list, title } => {
                let title = normalize_title(&title)
                    .map_err(|_| Error::Usage("a list needs a title".into()))?;
                let lists = tasklists(api)?;
                let list = resolve_list(&lists, &list)?;
                let list = api
                    .patch_list(&list.id, &ListPatch { title }, Some(&list.etag))
                    .map_err(api_fail)?;
                cache_upsert_list(api, paths, list.clone())?;
                write_list(out, format, &list)?;
            }
            ListCmd::Delete { list, yes } => {
                let lists = tasklists(api)?;
                let list = resolve_list(&lists, &list)?;
                if !yes {
                    return Err(Error::Usage(format!(
                        "list delete requires --yes (Google also removes every task in \"{}\")",
                        list.title
                    )));
                }
                let id = list.id.clone();
                api.delete_list(&id).map_err(api_fail)?;
                cache_remove_list(api, paths, &id)?;
            }
        },
    }
    Ok(())
}

fn patch_status(
    api: &mut impl TasksApi,
    out: &mut impl Write,
    format: Format,
    paths: &Paths,
    task_id: &str,
    list: Option<&str>,
    status: &str,
) -> Result<(), Error> {
    let (list_id, task) = find_task(api, task_id, list)?;
    let patch = TaskPatch {
        status: Some(status.into()),
        completed: (status == "needsAction").then_some(None),
        ..Default::default()
    };
    let task = api
        .patch(&list_id, &task.id, &patch, Some(&task.etag))
        .map_err(api_fail)?;
    refresh_cache(api, paths, &list_id)?;
    write_task(out, format, &task)
}

fn tasklists(api: &mut impl TasksApi) -> Result<Vec<TaskList>, Error> {
    api.tasklists().map_err(api_fail)
}

fn tasks(api: &mut impl TasksApi, list_id: &str) -> Result<Vec<Task>, Error> {
    api.tasks(list_id).map_err(api_fail)
}

fn load_board(api: &mut impl TasksApi) -> Result<Vec<(TaskList, Vec<Task>)>, Error> {
    let lists = tasklists(api)?;
    let mut board = Vec::with_capacity(lists.len());
    for list in lists {
        let tasks = tasks(api, &list.id)?;
        board.push((list, tasks));
    }
    Ok(board)
}

fn find_task(
    api: &mut impl TasksApi,
    task_id: &str,
    list_spec: Option<&str>,
) -> Result<(String, Task), Error> {
    let lists = tasklists(api)?;
    if let Some(spec) = list_spec {
        let list = resolve_list(&lists, spec)?;
        let tasks = tasks(api, &list.id)?;
        let task = tasks
            .into_iter()
            .find(|t| t.id == task_id)
            .ok_or_else(|| Error::Usage(format!("no task {task_id:?} in list {spec:?}")))?;
        return Ok((list.id.clone(), task));
    }
    let mut hits = Vec::new();
    for list in &lists {
        if let Some(task) = tasks(api, &list.id)?.into_iter().find(|t| t.id == task_id) {
            hits.push((list.id.clone(), task));
        }
    }
    match hits.len() {
        0 => Err(Error::Usage(format!("no task {task_id:?}"))),
        1 => Ok(hits.pop().unwrap()),
        _ => Err(Error::Usage(format!(
            "task {task_id:?} appears in more than one list; pass --list"
        ))),
    }
}

pub fn resolve_list<'a>(lists: &'a [TaskList], spec: &str) -> Result<&'a TaskList, Error> {
    if let Some(list) = lists.iter().find(|l| l.id == spec) {
        return Ok(list);
    }
    let hits: Vec<_> = lists.iter().filter(|l| l.title == spec).collect();
    match hits.as_slice() {
        [one] => Ok(one),
        [] => Err(Error::Usage(format!("no list matches {spec:?}"))),
        _ => Err(Error::Usage(format!("list title {spec:?} is not unique"))),
    }
}

fn due_rfc3339(d: &str) -> Result<String, Error> {
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .map(|d| format!("{d}T00:00:00.000Z"))
        .map_err(|_| Error::Usage(format!("due date must be YYYY-MM-DD, not {d:?}")))
}

fn cache_state(api: &impl TasksApi, paths: &Paths) -> Result<(Board, cache::Meta), Error> {
    let account = api.account_fingerprint();
    let (cache, meta) = cache::load_for(paths, &account)
        .map_err(|error| Error::Fail(format!("reading cache: {error}")))?;
    Ok((Board::from_cache(cache), meta))
}

fn save_board(paths: &Paths, board: &Board, meta: &mut cache::Meta) -> Result<(), Error> {
    cache::save_for(paths, &board.snapshot(), meta).map_err(|error| {
        Error::Fail(format!(
            "Google mutation succeeded, but local cache update failed: {error}"
        ))
    })
}

fn refresh_cache(api: &mut impl TasksApi, paths: &Paths, list_id: &str) -> Result<(), Error> {
    let tasks = api.tasks(list_id).map_err(api_fail)?;
    let (mut board, mut meta) = cache_state(api, paths)?;
    if board.list(list_id).is_none() {
        if let Ok(lists) = api.tasklists() {
            if let Some(list) = lists.into_iter().find(|l| l.id == list_id) {
                board.upsert_list(list);
            }
        }
    }
    board.replace_tasks(list_id, tasks);
    save_board(paths, &board, &mut meta)
}

fn cache_upsert_list(api: &impl TasksApi, paths: &Paths, list: TaskList) -> Result<(), Error> {
    let (mut board, mut meta) = cache_state(api, paths)?;
    board.upsert_list(list);
    save_board(paths, &board, &mut meta)
}

fn cache_remove_list(api: &impl TasksApi, paths: &Paths, list_id: &str) -> Result<(), Error> {
    let (mut board, mut meta) = cache_state(api, paths)?;
    board.remove_list(list_id);
    save_board(paths, &board, &mut meta)
}

fn write_json(out: &mut impl Write, v: &impl serde::Serialize) -> Result<(), Error> {
    writeln!(
        out,
        "{}",
        serde_json::to_string(v).map_err(|e| Error::Fail(e.to_string()))?
    )
    .map_err(io_fail)
}

fn write_str(out: &mut impl Write, s: &str) -> Result<(), Error> {
    write!(out, "{s}").map_err(io_fail)?;
    if !s.ends_with('\n') {
        writeln!(out).map_err(io_fail)?;
    }
    Ok(())
}

fn write_task(out: &mut impl Write, format: Format, task: &Task) -> Result<(), Error> {
    match format {
        Format::Json => write_json(out, task),
        Format::Md => write_str(out, &md_task(task)),
    }
}

fn write_list(out: &mut impl Write, format: Format, list: &TaskList) -> Result<(), Error> {
    match format {
        Format::Json => write_json(out, list),
        Format::Md => write_str(out, &md_lists(std::slice::from_ref(list))),
    }
}

fn api_fail(e: ApiError) -> Error {
    Error::Fail(e.to_string())
}

fn io_fail(e: io::Error) -> Error {
    Error::Fail(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Link;
    use crate::presentation::md_yank;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fake {
        lists: Vec<TaskList>,
        tasks: HashMap<String, Vec<Task>>,
        fail: Option<String>,
        creates: usize,
        patches: Vec<Value>,
        deletes: Vec<(String, String)>,
        task_fetches: usize,
        next_id: u32,
        last_cache: Option<cache::Cache>,
    }

    impl Fake {
        fn new() -> Self {
            let inbox = TaskList {
                id: "l1".into(),
                title: "Inbox".into(),
                ..Default::default()
            };
            let other = TaskList {
                id: "l2".into(),
                title: "Inbox".into(),
                ..Default::default()
            };
            let work = TaskList {
                id: "lw".into(),
                title: "Work".into(),
                ..Default::default()
            };
            let t1 = task("t1", "Buy milk", "needsAction");
            let t2 = Task {
                notes: Some("Phase 3\nsee PR".into()),
                ..task("t2", "Ship CLI", "completed")
            };
            Self {
                lists: vec![inbox.clone(), other, work],
                tasks: HashMap::from([
                    ("l1".into(), vec![t1]),
                    ("l2".into(), vec![task("t1", "dup", "needsAction")]),
                    ("lw".into(), vec![t2]),
                ]),
                fail: None,
                creates: 0,
                patches: Vec::new(),
                deletes: Vec::new(),
                task_fetches: 0,
                next_id: 10,
                last_cache: None,
            }
        }
    }

    impl TasksApi for Fake {
        fn account_fingerprint(&self) -> String {
            "test-account".into()
        }

        fn tasklists(&mut self) -> crate::api::Result<Vec<TaskList>> {
            if let Some(e) = &self.fail {
                return Err(ApiError::Transport(e.clone()));
            }
            Ok(self.lists.clone())
        }

        fn tasks(&mut self, list_id: &str) -> crate::api::Result<Vec<Task>> {
            self.task_fetches += 1;
            if let Some(e) = &self.fail {
                return Err(ApiError::Transport(e.clone()));
            }
            Ok(self.tasks.get(list_id).cloned().unwrap_or_default())
        }

        fn patch(
            &mut self,
            list_id: &str,
            task_id: &str,
            patch: &TaskPatch,
            _etag: Option<&str>,
        ) -> crate::api::Result<Task> {
            self.patches.push(serde_json::to_value(patch).unwrap());
            let tasks = self
                .tasks
                .get_mut(list_id)
                .ok_or_else(|| ApiError::Transport("no list".into()))?;
            let task = tasks
                .iter_mut()
                .find(|task| task.id == task_id)
                .ok_or_else(|| ApiError::Transport("no task".into()))?;
            if let Some(title) = &patch.title {
                task.title = title.clone();
            }
            if let Some(notes) = &patch.notes {
                task.notes = Some(notes.clone());
            }
            if let Some(due) = &patch.due {
                task.due = Some(due.clone());
            }
            if let Some(status) = &patch.status {
                task.status = status.clone();
            }
            if let Some(completed) = &patch.completed {
                task.completed = completed.clone();
            }
            Ok(task.clone())
        }

        fn create(
            &mut self,
            list_id: &str,
            create: &CreateTask,
            parent: Option<&str>,
        ) -> crate::api::Result<Task> {
            self.creates += 1;
            let t = Task {
                id: format!("n{}", self.next_id),
                title: create.title.clone(),
                notes: create.notes.clone(),
                due: create.due.clone(),
                parent: parent.map(str::to_string),
                status: "needsAction".into(),
                ..Default::default()
            };
            self.next_id += 1;
            self.tasks
                .entry(list_id.into())
                .or_default()
                .push(t.clone());
            Ok(t)
        }

        fn delete(
            &mut self,
            list_id: &str,
            task_id: &str,
            _etag: Option<&str>,
        ) -> crate::api::Result<()> {
            self.deletes.push((list_id.into(), task_id.into()));
            if let Some(tasks) = self.tasks.get_mut(list_id) {
                tasks.retain(|t| t.id != task_id);
            }
            Ok(())
        }

        fn move_task(
            &mut self,
            list_id: &str,
            task_id: &str,
            movement: &crate::api::MoveTask,
        ) -> crate::api::Result<Task> {
            let mut task = {
                let source = self
                    .tasks
                    .get_mut(list_id)
                    .ok_or_else(|| ApiError::Transport("no list".into()))?;
                let index = source
                    .iter()
                    .position(|task| task.id == task_id)
                    .ok_or_else(|| ApiError::Transport("no task".into()))?;
                source.remove(index)
            };
            task.parent = movement.parent.clone();
            let destination = movement.destination_list.as_deref().unwrap_or(list_id);
            self.tasks
                .entry(destination.to_string())
                .or_default()
                .push(task.clone());
            Ok(task)
        }

        fn create_list(&mut self, title: &str) -> crate::api::Result<TaskList> {
            let list = TaskList {
                id: format!("n{}", self.next_id),
                title: title.into(),
                ..Default::default()
            };
            self.next_id += 1;
            self.lists.push(list.clone());
            self.tasks.insert(list.id.clone(), Vec::new());
            Ok(list)
        }

        fn patch_list(
            &mut self,
            list_id: &str,
            patch: &ListPatch,
            _etag: Option<&str>,
        ) -> crate::api::Result<TaskList> {
            let list = self
                .lists
                .iter_mut()
                .find(|list| list.id == list_id)
                .ok_or_else(|| ApiError::Transport("no list".into()))?;
            list.title = patch.title.clone();
            Ok(list.clone())
        }

        fn delete_list(&mut self, list_id: &str) -> crate::api::Result<()> {
            self.lists.retain(|l| l.id != list_id);
            self.tasks.remove(list_id);
            Ok(())
        }
    }

    fn task(id: &str, title: &str, status: &str) -> Task {
        Task {
            id: id.into(),
            title: title.into(),
            status: status.into(),
            ..Default::default()
        }
    }

    fn parse(args: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("google-tasks-tui").chain(args.iter().copied()))
            .unwrap()
    }

    fn run(api: &mut Fake, args: &[&str]) -> (Result<(), Error>, String) {
        let root = std::env::temp_dir().join(format!(
            "gtt-cli-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let paths = Paths {
            credentials: root.join("credentials.json"),
            config_dir: root.clone(),
        };
        let parsed = parse(args);
        let mut out = Vec::new();
        let res = execute_to_with_paths(
            api,
            parsed.command.expect("subcommand"),
            parsed.format,
            &mut out,
            &paths,
        );
        api.last_cache = Some(
            cache::load_for(&paths, &api.account_fingerprint())
                .unwrap()
                .0,
        );
        let _ = fs::remove_dir_all(&root);
        (res, String::from_utf8(out).unwrap())
    }

    #[test]
    fn no_subcommand_means_tui() {
        let a = Args::try_parse_from(["google-tasks-tui"]).unwrap();
        assert!(a.command.is_none());
    }

    #[test]
    fn resolve_list_prefers_id_then_unique_title() {
        let lists = [
            TaskList {
                id: "l1".into(),
                title: "Inbox".into(),
                ..Default::default()
            },
            TaskList {
                id: "l2".into(),
                title: "Inbox".into(),
                ..Default::default()
            },
            TaskList {
                id: "lw".into(),
                title: "Work".into(),
                ..Default::default()
            },
        ];
        assert_eq!(resolve_list(&lists, "lw").unwrap().id, "lw");
        assert_eq!(resolve_list(&lists, "Work").unwrap().id, "lw");
        assert!(matches!(
            resolve_list(&lists, "Inbox"),
            Err(Error::Usage(_))
        ));
        assert!(matches!(resolve_list(&lists, "nope"), Err(Error::Usage(_))));
        assert_eq!(resolve_list(&lists, "l1").unwrap().title, "Inbox");
    }

    #[test]
    fn format_defaults_to_md_and_is_global() {
        let a = parse(&["lists"]);
        assert_eq!(a.format, Format::Md);
        assert_eq!(parse(&["-f", "json", "lists"]).format, Format::Json);
        assert_eq!(parse(&["lists", "--format", "json"]).format, Format::Json);
    }

    #[test]
    fn lists_markdown_and_json() {
        let mut api = Fake::new();
        let (_, md) = run(&mut api, &["lists"]);
        assert_eq!(md, "- Inbox (`l1`)\n- Inbox (`l2`)\n- Work (`lw`)\n");
        let (_, json) = run(&mut api, &["lists", "-f", "json"]);
        let v: Vec<TaskList> = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn tasks_one_list_and_whole_board() {
        let mut api = Fake::new();
        let (res, md) = run(&mut api, &["tasks", "--list", "Work"]);
        assert!(res.is_ok());
        assert_eq!(
            md,
            "# Work (`lw`)\n\n- [x] Ship CLI (`t2`)\n  Phase 3\n  see PR\n"
        );
        let (_, json) = run(&mut api, &["-f", "json", "tasks", "--list", "Work"]);
        let v: Vec<Task> = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(v[0].id, "t2");
        let (_, board) = run(&mut api, &["-f", "json", "tasks"]);
        let cache: cache::Cache = serde_json::from_str(board.trim()).unwrap();
        assert_eq!(cache.lists.len(), 3);
        let (_, board_md) = run(&mut api, &["tasks"]);
        assert!(board_md.contains("# Inbox (`l1`)"));
        assert!(board_md.contains("- [ ] Buy milk (`t1`)"));
        assert!(board_md.contains("# Work (`lw`)"));
        assert!(board_md.contains("Phase 3"));
    }

    #[test]
    fn get_needs_list_when_id_is_duplicated() {
        let mut api = Fake::new();
        let (res, _) = run(&mut api, &["get", "t1"]);
        assert!(matches!(res, Err(Error::Usage(m)) if m.contains("more than one")));
        let (_, md) = run(&mut api, &["get", "t1", "--list", "l1"]);
        assert_eq!(md, "# Buy milk\n\n- id: `t1`\n- status: needsAction\n");
        let (_, json) = run(&mut api, &["-f", "json", "get", "t1", "--list", "l1"]);
        let t: Task = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(t.title, "Buy milk");
    }

    #[test]
    fn create_sends_optional_fields_and_refetches_the_list() {
        let mut api = Fake::new();
        let fetches = api.task_fetches;
        let (res, json) = run(
            &mut api,
            &[
                "-f",
                "json",
                "create",
                "--list",
                "Work",
                "--title",
                "New",
                "--notes",
                "hi",
                "--due",
                "2026-09-21",
                "--parent",
                "t2",
            ],
        );
        assert!(res.is_ok());
        let t: Task = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(t.title, "New");
        assert_eq!(t.notes.as_deref(), Some("hi"));
        assert_eq!(t.due.as_deref(), Some("2026-09-21T00:00:00.000Z"));
        assert_eq!(t.parent.as_deref(), Some("t2"));
        assert_eq!(api.creates, 1);
        assert!(api.task_fetches > fetches);
        let cache = api.last_cache.as_ref().unwrap();
        assert!(cache
            .lists
            .iter()
            .any(|c| c.list.id == "lw" && c.tasks.iter().any(|t| t.title == "New")));
    }

    #[test]
    fn update_and_done_patch_only_given_fields() {
        let mut api = Fake::new();
        let (res, _) = run(&mut api, &["update", "t2"]);
        assert!(matches!(res, Err(Error::Usage(_))));
        let (_, json) = run(
            &mut api,
            &[
                "-f", "json", "update", "t2", "--title", "Shipped", "--list", "Work",
            ],
        );
        let t: Task = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(t.title, "Shipped");
        assert_eq!(api.patches[0], json!({ "title": "Shipped" }));
        let _ = run(&mut api, &["done", "t1", "--list", "l1"]);
        assert_eq!(api.patches.last().unwrap()["status"], "completed");
        let _ = run(&mut api, &["undone", "t2", "--list", "lw"]);
        assert_eq!(api.patches.last().unwrap()["status"], "needsAction");
    }

    #[test]
    fn assigned_tasks_reject_notes_updates() {
        let mut api = Fake::new();
        api.tasks.get_mut("lw").unwrap()[0].assignment_info = Some(crate::api::AssignmentInfo {
            link_to_task: "https://docs.google.com/document/d/1".into(),
            surface_type: "DOCUMENT".into(),
        });

        let (result, _) = run(
            &mut api,
            &["update", "t2", "--list", "Work", "--notes", "not allowed"],
        );

        assert!(
            matches!(result, Err(Error::Usage(message)) if message.contains("cannot have notes"))
        );
        assert!(api.patches.is_empty());
    }

    #[test]
    fn delete_requires_yes() {
        let mut api = Fake::new();
        let (res, out) = run(&mut api, &["delete", "t2", "--list", "Work"]);
        assert!(matches!(res, Err(Error::Usage(m)) if m.contains("--yes")));
        assert!(out.is_empty());
        let (res, out) = run(&mut api, &["delete", "t2", "--list", "Work", "--yes"]);
        assert!(res.is_ok());
        assert!(out.is_empty());
        assert_eq!(api.deletes, vec![("lw".into(), "t2".into())]);
    }

    #[test]
    fn md_yank_is_the_paste_shape() {
        // Nothing but a title, even when Google sends a link to the task.
        let bare = Task {
            web_view_link: Some("https://tasks.google.com/t/9".into()),
            ..task("t1", "Buy milk", "needsAction")
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
            ..task("t2", "Linked", "needsAction")
        };
        assert_eq!(
            md_yank(&t),
            "# Linked\n\n\
             - [https://bare.test/x](<https://bare.test/x>)\n\
             - [a \\[b\\] c](<https://named.test>)\n"
        );

        // Due and notes, in that order, each its own block.
        let t = Task {
            due: Some("2026-09-25T00:00:00.000Z".into()),
            notes: Some("  line one\nline two  ".into()),
            ..task("t3", "Both", "needsAction")
        };
        assert_eq!(
            md_yank(&t),
            "# Both\n\nDue: 2026-09-25\n\nline one\nline two\n"
        );
    }

    #[test]
    fn due_must_be_a_date() {
        assert!(due_rfc3339("nope").is_err());
        assert_eq!(
            due_rfc3339("2026-01-02").unwrap(),
            "2026-01-02T00:00:00.000Z"
        );
    }

    #[test]
    fn api_failures_are_exit_one() {
        let mut api = Fake::new();
        api.fail = Some("sign-in expired".into());
        let err = run(&mut api, &["lists"]).0.unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert!(matches!(err, Error::Fail(m) if m.contains("sign-in expired")));
    }

    #[test]
    fn list_create_update_delete() {
        let mut api = Fake::new();
        let (res, md) = run(&mut api, &["list", "create", "--title", "Errands"]);
        assert!(res.is_ok());
        assert_eq!(md, "- Errands (`n10`)\n");
        assert!(api
            .lists
            .iter()
            .any(|l| l.id == "n10" && l.title == "Errands"));
        let cache = api.last_cache.as_ref().unwrap();
        assert!(cache
            .lists
            .iter()
            .any(|c| c.list.id == "n10" && c.tasks.is_empty()));

        let (_, json) = run(
            &mut api,
            &["-f", "json", "list", "create", "--title", "Later"],
        );
        let created: TaskList = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(created.title, "Later");

        let (res, md) = run(&mut api, &["list", "update", "Work", "--title", "Projects"]);
        assert!(res.is_ok());
        assert_eq!(md, "- Projects (`lw`)\n");
        assert_eq!(
            api.lists.iter().find(|l| l.id == "lw").unwrap().title,
            "Projects"
        );
        let cache = api.last_cache.as_ref().unwrap();
        assert!(cache
            .lists
            .iter()
            .any(|c| c.list.id == "lw" && c.list.title == "Projects"));

        let (res, _) = run(&mut api, &["list", "update", "lw", "--title", "Office"]);
        assert!(res.is_ok());
        assert_eq!(
            api.lists.iter().find(|l| l.id == "lw").unwrap().title,
            "Office"
        );

        let (res, _) = run(&mut api, &["list", "update", "Inbox", "--title", "Nope"]);
        assert!(matches!(res, Err(Error::Usage(m)) if m.contains("not unique")));

        let (res, _) = run(&mut api, &["list", "create", "--title", "  "]);
        assert!(matches!(res, Err(Error::Usage(m)) if m.contains("title")));

        let (res, out) = run(&mut api, &["list", "delete", "Office"]);
        assert!(matches!(res, Err(Error::Usage(m)) if m.contains("--yes") && m.contains("Office")));
        assert!(out.is_empty());
        assert!(api.lists.iter().any(|l| l.id == "lw"));

        let (res, out) = run(&mut api, &["list", "delete", "lw", "--yes"]);
        assert!(res.is_ok());
        assert!(out.is_empty());
        assert!(api.lists.iter().all(|l| l.id != "lw"));
        assert!(!api.tasks.contains_key("lw"));
        let cache = api.last_cache.as_ref().unwrap();
        assert!(cache.lists.iter().all(|c| c.list.id != "lw"));
    }
}
