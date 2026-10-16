//! The one in-flight mutation: sending it, matching its reply, and rolling
//! back when it fails.

use crate::board::previous_sibling;
use crate::google::TaskPatch;
use crate::model::Task;

use super::app::{App, Interaction, QuitState, Shelf};
use super::editor::{Edit, EditTarget, FieldId};
use super::sync::SyncState;
use super::worker::{Cmd, Msg};

pub(super) enum MutationContext {
    Create(Edit),
    Patch {
        list_id: String,
        task_id: String,
        before: Box<Task>,
    },
    Delete {
        list_id: String,
        task_id: String,
        removed: Vec<Task>,
    },
    Move {
        from: String,
        to: String,
        ids: Vec<String>,
        source_before: Vec<Task>,
        destination_before: Vec<Task>,
    },
    Reparent {
        list_id: String,
        task_id: String,
        before: Box<Task>,
    },
    /// A reorder. Local positions stand in until Google's reloaded list
    /// replaces them; `before` is the list ahead of every local change, so
    /// a failure also drops `queued` presses.
    Reorder {
        list_id: String,
        task_id: String,
        before: Vec<Task>,
        /// The task was moved again locally while this reorder was saving.
        queued: bool,
    },
}

pub(super) struct PendingOperation {
    pub(super) id: u64,
    pub(super) context: MutationContext,
}

impl App {
    pub(super) fn mutation_finished(&mut self, operation: u64) -> Option<MutationContext> {
        if self.pending_operation.as_ref().map(|pending| pending.id) != Some(operation) {
            return None;
        }
        let context = self.pending_operation.take().map(|pending| pending.context);
        if self.quit == QuitState::Waiting {
            self.quit = QuitState::Drained;
        }
        context
    }

    pub(super) fn send_mutation(
        &mut self,
        context: MutationContext,
        build: impl FnOnce(u64) -> Cmd,
    ) -> bool {
        if self.pending_operation.is_some() {
            self.error = Some("wait for the current save to finish".into());
            return false;
        }
        self.next_operation = self.next_operation.wrapping_add(1);
        let operation = self.next_operation;
        let cmd = build(operation);
        match self.cmds.send(cmd) {
            Ok(()) => {
                self.pending_operation = Some(PendingOperation {
                    id: operation,
                    context,
                });
                true
            }
            Err(_) => {
                self.rollback(context);
                self.sync = SyncState::Failed;
                self.error = Some("network worker stopped; change was not sent".into());
                false
            }
        }
    }

    pub(super) fn rollback(&mut self, context: MutationContext) {
        match context {
            MutationContext::Create(edit) => {
                self.interaction = Interaction::Editing(edit);
            }
            MutationContext::Patch {
                list_id, before, ..
            }
            | MutationContext::Reparent {
                list_id, before, ..
            } => {
                self.board.upsert_task(&list_id, *before);
                self.rebuild_column(&list_id);
            }
            MutationContext::Reorder {
                list_id, before, ..
            } => {
                self.board.replace_tasks(&list_id, before);
                self.rebuild_column(&list_id);
            }
            MutationContext::Delete {
                list_id, removed, ..
            } => {
                self.board.restore(&list_id, removed);
                self.rebuild_column(&list_id);
            }
            MutationContext::Move {
                from,
                to,
                source_before,
                destination_before,
                ..
            } => {
                self.board.replace_tasks(&from, source_before);
                self.board.replace_tasks(&to, destination_before);
                self.rebuild_column(&from);
                self.rebuild_column(&to);
            }
        }
    }

    pub(super) fn worker_disconnected(&mut self) {
        self.sync = SyncState::Failed;
        self.pending_operation = None;
        self.error = Some("network worker stopped".into());
        if self.quit == QuitState::Waiting {
            self.quit = QuitState::Drained;
        }
    }

    pub(super) fn send_patch(
        &mut self,
        list_id: String,
        task_id: String,
        patch: TaskPatch,
        before: Task,
    ) {
        let etag = before.etag.clone();
        self.send_mutation(
            MutationContext::Patch {
                list_id: list_id.clone(),
                task_id: task_id.clone(),
                before: Box::new(before),
            },
            |operation| Cmd::Patch {
                operation,
                list_id,
                task_id,
                patch,
                etag,
            },
        );
    }

    /// Apply a worker reply to the pending operation it answers.
    pub(super) fn mutation_replied(&mut self, msg: Msg) {
        match msg {
            Msg::Lists(..) | Msg::Tasks(..) => {}
            Msg::Created {
                operation,
                list_id,
                selection: selection_at_send,
                result,
            } => {
                let Some(context) = self.mutation_finished(operation) else {
                    return;
                };
                let MutationContext::Create(draft) = context else {
                    self.recover("create response did not match the pending operation".into());
                    return;
                };
                let EditTarget::New {
                    list_id: draft_list,
                    selection: draft_selection,
                    ..
                } = &draft.target
                else {
                    self.recover("create response did not match its draft".into());
                    return;
                };
                if draft_list != &list_id || draft_selection != &selection_at_send {
                    self.recover("create response did not match its draft".into());
                    return;
                }
                match result {
                    Ok(task) => {
                        let focused_list = self.columns.get(self.focus).map(|c| c.list.id.as_str());
                        let may_follow =
                            self.edit().is_none() && focused_list == Some(list_id.as_str());
                        let Some(col_index) =
                            self.columns.iter().position(|c| c.list.id == list_id)
                        else {
                            self.recover("create succeeded but its list disappeared".into());
                            return;
                        };
                        let id = task.id.clone();
                        let selected = self.columns[col_index].selected_id.clone();
                        let follow_created = may_follow && selected == selection_at_send;
                        if !self.board.upsert_task(&list_id, task) {
                            self.recover("create succeeded but its list disappeared".into());
                            return;
                        }
                        self.rebuild_column(&list_id);
                        if follow_created {
                            self.columns[col_index].select_id(Some(id));
                        }
                    }
                    Err(e) => {
                        if let Some(index) = self
                            .columns
                            .iter()
                            .position(|column| column.list.id == *draft_list)
                        {
                            self.focus = index;
                        }
                        self.shelf = Shelf::Open(FieldId::Title);
                        self.interaction = Interaction::Editing(draft);
                        self.recover(format!("create failed: {e}"));
                    }
                }
            }
            Msg::Deleted {
                operation,
                list_id,
                task_id,
                result,
            } => {
                let Some(context) = self.mutation_finished(operation) else {
                    return;
                };
                if !matches!(
                    &context,
                    MutationContext::Delete {
                        list_id: expected_list,
                        task_id: expected_task,
                        ..
                    } if expected_list == &list_id && expected_task == &task_id
                ) {
                    self.recover("delete response did not match the pending operation".into());
                    return;
                }
                match result {
                    Ok(()) => {}
                    Err(e) => {
                        // The task is still there, so put the board back.
                        self.rollback(context);
                        self.recover(format!("delete failed for {list_id}/{task_id}: {e}"));
                    }
                }
            }
            Msg::Moved {
                operation,
                from,
                to,
                ids,
                result,
            } => {
                let Some(context) = self.mutation_finished(operation) else {
                    return;
                };
                if !matches!(
                    &context,
                    MutationContext::Move {
                        from: expected_from,
                        to: expected_to,
                        ids: expected_ids,
                        ..
                    } if expected_from == &from && expected_to == &to && expected_ids == &ids
                ) {
                    self.recover("move response did not match the pending operation".into());
                    return;
                }
                match (result.source, result.destination) {
                    (Ok(source), Ok(destination)) => {
                        self.board.replace_tasks(&from, source);
                        self.board.replace_tasks(&to, destination);
                        self.rebuild_column(&from);
                        self.rebuild_column(&to);
                        if let Some(failure) = result.failure {
                            self.recover(format!(
                                "move partially completed ({} moved); {} failed: {}",
                                result.moved.len(),
                                failure.task_id,
                                failure.error
                            ));
                        }
                    }
                    (source, destination) => {
                        self.rollback(context);
                        self.recover(format!(
                            "move reconciliation failed: source {}; destination {}",
                            source.err().unwrap_or_else(|| "ok".into()),
                            destination.err().unwrap_or_else(|| "ok".into())
                        ));
                    }
                }
            }
            Msg::Reparented {
                operation,
                list_id,
                task_id,
                parent,
                result,
            } => {
                let Some(context) = self.mutation_finished(operation) else {
                    return;
                };
                if !matches!(
                    &context,
                    MutationContext::Reparent {
                        list_id: expected_list,
                        task_id: expected_task,
                        ..
                    } if expected_list == &list_id && expected_task == &task_id
                ) {
                    self.recover("reparent response did not match the pending operation".into());
                    return;
                }
                match result {
                    Ok(task) if task.id == task_id && task.parent == parent => {
                        self.board.upsert_task(&list_id, task);
                        self.rebuild_column(&list_id);
                    }
                    Ok(_) => {
                        self.rollback(context);
                        self.recover("Google returned a mismatched reparented task".into());
                    }
                    Err(error) => {
                        self.rollback(context);
                        self.recover(format!("reparent failed: {error}"));
                    }
                }
            }
            Msg::Reordered {
                operation,
                list_id,
                task_id,
                parent,
                result,
                tasks,
            } => {
                let Some(context) = self.mutation_finished(operation) else {
                    return;
                };
                let queued = match &context {
                    MutationContext::Reorder {
                        list_id: expected_list,
                        task_id: expected_task,
                        queued,
                        ..
                    } if expected_list == &list_id && expected_task == &task_id => *queued,
                    _ => {
                        self.recover("reorder response did not match the pending operation".into());
                        return;
                    }
                };
                match (result, tasks) {
                    (Ok(task), Some(Ok(tasks))) if task.id == task_id && task.parent == parent => {
                        let shown = queued
                            .then(|| previous_sibling(&self.board.rows(&list_id), &task_id).ok())
                            .flatten();
                        self.board.replace_tasks(&list_id, tasks);
                        self.rebuild_column(&list_id);
                        if let Some(previous) = shown {
                            self.send_queued_reorder(list_id, task_id, previous);
                        }
                    }
                    (Ok(task), Some(Err(error))) if task.id == task_id && task.parent == parent => {
                        // Saved, but the siblings' new positions are unknown.
                        self.board.upsert_task(&list_id, task);
                        self.rebuild_column(&list_id);
                        self.recover(format!("reorder saved; reloading the list failed: {error}"));
                    }
                    (Ok(_), _) => {
                        self.rollback(context);
                        self.recover("Google returned a mismatched reordered task".into());
                    }
                    (Err(error), _) => {
                        self.rollback(context);
                        self.recover(format!("reorder failed: {error}"));
                    }
                }
            }
            Msg::Updated {
                operation,
                list_id,
                task_id,
                result,
            } => {
                let Some(context) = self.mutation_finished(operation) else {
                    return;
                };
                if !matches!(
                    &context,
                    MutationContext::Patch {
                        list_id: expected_list,
                        task_id: expected_task,
                        ..
                    } if expected_list == &list_id && expected_task == &task_id
                ) {
                    self.recover("save response did not match the pending operation".into());
                    return;
                }
                match result {
                    Ok(task) => {
                        if !self.columns.iter().any(|column| column.list.id == list_id) {
                            self.recover("save succeeded but its list disappeared".into());
                            return;
                        }
                        if task.id != task_id || self.board.task(&list_id, &task_id).is_none() {
                            self.recover("save succeeded but its task disappeared".into());
                            return;
                        }
                        self.board.upsert_task(&list_id, task);
                        self.rebuild_column(&list_id);
                    }
                    Err(e) => {
                        // The optimistic title may be wrong now, so resync.
                        self.rollback(context);
                        self.recover(format!("save failed: {e}"));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::*;

    #[test]
    fn a_failed_mutation_enters_read_only_recovery() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        let removed = app.board.remove_tree("a", "a-cached");
        app.rebuild_column("a");
        let operation = pending(
            &mut app,
            MutationContext::Delete {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                removed,
            },
        );

        app.on_msg(Msg::Deleted {
            operation,
            list_id: "a".into(),
            task_id: "a-cached".into(),
            result: Err("offline".into()),
        });

        assert_ne!(app.sync, SyncState::Ready);
        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app.pending_operation.is_none());
        assert!(app.board.task("a", "a-cached").is_some());
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("a-cached"));
        assert!(app
            .error
            .as_deref()
            .is_some_and(|e| e.contains("delete failed")));
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }

    #[test]
    fn a_failed_create_uses_the_same_read_only_recovery() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        let operation = pending(
            &mut app,
            MutationContext::Create(new_edit("a", Some("a-cached"), "draft")),
        );

        app.on_msg(Msg::Created {
            operation,
            list_id: "a".into(),
            selection: Some("a-cached".into()),
            result: Err("offline".into()),
        });

        assert_ne!(app.sync, SyncState::Ready);
        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|e| e.contains("create failed")));
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }

    #[test]
    fn a_disconnected_worker_does_not_create_a_pending_save() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        drop(cmds);
        app.sync = SyncState::Ready;

        app.send_patch(
            "a".into(),
            "a-cached".into(),
            TaskPatch {
                title: Some("changed".into()),
                ..Default::default()
            },
            task("a-cached", None, "1", false),
        );

        assert!(app.pending_operation.is_none());
        assert_eq!(app.sync, SyncState::Failed);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|e| e.contains("worker stopped")));
    }

    #[test]
    fn only_one_mutation_can_be_in_flight() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;

        app.send_patch(
            "a".into(),
            "one".into(),
            TaskPatch {
                title: Some("one".into()),
                ..Default::default()
            },
            task("one", None, "1", false),
        );
        app.send_patch(
            "a".into(),
            "two".into(),
            TaskPatch {
                title: Some("two".into()),
                ..Default::default()
            },
            task("two", None, "2", false),
        );

        assert!(app.pending_operation.is_some());
        let patches = cmds
            .try_iter()
            .filter(|cmd| matches!(cmd, Cmd::Patch { .. }))
            .count();
        assert_eq!(patches, 1);
    }

    #[test]
    fn a_failed_patch_rolls_back_the_canonical_task() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;
        let before = app.board.task("a", "a-cached").unwrap().clone();
        app.board.task_mut("a", "a-cached").unwrap().title = "optimistic".into();
        app.rebuild_column("a");
        let operation = pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                before: Box::new(before),
            },
        );

        app.on_msg(Msg::Updated {
            operation,
            list_id: "a".into(),
            task_id: "a-cached".into(),
            result: Err("offline".into()),
        });

        assert_ne!(app.board.task("a", "a-cached").unwrap().title, "optimistic");
        assert_eq!(
            app.columns[0].rows[0].task.title,
            app.board.task("a", "a-cached").unwrap().title
        );
    }

    #[test]
    fn a_response_for_another_operation_is_ignored() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;
        pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                before: Box::new(task("a-cached", None, "1", false)),
            },
        );

        app.on_msg(Msg::Updated {
            operation: 99,
            list_id: "a".into(),
            task_id: "a-cached".into(),
            result: Ok(Task {
                title: "wrong response".into(),
                ..task("a-cached", None, "1", false)
            }),
        });

        assert_eq!(
            app.pending_operation.as_ref().map(|pending| pending.id),
            Some(1)
        );
        assert_ne!(app.columns[0].rows[0].task.title, "wrong response");
    }

    #[test]
    fn a_failed_create_restores_the_exact_draft() {
        let (mut app, cmds) = app_with_lists(&["a", "b"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        app.start_create(false);
        let edit = app.edit_mut().unwrap();
        edit.buf = "keep this draft".into();
        edit.cursor = edit.buf.len();

        app.commit_edit();
        let _ = cmds.recv().unwrap(); // create command
        app.focus = 1; // navigation remains available while the create is pending
        let operation = app.pending_operation.as_ref().unwrap().id;
        app.on_msg(Msg::Created {
            operation,
            list_id: "a".into(),
            selection: Some("a-cached".into()),
            result: Err("offline".into()),
        });

        assert_eq!(app.edit().map(|e| e.buf.as_str()), Some("keep this draft"));
        assert!(app.is_creating());
        assert_eq!(app.columns[app.focus].list.id, "a");
        let _ = cmds.recv().unwrap(); // recovery refresh
        app.commit_edit();
        assert!(
            cmds.try_recv().is_err(),
            "draft must wait for recovery sync"
        );
        assert_eq!(app.edit().map(|e| e.buf.as_str()), Some("keep this draft"));
    }

    #[test]
    fn a_delayed_update_does_not_hide_another_tasks_editor() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        set_tasks(
            &mut app,
            "a",
            vec![
                task("a-one", None, "1", false),
                task("a-two", None, "2", false),
            ],
            "a-two",
        );
        app.interaction = Interaction::Editing(Edit {
            target: EditTarget::Existing {
                list_id: "a".into(),
                task_id: "a-two".into(),
                field: FieldId::Title,
            },
            buf: "editing two".into(),
            cursor: 11,
        });
        let operation = pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-one".into(),
                before: Box::new(task("a-one", None, "1", false)),
            },
        );

        app.on_msg(Msg::Updated {
            operation,
            list_id: "a".into(),
            task_id: "a-one".into(),
            result: Ok(Task {
                title: "updated one".into(),
                ..task("a-one", None, "1", false)
            }),
        });

        assert_eq!(
            app.selected_task().map(|r| r.task.id.as_str()),
            Some("a-two")
        );
        assert_eq!(
            app.edit()
                .and_then(Edit::existing)
                .map(|(_, task_id, _)| task_id),
            Some("a-two")
        );
    }

    #[test]
    fn a_delayed_create_does_not_hide_another_tasks_editor() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        set_tasks(
            &mut app,
            "a",
            vec![
                task("a-one", None, "1", false),
                task("a-two", None, "2", false),
            ],
            "a-two",
        );
        app.interaction = Interaction::Editing(Edit {
            target: EditTarget::Existing {
                list_id: "a".into(),
                task_id: "a-two".into(),
                field: FieldId::Title,
            },
            buf: "editing two".into(),
            cursor: 11,
        });
        let operation = pending(
            &mut app,
            MutationContext::Create(new_edit("a", Some("a-one"), "new task")),
        );

        app.on_msg(Msg::Created {
            operation,
            list_id: "a".into(),
            selection: Some("a-one".into()),
            result: Ok(Task {
                id: "new".into(),
                title: "new task".into(),
                status: Status::NeedsAction,
                position: "0".into(),
                ..Default::default()
            }),
        });

        assert_eq!(
            app.selected_task().map(|r| r.task.id.as_str()),
            Some("a-two")
        );
        assert_eq!(
            app.edit()
                .and_then(Edit::existing)
                .map(|(_, task_id, _)| task_id),
            Some("a-two")
        );
    }

    #[test]
    fn partial_move_applies_authoritative_lists_and_reports_the_failed_id() {
        let (mut app, cmds) = app_with_lists(&["a", "b"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        let parent = task("parent", None, "1", false);
        let child = task("child", Some("parent"), "2", false);
        let there = task("there", None, "1", false);
        set_tasks(&mut app, "a", vec![parent.clone(), child.clone()], "parent");
        set_tasks(&mut app, "b", vec![there.clone()], "there");
        let operation = pending(
            &mut app,
            MutationContext::Move {
                from: "a".into(),
                to: "b".into(),
                ids: vec!["parent".into(), "child".into()],
                source_before: vec![parent.clone(), child.clone()],
                destination_before: vec![there.clone()],
            },
        );
        let moved_parent = Task {
            position: "9".into(),
            ..parent
        };

        app.on_msg(Msg::Moved {
            operation,
            from: "a".into(),
            to: "b".into(),
            ids: vec!["parent".into(), "child".into()],
            result: MoveOutcome {
                moved: vec![moved_parent.clone()],
                failure: Some(MoveFailure {
                    task_id: "child".into(),
                    error: "cannot move assigned task".into(),
                }),
                source: Ok(vec![child]),
                destination: Ok(vec![there, moved_parent]),
            },
        });

        assert!(app.board.task("a", "parent").is_none());
        assert_eq!(
            app.board
                .task("b", "parent")
                .map(|task| task.position.as_str()),
            Some("9")
        );
        assert!(app.error.as_deref().is_some_and(|error| {
            error.contains("partially completed") && error.contains("child")
        }));
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }
}
