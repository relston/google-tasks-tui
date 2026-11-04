//! The background thread that talks to Google. Commands arrive one at a
//! time, so mutations are serialized behind any refresh in progress.

use std::sync::mpsc::{Receiver, Sender};

use crate::google::{CreateTask, Error, ListPatch, MoveTask, TaskPatch, TasksApi};
use crate::model::{Task, TaskList};

/// A change to make in Google.
pub(super) enum Op {
    Patch {
        list: String,
        task: String,
        patch: TaskPatch,
        etag: String,
    },
    Create {
        list: String,
        task: CreateTask,
        parent: Option<String>,
    },
    Delete {
        list: String,
        task: String,
        etag: String,
    },
    /// Move a task and its subtasks into another list, then reload both lists.
    MoveToList {
        from: String,
        to: String,
        ids: Vec<String>,
    },
    /// Reparent or reorder within a list. Google renumbers siblings when a
    /// task moves, so the list is reloaded to learn every new position.
    Place {
        list: String,
        task: String,
        parent: Option<String>,
        previous: Option<String>,
    },
    PatchList {
        list: String,
        title: String,
        etag: String,
    },
    CreateList {
        title: String,
    },
    DeleteList {
        list: String,
    },
    /// Clear completed tasks in a list, then reload it.
    ClearCompleted {
        list: String,
    },
}

/// What Google says is true after an operation.
#[derive(Default)]
pub(super) struct Outcome {
    /// Lists Google sent back in full.
    pub replaced: Vec<(String, Vec<Task>)>,
    /// Single tasks Google returned.
    pub upserted: Vec<(String, Task)>,
    /// Task lists Google returned (rename / create).
    pub upserted_lists: Vec<TaskList>,
    /// Part of the operation landed, but the board needs a refresh.
    pub warnings: Vec<Warning>,
}

pub(super) enum Warning {
    PartialMove {
        moved: usize,
        failed: String,
        error: Error,
    },
    ReloadFailed(Error),
}

pub(super) enum Cmd {
    Refresh(u64),
    Run { id: u64, op: Op },
}

pub(super) enum Msg {
    Lists(u64, Result<Vec<TaskList>, Error>),
    Tasks(u64, String, Result<Vec<Task>, Error>),
    Finished {
        id: u64,
        result: Result<Outcome, Error>,
    },
}

pub(super) fn worker<A: TasksApi>(mut client: A, commands: Receiver<Cmd>, output: Sender<Msg>) {
    for command in commands {
        match command {
            Cmd::Run { id, op } => {
                let result = execute(&mut client, op);
                if output.send(Msg::Finished { id, result }).is_err() {
                    return;
                }
            }
            Cmd::Refresh(generation) => {
                let lists = client.tasklists();
                let ids: Vec<String> = lists.iter().flatten().map(|list| list.id.clone()).collect();
                if output.send(Msg::Lists(generation, lists)).is_err() {
                    return;
                }
                for id in ids {
                    let tasks = client.tasks(&id);
                    if output.send(Msg::Tasks(generation, id, tasks)).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

fn execute<A: TasksApi>(client: &mut A, op: Op) -> Result<Outcome, Error> {
    match op {
        Op::Patch {
            list,
            task,
            patch,
            etag,
        } => {
            let task = client.patch(&list, &task, &patch, Some(&etag))?;
            Ok(Outcome {
                upserted: vec![(list, task)],
                ..Outcome::default()
            })
        }
        Op::Create { list, task, parent } => {
            let task = client.create(&list, &task, parent.as_deref())?;
            Ok(Outcome {
                upserted: vec![(list, task)],
                ..Outcome::default()
            })
        }
        Op::Delete { list, task, etag } => {
            client.delete(&list, &task, Some(&etag))?;
            Ok(Outcome::default())
        }
        Op::MoveToList { from, to, ids } => {
            let tree = crate::google::move_tree(client, &from, &to, &ids)?;
            let mut warnings = Vec::new();
            if let Some((failed, error)) = tree.partial {
                warnings.push(Warning::PartialMove {
                    moved: tree.moved,
                    failed,
                    error,
                });
            }
            // Tasks have moved, so a failed reload is a warning, not a failure.
            let mut replaced = Vec::new();
            for list in [from, to] {
                match client.tasks(&list) {
                    Ok(tasks) => replaced.push((list, tasks)),
                    Err(error) => warnings.push(Warning::ReloadFailed(error)),
                }
            }
            Ok(Outcome {
                replaced,
                upserted: Vec::new(),
                upserted_lists: Vec::new(),
                warnings,
            })
        }
        Op::Place {
            list,
            task,
            parent,
            previous,
        } => {
            let moved = client.move_task(
                &list,
                &task,
                &MoveTask {
                    destination_list: None,
                    parent,
                    previous,
                },
            )?;
            Ok(match client.tasks(&list) {
                Ok(tasks) => Outcome {
                    replaced: vec![(list, tasks)],
                    ..Outcome::default()
                },
                Err(error) => Outcome {
                    upserted: vec![(list, moved)],
                    warnings: vec![Warning::ReloadFailed(error)],
                    ..Outcome::default()
                },
            })
        }
        Op::PatchList { list, title, etag } => {
            let list = client.patch_list(&list, &ListPatch { title }, Some(&etag))?;
            Ok(Outcome {
                upserted_lists: vec![list],
                ..Outcome::default()
            })
        }
        Op::CreateList { title } => {
            let list = client.create_list(&title)?;
            Ok(Outcome {
                upserted_lists: vec![list],
                ..Outcome::default()
            })
        }
        Op::DeleteList { list } => {
            client.delete_list(&list)?;
            Ok(Outcome::default())
        }
        Op::ClearCompleted { list } => {
            client.clear_completed(&list)?;
            Ok(match client.tasks(&list) {
                Ok(tasks) => Outcome {
                    replaced: vec![(list, tasks)],
                    ..Outcome::default()
                },
                Err(error) => Outcome {
                    warnings: vec![Warning::ReloadFailed(error)],
                    ..Outcome::default()
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::{ListPatch, Result};
    use std::sync::{mpsc, Arc, Mutex};

    #[derive(Clone, Default)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        fail_move: Option<String>,
        fail_tasks: bool,
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
            if self.fail_tasks {
                return Err(Error::Timeout);
            }
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

        fn clear_completed(&mut self, list_id: &str) -> Result<()> {
            self.record(format!("clear:{list_id}"));
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

    fn run(fake: Fake, op: Op) -> (u64, std::result::Result<Outcome, Error>) {
        let (commands, input) = mpsc::channel();
        let (output, messages) = mpsc::channel();
        commands.send(Cmd::Run { id: 42, op }).unwrap();
        drop(commands);
        worker(fake, input, output);
        match messages.recv().unwrap() {
            Msg::Finished { id, result } => (id, result),
            _ => panic!("expected a finished operation"),
        }
    }

    fn list_ids(outcome: &Outcome) -> Vec<&str> {
        outcome
            .replaced
            .iter()
            .map(|(list, _)| list.as_str())
            .collect()
    }

    #[test]
    fn replies_carry_the_operation_id_and_googles_task() {
        let fake = Fake::default();
        let (id, result) = run(
            fake.clone(),
            Op::Patch {
                list: "list".into(),
                task: "task".into(),
                patch: TaskPatch {
                    title: Some("changed".into()),
                    ..Default::default()
                },
                etag: "\"v1\"".into(),
            },
        );

        assert_eq!(id, 42);
        assert_eq!(fake.calls(), ["patch:list:task:\"v1\""]);
        let outcome = result.unwrap();
        assert_eq!(outcome.upserted.len(), 1);
        assert_eq!(outcome.upserted[0].0, "list");
        assert_eq!(outcome.upserted[0].1.id, "task");
    }

    #[test]
    fn move_executes_parent_then_ordered_children_and_reloads_both_lists() {
        let fake = Fake::default();
        let (_, result) = run(
            fake.clone(),
            Op::MoveToList {
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
        let outcome = result.unwrap();
        assert_eq!(list_ids(&outcome), ["source", "destination"]);
        assert!(outcome.warnings.is_empty());
    }

    #[test]
    fn partial_move_stops_at_failure_and_still_reloads_both_lists() {
        let fake = Fake {
            fail_move: Some("first".into()),
            ..Default::default()
        };
        let (_, result) = run(
            fake.clone(),
            Op::MoveToList {
                from: "source".into(),
                to: "destination".into(),
                ids: vec!["parent".into(), "first".into(), "never".into()],
            },
        );

        assert_eq!(
            fake.calls(),
            [
                "move:source:parent:destination::",
                "move:source:first:destination:parent:",
                "tasks:source",
                "tasks:destination",
            ]
        );
        let outcome = result.unwrap();
        assert_eq!(list_ids(&outcome), ["source", "destination"]);
        let [Warning::PartialMove {
            moved,
            failed,
            error,
        }] = outcome.warnings.as_slice()
        else {
            panic!("expected a partial move");
        };
        assert_eq!((*moved, failed.as_str()), (1, "first"));
        assert!(error.to_string().contains("first failed"));
    }

    #[test]
    fn unparent_places_after_the_former_parent_then_reloads_the_list() {
        let fake = Fake::default();
        let (_, result) = run(
            fake.clone(),
            Op::Place {
                list: "list".into(),
                task: "child".into(),
                parent: None,
                previous: Some("former-parent".into()),
            },
        );

        assert_eq!(
            fake.calls(),
            ["move:list:child:::former-parent", "tasks:list"]
        );
        assert_eq!(list_ids(&result.unwrap()), ["list"]);
    }

    #[test]
    fn placing_under_a_parent_moves_then_reloads_the_list() {
        let fake = Fake::default();
        let (_, result) = run(
            fake.clone(),
            Op::Place {
                list: "list".into(),
                task: "two".into(),
                parent: Some("parent".into()),
                previous: None,
            },
        );

        assert_eq!(fake.calls(), ["move:list:two::parent:", "tasks:list"]);
        assert_eq!(list_ids(&result.unwrap()), ["list"]);
    }

    #[test]
    fn clearing_completed_clears_then_reloads_the_list() {
        let fake = Fake::default();
        let (_, result) = run(
            fake.clone(),
            Op::ClearCompleted {
                list: "list".into(),
            },
        );

        assert_eq!(fake.calls(), ["clear:list", "tasks:list"]);
        assert_eq!(list_ids(&result.unwrap()), ["list"]);
    }

    #[test]
    fn a_failed_reload_after_clear_keeps_the_clear_and_warns() {
        let fake = Fake {
            fail_tasks: true,
            ..Fake::default()
        };
        let (_, result) = run(
            fake.clone(),
            Op::ClearCompleted {
                list: "list".into(),
            },
        );

        assert_eq!(fake.calls(), ["clear:list", "tasks:list"]);
        let outcome = result.unwrap();
        assert!(outcome.replaced.is_empty());
        assert!(matches!(
            outcome.warnings.as_slice(),
            [Warning::ReloadFailed(Error::Timeout)]
        ));
    }

    #[test]
    fn a_failed_place_skips_the_reload() {
        let fake = Fake {
            fail_move: Some("two".into()),
            ..Fake::default()
        };
        let (_, result) = run(
            fake.clone(),
            Op::Place {
                list: "list".into(),
                task: "two".into(),
                parent: None,
                previous: Some("one".into()),
            },
        );

        assert_eq!(fake.calls(), ["move:list:two:::one"]);
        assert!(result.is_err());
    }

    #[test]
    fn a_failed_reload_keeps_the_moved_task_and_warns() {
        let fake = Fake {
            fail_tasks: true,
            ..Fake::default()
        };
        let (_, result) = run(
            fake,
            Op::Place {
                list: "list".into(),
                task: "two".into(),
                parent: None,
                previous: None,
            },
        );

        let outcome = result.unwrap();
        assert!(outcome.replaced.is_empty());
        assert_eq!(outcome.upserted[0].1.id, "two");
        assert!(matches!(
            outcome.warnings.as_slice(),
            [Warning::ReloadFailed(Error::Timeout)]
        ));
    }

    #[test]
    fn a_failed_reload_after_a_move_to_list_warns_instead_of_failing() {
        let fake = Fake {
            fail_tasks: true,
            ..Fake::default()
        };
        let (_, result) = run(
            fake.clone(),
            Op::MoveToList {
                from: "source".into(),
                to: "destination".into(),
                ids: vec!["parent".into(), "child".into()],
            },
        );

        assert_eq!(
            fake.calls(),
            [
                "move:source:parent:destination::",
                "move:source:child:destination:parent:",
                "tasks:source",
                "tasks:destination",
            ]
        );
        let outcome = result.unwrap();
        assert!(outcome.replaced.is_empty());
        assert!(matches!(
            outcome.warnings.as_slice(),
            [
                Warning::ReloadFailed(Error::Timeout),
                Warning::ReloadFailed(Error::Timeout)
            ]
        ));
    }

    #[test]
    fn a_partial_move_keeps_its_detail_when_the_reload_also_fails() {
        let fake = Fake {
            fail_move: Some("child".into()),
            fail_tasks: true,
            ..Fake::default()
        };
        let (_, result) = run(
            fake,
            Op::MoveToList {
                from: "source".into(),
                to: "destination".into(),
                ids: vec!["parent".into(), "child".into()],
            },
        );

        assert!(matches!(
            result.unwrap().warnings.as_slice(),
            [
                Warning::PartialMove { moved: 1, .. },
                Warning::ReloadFailed(_),
                Warning::ReloadFailed(_)
            ]
        ));
    }

    #[test]
    fn a_move_to_list_that_moves_nothing_fails_without_reloading() {
        let fake = Fake {
            fail_move: Some("parent".into()),
            ..Fake::default()
        };
        let (_, result) = run(
            fake.clone(),
            Op::MoveToList {
                from: "source".into(),
                to: "destination".into(),
                ids: vec!["parent".into(), "child".into()],
            },
        );

        assert_eq!(fake.calls(), ["move:source:parent:destination::"]);
        assert!(result.is_err());
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
