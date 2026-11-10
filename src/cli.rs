//! One-shot CLI: markdown on stdout by default, diagnostics on stderr.

use std::io::{self, Write};

use chrono::NaiveDate;
use clap::{builder::PossibleValue, ArgGroup, Parser, Subcommand, ValueEnum};

use crate::board::{
    cross_list_block, eligible_parents, move_ids, ordered, reorder_after, reorder_step,
    subtask_block, Board,
};
use crate::cache;
use crate::google::{
    auth::Auth, move_tree, Client, CreateTask, Error as ApiError, ListPatch, MoveTask, TaskPatch,
    TasksApi,
};
use crate::markdown::{md_lists, md_task, md_task_section};
use crate::model::{normalize_title, Status, Task, TaskList};
use crate::paths::Paths;

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
    /// Print the complete documented default configuration and exit.
    #[arg(long, global = true)]
    pub default_config: bool,
    /// Open a blank TUI with a centered date panel (due-date editing scratch).
    #[arg(long, global = true)]
    pub debug_date_time: bool,
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
        #[arg(long, value_enum)]
        status: Option<Status>,
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
    /// Reorder, reparent, or move a task to another list.
    #[command(group(
        ArgGroup::new("kind")
            .required(true)
            .args(["up", "down", "first", "after", "parent", "top_level", "to_list"])
    ))]
    Move {
        task_id: String,
        #[arg(short, long)]
        list: Option<String>,
        /// One place up among open siblings.
        #[arg(long)]
        up: bool,
        /// One place down among open siblings.
        #[arg(long)]
        down: bool,
        /// First among open siblings.
        #[arg(long)]
        first: bool,
        /// Place after this open sibling.
        #[arg(long)]
        after: Option<String>,
        /// Make this task a subtask of an open top-level task.
        #[arg(long)]
        parent: Option<String>,
        /// Promote a subtask to the top level.
        #[arg(long)]
        top_level: bool,
        /// Move the task and its subtasks into another list.
        #[arg(long)]
        to_list: Option<String>,
    },
    /// Create, rename, or delete a task list.
    List {
        #[command(subcommand)]
        cmd: ListCmd,
    },
    /// Forget the saved Google sign-in so the next run opens the browser.
    Logout,
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

impl ValueEnum for Status {
    fn value_variants<'a>() -> &'a [Self] {
        &[Status::NeedsAction, Status::Completed]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(PossibleValue::new(self.as_str()))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Usage(String),
    Fail(String),
}

impl Error {
    pub fn exit_code(&self) -> u8 {
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

/// Sign in, then run one command against Google.
pub fn run(
    paths: &Paths,
    command: Command,
    format: Format,
    out: &mut impl Write,
) -> Result<(), Error> {
    if matches!(command, Command::Logout) {
        return logout(paths, out);
    }
    let auth = Auth::load(paths).map_err(|error| Error::Fail(error.to_string()))?;
    execute(&mut Client::new(auth), command, format, out, paths)
}

fn logout(paths: &Paths, out: &mut impl Write) -> Result<(), Error> {
    let token = paths.token();
    let existed = crate::google::auth::clear_cached_token(paths)
        .map_err(|error| Error::Fail(error.to_string()))?;
    if existed {
        writeln!(
            out,
            "Signed out. Removed {}.\nNext run will open a browser to sign in again.",
            token.display()
        )
        .map_err(|error| Error::Fail(error.to_string()))?;
    } else {
        writeln!(
            out,
            "Already signed out (no token at {}).\nNext run will open a browser to sign in.",
            token.display()
        )
        .map_err(|error| Error::Fail(error.to_string()))?;
    }
    Ok(())
}

fn execute(
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
                            .map(|(list, tasks)| cache::CachedList {
                                list,
                                tasks: ordered(tasks).into_iter().map(|row| row.task).collect(),
                            })
                            .collect();
                        write_json(out, &cache::Cache { lists })?;
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
                due: due.as_deref().map(parse_due).transpose()?,
            };
            let task = api
                .create(&list.id, &task, parent.as_deref())
                .map_err(api_fail)?;
            write_task(out, format, &task)?;
            refresh_cache(api, paths, &list.id);
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
                due: due.as_deref().map(parse_due).transpose()?,
                status,
                clear_completed: false,
            };
            let task = api
                .patch(&list_id, &task.id, &patch, Some(&task.etag))
                .map_err(api_fail)?;
            write_task(out, format, &task)?;
            refresh_cache(api, paths, &list_id);
        }
        Command::Done { task_id, list } => {
            patch_status(
                api,
                out,
                format,
                paths,
                &task_id,
                list.as_deref(),
                Status::Completed,
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
                Status::NeedsAction,
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
            refresh_cache(api, paths, &list_id);
        }
        Command::Move {
            task_id,
            list,
            up,
            down,
            first,
            after,
            parent,
            top_level,
            to_list,
        } => {
            move_command(
                api,
                out,
                format,
                paths,
                &task_id,
                list.as_deref(),
                MoveKind::from_flags(up, down, first, after, parent, top_level, to_list)?,
            )?;
        }
        Command::Logout => unreachable!("logout is handled before Auth::load"),
        Command::List { cmd } => match cmd {
            ListCmd::Create { title } => {
                let title = normalize_title(&title)
                    .map_err(|_| Error::Usage("a list needs a title".into()))?;
                let list = api.create_list(&title).map_err(api_fail)?;
                write_list(out, format, &list)?;
                cache_upsert_list(api, paths, list);
            }
            ListCmd::Update { list, title } => {
                let title = normalize_title(&title)
                    .map_err(|_| Error::Usage("a list needs a title".into()))?;
                let lists = tasklists(api)?;
                let list = resolve_list(&lists, &list)?;
                let list = api
                    .patch_list(&list.id, &ListPatch { title }, Some(&list.etag))
                    .map_err(api_fail)?;
                write_list(out, format, &list)?;
                cache_upsert_list(api, paths, list);
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
                cache_remove_list(api, paths, &id);
            }
        },
    }
    Ok(())
}

enum MoveKind {
    Up,
    Down,
    First,
    After(String),
    Parent(String),
    TopLevel,
    ToList(String),
}

impl MoveKind {
    fn from_flags(
        up: bool,
        down: bool,
        first: bool,
        after: Option<String>,
        parent: Option<String>,
        top_level: bool,
        to_list: Option<String>,
    ) -> Result<Self, Error> {
        Ok(match (up, down, first, after, parent, top_level, to_list) {
            (true, false, false, None, None, false, None) => Self::Up,
            (false, true, false, None, None, false, None) => Self::Down,
            (false, false, true, None, None, false, None) => Self::First,
            (false, false, false, Some(after), None, false, None) => Self::After(after),
            (false, false, false, None, Some(parent), false, None) => Self::Parent(parent),
            (false, false, false, None, None, true, None) => Self::TopLevel,
            (false, false, false, None, None, false, Some(to_list)) => Self::ToList(to_list),
            _ => {
                return Err(Error::Usage(
                    "move needs exactly one of --up, --down, --first, --after, --parent, --top-level, --to-list"
                        .into(),
                ))
            }
        })
    }
}

fn move_command(
    api: &mut impl TasksApi,
    out: &mut impl Write,
    format: Format,
    paths: &Paths,
    task_id: &str,
    list: Option<&str>,
    kind: MoveKind,
) -> Result<(), Error> {
    match kind {
        MoveKind::ToList(dest) => move_to_list(api, out, format, paths, task_id, list, &dest),
        kind => place_in_list(api, out, format, paths, task_id, list, kind),
    }
}

fn place_in_list(
    api: &mut impl TasksApi,
    out: &mut impl Write,
    format: Format,
    paths: &Paths,
    task_id: &str,
    list: Option<&str>,
    kind: MoveKind,
) -> Result<(), Error> {
    let (list_id, task) = find_task(api, task_id, list)?;
    let rows = ordered(tasks(api, &list_id)?);
    let (parent, previous) = match kind {
        MoveKind::Up | MoveKind::Down => {
            let plan = reorder_step(&rows, &task.id, matches!(kind, MoveKind::Down))
                .map_err(|error| Error::Usage(error.into()))?;
            (plan.parent, plan.previous)
        }
        MoveKind::First => {
            let plan = reorder_after(&rows, &task.id, None).map_err(reorder_usage)?;
            (plan.parent, plan.previous)
        }
        MoveKind::After(after) => {
            let plan = reorder_after(&rows, &task.id, Some(&after)).map_err(reorder_usage)?;
            (plan.parent, plan.previous)
        }
        MoveKind::Parent(parent_id) => {
            if let Some(error) = subtask_block(&rows, &task.id) {
                return Err(Error::Usage(error.into()));
            }
            if !eligible_parents(&rows, &task.id)
                .iter()
                .any(|parent| parent.id == parent_id)
            {
                return Err(Error::Usage(
                    "--parent must be an open top-level task in this list".into(),
                ));
            }
            (Some(parent_id), None)
        }
        MoveKind::TopLevel => {
            let Some(former) = task.parent.clone() else {
                return Err(Error::Usage("task is already top-level".into()));
            };
            (None, Some(former))
        }
        MoveKind::ToList(_) => unreachable!("handled by move_to_list"),
    };
    api.move_task(
        &list_id,
        &task.id,
        &MoveTask {
            destination_list: None,
            parent,
            previous,
        },
    )
    .map_err(api_fail)?;
    let reloaded = tasks(api, &list_id)?;
    let task = reloaded
        .into_iter()
        .find(|candidate| candidate.id == task.id)
        .ok_or_else(|| Error::Fail("moved task missing after reload".into()))?;
    write_task(out, format, &task)?;
    refresh_cache(api, paths, &list_id);
    Ok(())
}

fn move_to_list(
    api: &mut impl TasksApi,
    out: &mut impl Write,
    format: Format,
    paths: &Paths,
    task_id: &str,
    list: Option<&str>,
    dest_spec: &str,
) -> Result<(), Error> {
    let (from_id, task) = find_task(api, task_id, list)?;
    let lists = tasklists(api)?;
    let dest = resolve_list(&lists, dest_spec)?;
    if dest.id == from_id {
        return Err(Error::Usage("--to-list must be a different list".into()));
    }
    let source_tasks = tasks(api, &from_id)?;
    let rows = ordered(source_tasks);
    if let Some(error) = cross_list_block(&rows, &task.id) {
        return Err(Error::Usage(error.into()));
    }
    let ids = move_ids(&rows, &task.id);
    if ids.is_empty() {
        return Err(Error::Usage(format!("no task {:?}", task.id)));
    }
    let total = ids.len();
    let tree = move_tree(api, &from_id, &dest.id, &ids).map_err(api_fail)?;
    if let Some((failed, error)) = tree.partial {
        return Err(Error::Fail(format!(
            "moved {} of {total}; task {failed} failed: {error}",
            tree.moved
        )));
    }
    let dest_tasks = tasks(api, &dest.id)?;
    let moved = dest_tasks
        .into_iter()
        .find(|candidate| candidate.id == task.id)
        .ok_or_else(|| Error::Fail("moved task missing after reload".into()))?;
    write_task(out, format, &moved)?;
    refresh_cache(api, paths, &from_id);
    refresh_cache(api, paths, &dest.id);
    Ok(())
}

fn reorder_usage(error: &str) -> Error {
    Error::Usage(
        match error {
            "its new neighbor is no longer beside it" => "--after must be an open sibling",
            other => other,
        }
        .into(),
    )
}

fn patch_status(
    api: &mut impl TasksApi,
    out: &mut impl Write,
    format: Format,
    paths: &Paths,
    task_id: &str,
    list: Option<&str>,
    status: Status,
) -> Result<(), Error> {
    let (list_id, task) = find_task(api, task_id, list)?;
    let task = api
        .patch(
            &list_id,
            &task.id,
            &TaskPatch::status(status),
            Some(&task.etag),
        )
        .map_err(api_fail)?;
    write_task(out, format, &task)?;
    refresh_cache(api, paths, &list_id);
    Ok(())
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

fn parse_due(d: &str) -> Result<NaiveDate, Error> {
    NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .map_err(|_| Error::Usage(format!("due date must be YYYY-MM-DD, not {d:?}")))
}

fn cache_state(api: &impl TasksApi, paths: &Paths) -> Result<(Board, cache::Meta), String> {
    let account = api.account_fingerprint();
    let (cache, meta) =
        cache::load_for(paths, &account).map_err(|error| format!("reading cache: {error}"))?;
    Ok((Board::from_cache(cache), meta))
}

fn save_board(paths: &Paths, board: &Board, meta: &mut cache::Meta) -> Result<(), String> {
    cache::save_for(paths, &board.snapshot(), meta)
        .map_err(|error| format!("writing cache: {error}"))
}

/// Reload one list into the startup cache. Failures are disposable: warn on
/// stderr and leave the Google mutation's printed result alone.
fn refresh_cache(api: &mut impl TasksApi, paths: &Paths, list_id: &str) {
    if let Err(error) = refresh_cache_inner(api, paths, list_id) {
        eprintln!("warning: local cache update failed: {error}");
    }
}

fn refresh_cache_inner(
    api: &mut impl TasksApi,
    paths: &Paths,
    list_id: &str,
) -> Result<(), String> {
    let tasks = api
        .tasks(list_id)
        .map_err(|error| format!("reloading list: {error}"))?;
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

fn cache_upsert_list(api: &impl TasksApi, paths: &Paths, list: TaskList) {
    if let Err(error) = cache_upsert_list_inner(api, paths, list) {
        eprintln!("warning: local cache update failed: {error}");
    }
}

fn cache_upsert_list_inner(
    api: &impl TasksApi,
    paths: &Paths,
    list: TaskList,
) -> Result<(), String> {
    let (mut board, mut meta) = cache_state(api, paths)?;
    board.upsert_list(list);
    save_board(paths, &board, &mut meta)
}

fn cache_remove_list(api: &impl TasksApi, paths: &Paths, list_id: &str) {
    if let Err(error) = cache_remove_list_inner(api, paths, list_id) {
        eprintln!("warning: local cache update failed: {error}");
    }
}

fn cache_remove_list_inner(
    api: &impl TasksApi,
    paths: &Paths,
    list_id: &str,
) -> Result<(), String> {
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
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fake {
        lists: Vec<TaskList>,
        tasks: HashMap<String, Vec<Task>>,
        fail: Option<String>,
        /// Fail `tasks()` once `task_fetches` reaches this 1-based count.
        fail_tasks_from: Option<usize>,
        /// Fail `move_task` for these task ids.
        fail_moves: Vec<String>,
        creates: usize,
        patches: Vec<Value>,
        deletes: Vec<(String, String)>,
        moves: Vec<(String, String, MoveTask)>,
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
            let order = TaskList {
                id: "lo".into(),
                title: "Order".into(),
                ..Default::default()
            };
            let t1 = task("t1", "Buy milk", Status::NeedsAction);
            let t2 = Task {
                notes: Some("Phase 3\nsee PR".into()),
                ..task("t2", "Ship CLI", Status::Completed)
            };
            let a = Task {
                position: "1".into(),
                ..task("a", "Alpha", Status::NeedsAction)
            };
            let a1 = Task {
                parent: Some("a".into()),
                position: "1".into(),
                ..task("a1", "Alpha child", Status::NeedsAction)
            };
            let b = Task {
                position: "2".into(),
                ..task("b", "Beta", Status::NeedsAction)
            };
            let c = Task {
                position: "3".into(),
                ..task("c", "Gamma", Status::NeedsAction)
            };
            Self {
                lists: vec![inbox.clone(), other, work, order],
                tasks: HashMap::from([
                    ("l1".into(), vec![t1]),
                    ("l2".into(), vec![task("t1", "dup", Status::NeedsAction)]),
                    ("lw".into(), vec![t2]),
                    ("lo".into(), vec![a, a1, b, c]),
                ]),
                fail: None,
                fail_tasks_from: None,
                fail_moves: Vec::new(),
                creates: 0,
                patches: Vec::new(),
                deletes: Vec::new(),
                moves: Vec::new(),
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

        fn tasklists(&mut self) -> crate::google::Result<Vec<TaskList>> {
            if let Some(e) = &self.fail {
                return Err(ApiError::Transport(e.clone()));
            }
            Ok(self.lists.clone())
        }

        fn tasks(&mut self, list_id: &str) -> crate::google::Result<Vec<Task>> {
            self.task_fetches += 1;
            if let Some(from) = self.fail_tasks_from {
                if self.task_fetches >= from {
                    return Err(ApiError::Transport("reload failed".into()));
                }
            }
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
        ) -> crate::google::Result<Task> {
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
            if patch.due.is_some() {
                task.due = patch.due;
            }
            if let Some(status) = patch.status {
                task.status = status;
            }
            if patch.clear_completed {
                task.completed = None;
            }
            Ok(task.clone())
        }

        fn create(
            &mut self,
            list_id: &str,
            create: &CreateTask,
            parent: Option<&str>,
        ) -> crate::google::Result<Task> {
            self.creates += 1;
            let t = Task {
                id: format!("n{}", self.next_id),
                title: create.title.clone(),
                notes: create.notes.clone(),
                due: create.due,
                parent: parent.map(str::to_string),
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
        ) -> crate::google::Result<()> {
            self.deletes.push((list_id.into(), task_id.into()));
            if let Some(tasks) = self.tasks.get_mut(list_id) {
                tasks.retain(|t| t.id != task_id);
            }
            Ok(())
        }

        fn clear_completed(&mut self, list_id: &str) -> crate::google::Result<()> {
            if let Some(tasks) = self.tasks.get_mut(list_id) {
                tasks.retain(|task| !task.is_done());
            }
            Ok(())
        }

        fn move_task(
            &mut self,
            list_id: &str,
            task_id: &str,
            movement: &MoveTask,
        ) -> crate::google::Result<Task> {
            self.moves
                .push((list_id.into(), task_id.into(), movement.clone()));
            if self.fail_moves.iter().any(|id| id == task_id) {
                return Err(ApiError::Transport("move refused".into()));
            }
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

        fn create_list(&mut self, title: &str) -> crate::google::Result<TaskList> {
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
        ) -> crate::google::Result<TaskList> {
            let list = self
                .lists
                .iter_mut()
                .find(|list| list.id == list_id)
                .ok_or_else(|| ApiError::Transport("no list".into()))?;
            list.title = patch.title.clone();
            Ok(list.clone())
        }

        fn delete_list(&mut self, list_id: &str) -> crate::google::Result<()> {
            self.lists.retain(|l| l.id != list_id);
            self.tasks.remove(list_id);
            Ok(())
        }
    }

    fn task(id: &str, title: &str, status: Status) -> Task {
        Task {
            id: id.into(),
            title: title.into(),
            status,
            ..Default::default()
        }
    }

    fn parse(args: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("google-tasks-tui").chain(args.iter().copied()))
            .unwrap()
    }

    fn run(api: &mut Fake, args: &[&str]) -> (Result<(), Error>, String) {
        run_with_cache(api, args, None)
    }

    fn run_with_cache(
        api: &mut Fake,
        args: &[&str],
        seed_cache: Option<&str>,
    ) -> (Result<(), Error>, String) {
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
            require_credentials_file: false,
            config_dir: root.clone(),
        };
        if let Some(raw) = seed_cache {
            fs::write(paths.cache(), raw).unwrap();
        }
        let parsed = parse(args);
        let mut out = Vec::new();
        let res = execute(
            api,
            parsed.command.expect("subcommand"),
            parsed.format,
            &mut out,
            &paths,
        );
        api.last_cache = cache::load_for(&paths, &api.account_fingerprint())
            .ok()
            .map(|(cache, _)| cache);
        let _ = fs::remove_dir_all(&root);
        (res, String::from_utf8(out).unwrap())
    }

    #[test]
    fn no_subcommand_means_tui() {
        let a = Args::try_parse_from(["google-tasks-tui"]).unwrap();
        assert!(a.command.is_none());
    }

    #[test]
    fn logout_deletes_the_saved_token_without_calling_google() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        let root = std::env::temp_dir().join(format!(
            "gtt-logout-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let paths = Paths {
            credentials: root.join("credentials.json"),
            require_credentials_file: false,
            config_dir: root.clone(),
        };
        let token = paths.token();
        fs::write(&token, r#"{"client_id":"x","refresh_token":"y"}"#).unwrap();
        assert!(matches!(parse(&["logout"]).command, Some(Command::Logout)));

        let mut out = Vec::new();
        super::run(&paths, Command::Logout, Format::Md, &mut out).unwrap();
        assert!(!token.exists(), "token.json should be removed");
        let message = String::from_utf8(out).unwrap();
        assert!(message.contains("Signed out"), "{message}");
        assert!(
            message.contains(token.to_string_lossy().as_ref()),
            "{message}"
        );

        // Idempotent when already signed out.
        let mut out = Vec::new();
        super::run(&paths, Command::Logout, Format::Md, &mut out).unwrap();
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("Already signed out"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn default_config_flag_needs_no_subcommand() {
        let args = Args::try_parse_from(["google-tasks-tui", "--default-config"]).unwrap();
        assert!(args.default_config);
        assert!(args.command.is_none());
    }

    #[test]
    fn debug_date_time_flag_needs_no_subcommand() {
        let args = Args::try_parse_from(["google-tasks-tui", "--debug-date-time"]).unwrap();
        assert!(args.debug_date_time);
        assert!(args.command.is_none());
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
        assert_eq!(
            md,
            "- Inbox (`l1`)\n- Inbox (`l2`)\n- Work (`lw`)\n- Order (`lo`)\n"
        );
        let (_, json) = run(&mut api, &["lists", "-f", "json"]);
        let v: Vec<TaskList> = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(v.len(), 4);
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
        assert_eq!(cache.lists.len(), 4);
        let (_, board_md) = run(&mut api, &["tasks"]);
        assert!(board_md.contains("# Inbox (`l1`)"));
        assert!(board_md.contains("- [ ] Buy milk (`t1`)"));
        assert!(board_md.contains("# Work (`lw`)"));
        assert!(board_md.contains("Phase 3"));
    }

    #[test]
    fn whole_board_json_uses_board_order_and_drops_deleted_tasks() {
        let mut api = Fake::new();
        api.tasks.insert(
            "lw".into(),
            vec![
                Task {
                    position: "0".into(),
                    ..task("done", "Done", Status::Completed)
                },
                Task {
                    parent: Some("parent".into()),
                    position: "0".into(),
                    ..task("child", "Child", Status::NeedsAction)
                },
                Task {
                    position: "1".into(),
                    ..task("parent", "Parent", Status::NeedsAction)
                },
                Task {
                    deleted: true,
                    ..task("gone", "Gone", Status::NeedsAction)
                },
            ],
        );

        let (_, json) = run(&mut api, &["-f", "json", "tasks"]);
        let board: cache::Cache = serde_json::from_str(json.trim()).unwrap();
        let work = board
            .lists
            .iter()
            .find(|list| list.list.id == "lw")
            .unwrap();
        assert_eq!(
            work.tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["parent", "child", "done"]
        );
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
        assert_eq!(t.due, NaiveDate::from_ymd_opt(2026, 9, 21));
        assert!(json.contains(r#""due":"2026-09-21T00:00:00.000Z""#));
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
    fn create_still_prints_when_the_cache_reload_fails() {
        let mut api = Fake::new();
        // create without --parent never fetches tasks until the cache refresh.
        api.fail_tasks_from = Some(1);
        let (res, json) = run(
            &mut api,
            &["-f", "json", "create", "--list", "Work", "--title", "New"],
        );
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(api.creates, 1);
        let t: Task = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(t.title, "New");
        assert_eq!(t.id, "n10");
    }

    #[test]
    fn create_still_prints_when_the_cache_file_is_malformed() {
        let mut api = Fake::new();
        let (res, json) = run_with_cache(
            &mut api,
            &["-f", "json", "create", "--list", "Work", "--title", "New"],
            Some("{not valid json"),
        );
        assert!(res.is_ok(), "{res:?}");
        let t: Task = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(t.title, "New");
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
        api.tasks.get_mut("lw").unwrap()[0].assignment_info = Some(crate::model::AssignmentInfo {
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
    fn due_must_be_a_date() {
        assert!(matches!(parse_due("nope"), Err(Error::Usage(_))));
        assert_eq!(
            parse_due("2026-01-02").unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()
        );
    }

    #[test]
    fn status_flag_accepts_google_spellings_only() {
        let parsed = parse(&["update", "t1", "--status", "needsAction"]);
        assert!(matches!(
            parsed.command,
            Some(Command::Update {
                status: Some(Status::NeedsAction),
                ..
            })
        ));
        let parsed = parse(&["update", "t1", "--status", "completed"]);
        assert!(matches!(
            parsed.command,
            Some(Command::Update {
                status: Some(Status::Completed),
                ..
            })
        ));
        assert!(Args::try_parse_from([
            "google-tasks-tui",
            "update",
            "t1",
            "--status",
            "needs-action"
        ])
        .is_err());
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
    fn move_requires_exactly_one_kind_of_flag() {
        assert!(Args::try_parse_from(["google-tasks-tui", "move", "a"]).is_err());
        assert!(Args::try_parse_from(["google-tasks-tui", "move", "a", "--up", "--down"]).is_err());
        let parsed = parse(&["move", "a", "--up"]);
        assert!(matches!(
            parsed.command,
            Some(Command::Move { up: true, .. })
        ));
    }

    #[test]
    fn move_reorders_among_open_siblings() {
        let mut api = Fake::new();
        let (res, _) = run(&mut api, &["move", "a", "--list", "Order", "--up"]);
        assert!(matches!(res, Err(Error::Usage(m)) if m.contains("already first")));
        assert!(api.moves.is_empty());

        let (res, md) = run(&mut api, &["move", "a", "--list", "Order", "--down"]);
        assert!(res.is_ok(), "{res:?}");
        assert!(md.contains("# Alpha"));
        assert_eq!(api.moves.len(), 1);
        assert_eq!(api.moves[0].1, "a");
        assert_eq!(api.moves[0].2.previous.as_deref(), Some("b"));
        assert!(api.moves[0].2.destination_list.is_none());

        api.moves.clear();
        let (res, _) = run(&mut api, &["move", "c", "--list", "Order", "--first"]);
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(api.moves[0].2.previous, None);

        api.moves.clear();
        let (res, _) = run(&mut api, &["move", "a", "--list", "Order", "--after", "c"]);
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(api.moves[0].2.previous.as_deref(), Some("c"));

        let (res, _) = run(
            &mut api,
            &["move", "a", "--list", "Order", "--after", "gone"],
        );
        assert!(matches!(res, Err(Error::Usage(m)) if m.contains("--after")));
    }

    #[test]
    fn move_reparents_and_promotes() {
        let mut api = Fake::new();
        let (res, _) = run(&mut api, &["move", "a", "--list", "Order", "--parent", "b"]);
        assert!(matches!(res, Err(Error::Usage(m)) if m.contains("subtasks")));
        assert!(api.moves.is_empty());

        let (res, md) = run(&mut api, &["move", "b", "--list", "Order", "--parent", "a"]);
        assert!(res.is_ok(), "{res:?}");
        assert!(md.contains("# Beta"));
        assert_eq!(api.moves[0].2.parent.as_deref(), Some("a"));
        assert_eq!(api.moves[0].2.previous, None);

        api.moves.clear();
        let (res, _) = run(&mut api, &["move", "a1", "--list", "Order", "--top-level"]);
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(api.moves[0].2.parent, None);
        assert_eq!(api.moves[0].2.previous.as_deref(), Some("a"));

        let (res, _) = run(&mut api, &["move", "c", "--list", "Order", "--top-level"]);
        assert!(matches!(res, Err(Error::Usage(m)) if m.contains("already top-level")));
    }

    #[test]
    fn move_to_list_keeps_subtask_order_and_reports_partial() {
        let mut api = Fake::new();
        let (res, md) = run(
            &mut api,
            &["move", "a", "--list", "Order", "--to-list", "Work"],
        );
        assert!(res.is_ok(), "{res:?}");
        assert!(md.contains("# Alpha"));
        assert_eq!(api.moves.len(), 2);
        assert_eq!(api.moves[0].1, "a");
        assert_eq!(api.moves[0].2.destination_list.as_deref(), Some("lw"));
        assert_eq!(api.moves[1].1, "a1");
        assert_eq!(api.moves[1].2.parent.as_deref(), Some("a"));
        assert!(api
            .tasks
            .get("lw")
            .unwrap()
            .iter()
            .any(|task| task.id == "a"));
        assert!(api
            .last_cache
            .as_ref()
            .unwrap()
            .lists
            .iter()
            .any(|list| list.list.id == "lw" && list.tasks.iter().any(|task| task.id == "a")));

        let mut api = Fake::new();
        api.fail_moves = vec!["a1".into()];
        let (res, out) = run(
            &mut api,
            &["move", "a", "--list", "Order", "--to-list", "Work"],
        );
        assert!(
            matches!(res, Err(Error::Fail(m)) if m.contains("moved 1 of 2") && m.contains("a1"))
        );
        assert!(out.is_empty());
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
