use std::sync::mpsc::{Receiver, Sender};

use crate::api::{CreateTask, MoveTask, Task, TaskList, TaskPatch, TasksApi};

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
    Reparent {
        operation: u64,
        list_id: String,
        task_id: String,
        parent: Option<String>,
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
    Reparented {
        operation: u64,
        list_id: String,
        task_id: String,
        parent: Option<String>,
        result: Result<Task, String>,
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
            Cmd::Reparent {
                operation,
                list_id,
                task_id,
                parent,
            } => output.send(Msg::Reparented {
                operation,
                result: client
                    .move_task(
                        &list_id,
                        &task_id,
                        &MoveTask {
                            destination_list: None,
                            parent: parent.clone(),
                            previous: None,
                        },
                    )
                    .map_err(|error| error.to_string()),
                list_id,
                task_id,
                parent,
            }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Error, ListPatch, Result};
    use std::sync::{mpsc, Arc, Mutex};

    #[derive(Clone, Default)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        fail_move: Option<String>,
    }

    impl Fake {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn record(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }
    }

    impl TasksApi for Fake {
        fn account_fingerprint(&self) -> String {
            "worker-test".into()
        }

        fn tasklists(&mut self) -> Result<Vec<TaskList>> {
            self.record("tasklists".into());
            Ok(vec![TaskList {
                id: "list".into(),
                ..Default::default()
            }])
        }

        fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>> {
            self.record(format!("tasks:{list_id}"));
            Ok(vec![Task {
                id: format!("{list_id}-task"),
                ..Default::default()
            }])
        }

        fn patch(
            &mut self,
            list_id: &str,
            task_id: &str,
            _patch: &TaskPatch,
            etag: Option<&str>,
        ) -> Result<Task> {
            self.record(format!("patch:{list_id}:{task_id}:{}", etag.unwrap_or("")));
            Ok(Task {
                id: task_id.into(),
                ..Default::default()
            })
        }

        fn create(
            &mut self,
            list_id: &str,
            task: &CreateTask,
            parent: Option<&str>,
        ) -> Result<Task> {
            self.record(format!(
                "create:{list_id}:{}:{}",
                task.title,
                parent.unwrap_or("")
            ));
            Ok(Task {
                id: "created".into(),
                title: task.title.clone(),
                ..Default::default()
            })
        }

        fn delete(&mut self, list_id: &str, task_id: &str, etag: Option<&str>) -> Result<()> {
            self.record(format!("delete:{list_id}:{task_id}:{}", etag.unwrap_or("")));
            Ok(())
        }

        fn move_task(&mut self, list_id: &str, task_id: &str, movement: &MoveTask) -> Result<Task> {
            self.record(format!(
                "move:{list_id}:{task_id}:{}:{}:{}",
                movement.destination_list.as_deref().unwrap_or(""),
                movement.parent.as_deref().unwrap_or(""),
                movement.previous.as_deref().unwrap_or("")
            ));
            if self.fail_move.as_deref() == Some(task_id) {
                return Err(Error::Transport(format!("{task_id} failed")));
            }
            Ok(Task {
                id: task_id.into(),
                parent: movement.parent.clone(),
                ..Default::default()
            })
        }

        fn create_list(&mut self, title: &str) -> Result<TaskList> {
            Ok(TaskList {
                id: "created-list".into(),
                title: title.into(),
                ..Default::default()
            })
        }

        fn patch_list(
            &mut self,
            list_id: &str,
            patch: &ListPatch,
            _etag: Option<&str>,
        ) -> Result<TaskList> {
            Ok(TaskList {
                id: list_id.into(),
                title: patch.title.clone(),
                ..Default::default()
            })
        }

        fn delete_list(&mut self, _list_id: &str) -> Result<()> {
            Ok(())
        }
    }

    fn run_one(fake: Fake, command: Cmd) -> Msg {
        let (commands, input) = mpsc::channel();
        let (output, messages) = mpsc::channel();
        commands.send(command).unwrap();
        drop(commands);
        worker(fake, input, output);
        messages.recv().unwrap()
    }

    #[test]
    fn move_executes_parent_then_ordered_children_and_preserves_context() {
        let fake = Fake::default();
        let message = run_one(
            fake.clone(),
            Cmd::Move {
                operation: 42,
                from: "source".into(),
                to: "destination".into(),
                ids: vec!["parent".into(), "first".into(), "second".into()],
            },
        );

        assert_eq!(
            fake.calls(),
            [
                "move:source:parent:destination::",
                "move:source:first:destination:parent:",
                "move:source:second:destination:parent:first",
                "tasks:source",
                "tasks:destination",
            ]
        );
        let Msg::Moved {
            operation,
            from,
            to,
            ids,
            result,
        } = message
        else {
            panic!("expected move result");
        };
        assert_eq!(operation, 42);
        assert_eq!(from, "source");
        assert_eq!(to, "destination");
        assert_eq!(ids, ["parent", "first", "second"]);
        assert_eq!(result.moved.len(), 3);
        assert!(result.failure.is_none());
        assert!(result.source.is_ok());
        assert!(result.destination.is_ok());
    }

    #[test]
    fn partial_move_stops_at_failure_and_still_reconciles_both_lists() {
        let fake = Fake {
            fail_move: Some("first".into()),
            ..Default::default()
        };
        let message = run_one(
            fake.clone(),
            Cmd::Move {
                operation: 7,
                from: "source".into(),
                to: "destination".into(),
                ids: vec!["parent".into(), "first".into(), "never".into()],
            },
        );

        let Msg::Moved { result, .. } = message else {
            panic!("expected move result");
        };
        assert_eq!(
            result
                .moved
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["parent"]
        );
        let failure = result.failure.unwrap();
        assert_eq!(failure.task_id, "first");
        assert!(failure.error.contains("first failed"));
        assert_eq!(
            fake.calls(),
            [
                "move:source:parent:destination::",
                "move:source:first:destination:parent:",
                "tasks:source",
                "tasks:destination",
            ]
        );
    }

    #[test]
    fn mutation_responses_echo_operation_and_resource_context() {
        let message = run_one(
            Fake::default(),
            Cmd::Patch {
                operation: 99,
                list_id: "list".into(),
                task_id: "task".into(),
                patch: TaskPatch {
                    title: Some("changed".into()),
                    ..Default::default()
                },
                etag: "\"v1\"".into(),
            },
        );

        let Msg::Updated {
            operation,
            list_id,
            task_id,
            result,
        } = message
        else {
            panic!("expected update result");
        };
        assert_eq!(
            (operation, list_id.as_str(), task_id.as_str()),
            (99, "list", "task")
        );
        assert_eq!(result.unwrap().id, "task");
    }

    #[test]
    fn reparent_uses_a_same_list_move_and_preserves_context() {
        let fake = Fake::default();
        let message = run_one(
            fake.clone(),
            Cmd::Reparent {
                operation: 7,
                list_id: "list".into(),
                task_id: "child".into(),
                parent: Some("parent".into()),
            },
        );

        assert_eq!(fake.calls(), ["move:list:child::parent:"]);
        let Msg::Reparented {
            operation,
            list_id,
            task_id,
            parent,
            result,
        } = message
        else {
            panic!("expected reparent result");
        };
        assert_eq!(operation, 7);
        assert_eq!(list_id, "list");
        assert_eq!(task_id, "child");
        assert_eq!(parent.as_deref(), Some("parent"));
        assert_eq!(result.unwrap().parent.as_deref(), Some("parent"));
    }

    #[test]
    fn channel_closure_terminates_without_extra_api_work() {
        let fake = Fake::default();
        let (commands, input) = mpsc::channel();
        let (output, messages) = mpsc::channel();
        drop(messages);
        commands.send(Cmd::Refresh(1)).unwrap();
        commands.send(Cmd::Refresh(2)).unwrap();
        drop(commands);

        worker(fake.clone(), input, output);

        assert_eq!(fake.calls(), ["tasklists"]);

        let (commands, input) = mpsc::channel();
        let (output, messages) = mpsc::channel();
        drop(commands);
        worker(Fake::default(), input, output);
        assert!(messages.try_recv().is_err());
    }
}
