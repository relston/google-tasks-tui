use std::sync::mpsc::{Receiver, Sender};

use crate::api::{CreateTask, Task, TaskList, TaskPatch, TasksApi};

use super::board::move_requests;

pub(super) enum Cmd {
    Refresh(u64),
    Patch {
        operation: u64,
        list_id: String,
        task_id: String,
        patch: TaskPatch,
        etag: String,
    },
    Delete {
        operation: u64,
        list_id: String,
        task_id: String,
        etag: String,
    },
    Create {
        operation: u64,
        list_id: String,
        task: CreateTask,
        parent: Option<String>,
        selection: Option<String>,
    },
    Move {
        operation: u64,
        from: String,
        to: String,
        ids: Vec<String>,
    },
}

pub(super) enum Msg {
    Lists(u64, Result<Vec<TaskList>, String>),
    Tasks(u64, String, Result<Vec<Task>, String>),
    Updated {
        operation: u64,
        list_id: String,
        task_id: String,
        result: Result<Task, String>,
    },
    Deleted {
        operation: u64,
        list_id: String,
        task_id: String,
        result: Result<(), String>,
    },
    Created {
        operation: u64,
        list_id: String,
        selection: Option<String>,
        result: Result<Task, String>,
    },
    Moved {
        operation: u64,
        from: String,
        to: String,
        ids: Vec<String>,
        result: MoveOutcome,
    },
}

pub(super) struct MoveFailure {
    pub task_id: String,
    pub error: String,
}

pub(super) struct MoveOutcome {
    pub moved: Vec<Task>,
    pub failure: Option<MoveFailure>,
    pub source: Result<Vec<Task>, String>,
    pub destination: Result<Vec<Task>, String>,
}

pub(super) fn worker<A: TasksApi>(mut client: A, commands: Receiver<Cmd>, output: Sender<Msg>) {
    for command in commands {
        let sent = match command {
            Cmd::Create {
                operation,
                list_id,
                task,
                parent,
                selection,
            } => output.send(Msg::Created {
                operation,
                result: client
                    .create(&list_id, &task, parent.as_deref())
                    .map_err(|error| error.to_string()),
                list_id,
                selection,
            }),
            Cmd::Delete {
                operation,
                list_id,
                task_id,
                etag,
            } => output.send(Msg::Deleted {
                operation,
                result: client
                    .delete(&list_id, &task_id, Some(&etag))
                    .map_err(|error| error.to_string()),
                list_id,
                task_id,
            }),
            Cmd::Patch {
                operation,
                list_id,
                task_id,
                patch,
                etag,
            } => output.send(Msg::Updated {
                operation,
                result: client
                    .patch(&list_id, &task_id, &patch, Some(&etag))
                    .map_err(|error| error.to_string()),
                list_id,
                task_id,
            }),
            Cmd::Move {
                operation,
                from,
                to,
                ids,
            } => {
                let mut moved = Vec::new();
                let mut failure = None;
                let requests = move_requests(&to, &ids);
                if requests.is_empty() {
                    failure = Some(MoveFailure {
                        task_id: String::new(),
                        error: "no task".into(),
                    });
                } else {
                    for (id, movement) in requests {
                        match client.move_task(&from, &id, &movement) {
                            Ok(task) => moved.push(task),
                            Err(error) => {
                                failure = Some(MoveFailure {
                                    task_id: id,
                                    error: error.to_string(),
                                });
                                break;
                            }
                        }
                    }
                }
                let source = client.tasks(&from).map_err(|error| error.to_string());
                let destination = client.tasks(&to).map_err(|error| error.to_string());
                output.send(Msg::Moved {
                    operation,
                    from,
                    to,
                    ids,
                    result: MoveOutcome {
                        moved,
                        failure,
                        source,
                        destination,
                    },
                })
            }
            Cmd::Refresh(generation) => {
                let lists = client.tasklists().map_err(|error| error.to_string());
                let ids: Vec<String> = lists.iter().flatten().map(|list| list.id.clone()).collect();
                if output.send(Msg::Lists(generation, lists)).is_err() {
                    return;
                }
                for id in ids {
                    let tasks = client.tasks(&id).map_err(|error| error.to_string());
                    if output.send(Msg::Tasks(generation, id, tasks)).is_err() {
                        return;
                    }
                }
                continue;
            }
        };
        if sent.is_err() {
            return;
        }
    }
}
