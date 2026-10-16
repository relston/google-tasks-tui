//! The one in-flight mutation. `submit` copies the affected lists, applies
//! the change on screen, and sends the operation; `finish` applies Google's
//! answer, or puts the copied lists back and recovers.

use crate::board::{previous_sibling, Board};
use crate::google::Error;
use crate::model::Task;

use super::app::{App, Interaction, QuitState, Shelf};
use super::editor::{Edit, EditTarget, FieldId};
use super::sync::SyncState;
use super::worker::{Cmd, Op, Outcome, Warning};

/// A reorder in flight, so more presses on the same task can wait for it.
pub(super) struct Reorder {
    pub(super) list_id: String,
    pub(super) task_id: String,
    /// The task was moved again locally while this reorder was saving.
    pub(super) queued: bool,
}

pub(super) struct PendingOperation {
    pub(super) id: u64,
    /// Names the operation in messages, as in "save failed: …".
    label: &'static str,
    /// The affected lists as they were before the change on screen.
    before: Vec<(String, Vec<Task>)>,
    /// A new-task draft, reopened if Google refuses it.
    draft: Option<Edit>,
    pub(super) reorder: Option<Reorder>,
}

impl PendingOperation {
    #[cfg(test)]
    pub(super) fn for_test(id: u64) -> Self {
        Self {
            id,
            label: "save",
            before: Vec::new(),
            draft: None,
            reorder: None,
        }
    }
}

impl App {
    /// Copy `lists`, apply `change` to the board, and send `op`.
    pub(super) fn submit(
        &mut self,
        label: &'static str,
        op: Op,
        lists: &[&str],
        change: impl FnOnce(&mut Board),
    ) -> bool {
        if self.pending_operation.is_some() {
            self.error = Some("wait for the current save to finish".into());
            return false;
        }
        let before = lists
            .iter()
            .map(|list_id| (list_id.to_string(), self.board.tasks(list_id)))
            .collect();
        change(&mut self.board);
        for list_id in lists {
            self.rebuild_column(list_id);
        }
        self.send(
            PendingOperation {
                id: 0,
                label,
                before,
                draft: None,
                reorder: None,
            },
            op,
        )
    }

    /// Send a new task. Nothing changes on screen until Google answers.
    pub(super) fn submit_create(&mut self, draft: Edit, op: Op) -> bool {
        if self.pending_operation.is_some() {
            self.error = Some("wait for the current save to finish".into());
            self.interaction = Interaction::Editing(draft);
            return false;
        }
        self.send(
            PendingOperation {
                id: 0,
                label: "create",
                before: Vec::new(),
                draft: Some(draft),
                reorder: None,
            },
            op,
        )
    }

    fn send(&mut self, mut pending: PendingOperation, op: Op) -> bool {
        self.next_operation = self.next_operation.wrapping_add(1);
        pending.id = self.next_operation;
        match self.cmds.send(Cmd::Run { id: pending.id, op }) {
            Ok(()) => {
                self.pending_operation = Some(pending);
                true
            }
            Err(_) => {
                self.restore(pending);
                self.sync = SyncState::Failed;
                self.error = Some("network worker stopped; change was not sent".into());
                false
            }
        }
    }

    /// Apply a worker reply to the pending operation it answers.
    pub(super) fn finish(&mut self, id: u64, result: Result<Outcome, Error>) {
        if self.pending_operation.as_ref().map(|pending| pending.id) != Some(id) {
            return;
        }
        let Some(pending) = self.pending_operation.take() else {
            return;
        };
        if self.quit == QuitState::Waiting {
            self.quit = QuitState::Drained;
        }
        match result {
            Ok(outcome) => self.apply(pending, outcome),
            Err(error) => {
                let label = pending.label;
                self.restore(pending);
                self.recover(format!("{label} failed: {error}"));
            }
        }
    }

    fn apply(&mut self, pending: PendingOperation, outcome: Outcome) {
        // Where the board showed a queued reorder, read before Google's list
        // replaces the local positions.
        let queued = pending
            .reorder
            .filter(|reorder| reorder.queued)
            .and_then(|reorder| {
                let rows = self.board.rows(&reorder.list_id);
                let previous = previous_sibling(&rows, &reorder.task_id).ok()?;
                Some((reorder.list_id, reorder.task_id, previous))
            });
        for (list_id, tasks) in outcome.replaced {
            self.board.replace_tasks(&list_id, tasks);
            self.rebuild_column(&list_id);
        }
        for (list_id, task) in outcome.upserted {
            let task_id = task.id.clone();
            if !self.board.upsert_task(&list_id, task) {
                self.recover(format!(
                    "{} succeeded but its list disappeared",
                    pending.label
                ));
                return;
            }
            self.rebuild_column(&list_id);
            if let Some(draft) = &pending.draft {
                self.follow_created(draft, &list_id, task_id);
            }
        }
        match outcome.warning {
            Some(Warning::PartialMove {
                moved,
                failed,
                error,
            }) => self.recover(format!(
                "{} partially completed ({moved} moved); {failed} failed: {error}",
                pending.label
            )),
            Some(Warning::ReloadFailed(error)) => self.recover(format!(
                "{} saved; reloading the list failed: {error}",
                pending.label
            )),
            None => {
                if let Some((list_id, task_id, previous)) = queued {
                    self.send_queued_reorder(list_id, task_id, previous);
                }
            }
        }
    }

    /// Select a new task if the user is still where they created it.
    fn follow_created(&mut self, draft: &Edit, list_id: &str, task_id: String) {
        let EditTarget::New { selection, .. } = &draft.target else {
            return;
        };
        let focused = self
            .columns
            .get(self.focus)
            .is_some_and(|column| column.list.id == list_id);
        if self.edit().is_some() || !focused {
            return;
        }
        let Some(column) = self
            .columns
            .iter_mut()
            .find(|column| column.list.id == list_id)
        else {
            return;
        };
        if column.selected_id == *selection {
            column.select_id(Some(task_id));
        }
    }

    /// Put back what the operation changed on screen.
    fn restore(&mut self, pending: PendingOperation) {
        for (list_id, tasks) in pending.before {
            self.board.replace_tasks(&list_id, tasks);
            self.rebuild_column(&list_id);
        }
        if let Some(draft) = pending.draft {
            if let EditTarget::New { list_id, .. } = &draft.target {
                if let Some(index) = self
                    .columns
                    .iter()
                    .position(|column| column.list.id == *list_id)
                {
                    self.focus = index;
                }
            }
            self.shelf = Shelf::Open(FieldId::Title);
            self.interaction = Interaction::Editing(draft);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::Error;
    use crate::tui::test_support::*;

    fn patch_title(app: &mut App, title: &str) -> bool {
        let title = title.to_string();
        app.submit(
            "save",
            Op::Patch {
                list: "a".into(),
                task: "a-cached".into(),
                patch: TaskPatch {
                    title: Some(title.clone()),
                    ..Default::default()
                },
                etag: String::new(),
            },
            &["a"],
            |board| board.task_mut("a", "a-cached").unwrap().title = title,
        )
    }

    #[test]
    fn submit_copies_the_lists_applies_the_change_and_sends_the_op() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;

        assert!(patch_title(&mut app, "optimistic"));

        assert_eq!(app.columns[0].rows[0].task.title, "optimistic");
        let Cmd::Run {
            id,
            op: Op::Patch { list, task, .. },
        } = cmds.recv().unwrap()
        else {
            panic!("expected a patch");
        };
        assert_eq!((list.as_str(), task.as_str()), ("a", "a-cached"));
        assert_eq!(app.pending_operation.as_ref().map(|p| p.id), Some(id));
    }

    #[test]
    fn success_applies_googles_lists_and_tasks() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        patch_title(&mut app, "optimistic");
        let Cmd::Run { id, .. } = cmds.recv().unwrap() else {
            panic!("expected a run");
        };

        app.on_msg(Msg::Finished {
            id,
            result: Ok(Outcome {
                replaced: vec![(
                    "a".into(),
                    vec![
                        task("a-cached", None, "0", false),
                        task("other", None, "1", false),
                    ],
                )],
                upserted: vec![(
                    "a".into(),
                    Task {
                        title: "from google".into(),
                        ..task("a-cached", None, "0", false)
                    },
                )],
                warning: None,
            }),
        });

        assert!(app.pending_operation.is_none());
        assert_eq!(app.sync, SyncState::Ready);
        assert_eq!(
            app.board.task("a", "a-cached").unwrap().title,
            "from google"
        );
        assert!(app.board.task("a", "other").is_some());
        assert!(cmds.try_recv().is_err(), "success needs no recovery");
    }

    #[test]
    fn failure_restores_the_copied_lists_and_recovers() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        patch_title(&mut app, "optimistic");
        let Cmd::Run { id, .. } = cmds.recv().unwrap() else {
            panic!("expected a run");
        };

        app.on_msg(Msg::Finished {
            id,
            result: Err(Error::Transport("offline".into())),
        });

        assert!(app.pending_operation.is_none());
        assert_ne!(app.board.task("a", "a-cached").unwrap().title, "optimistic");
        assert_eq!(app.columns[0].rows[0].task.id, "a-cached");
        assert_eq!(app.sync, SyncState::Refreshing);
        assert_eq!(app.error.as_deref(), Some("save failed: offline"));
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }

    #[test]
    fn a_warning_applies_the_outcome_then_recovers() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        app.submit(
            "reorder",
            Op::Place {
                list: "a".into(),
                task: "a-cached".into(),
                parent: None,
                previous: None,
            },
            &["a"],
            |_| {},
        );
        let Cmd::Run { id, .. } = cmds.recv().unwrap() else {
            panic!("expected a run");
        };

        app.on_msg(Msg::Finished {
            id,
            result: Ok(Outcome {
                replaced: Vec::new(),
                upserted: vec![(
                    "a".into(),
                    Task {
                        title: "saved".into(),
                        ..task("a-cached", None, "0", false)
                    },
                )],
                warning: Some(Warning::ReloadFailed(Error::Timeout)),
            }),
        });

        assert_eq!(app.board.task("a", "a-cached").unwrap().title, "saved");
        assert_eq!(app.sync, SyncState::Refreshing);
        assert_eq!(
            app.error.as_deref(),
            Some("reorder saved; reloading the list failed: Google request timed out")
        );
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }

    #[test]
    fn a_failed_delete_restores_the_task_and_enters_recovery() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        app.ask_delete();
        app.confirm_delete();
        assert!(app.board.task("a", "a-cached").is_none());
        let (id, _) = sent(&cmds);

        app.on_msg(refused(id));

        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app.pending_operation.is_none());
        assert!(app.board.task("a", "a-cached").is_some());
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("a-cached"));
        assert_eq!(app.error.as_deref(), Some("delete failed: offline"));
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }

    #[test]
    fn a_failed_create_uses_the_same_read_only_recovery() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        app.start_create(false);
        app.edit_mut().unwrap().buf = "draft".into();
        app.commit_edit();
        let (id, op) = sent(&cmds);
        assert!(matches!(op, Op::Create { .. }));

        app.on_msg(refused(id));

        assert_eq!(app.sync, SyncState::Refreshing);
        assert_eq!(app.error.as_deref(), Some("create failed: offline"));
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }

    #[test]
    fn a_disconnected_worker_does_not_create_a_pending_save() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        drop(cmds);
        app.sync = SyncState::Ready;

        assert!(!patch_title(&mut app, "changed"));

        assert!(app.pending_operation.is_none());
        assert_ne!(app.board.task("a", "a-cached").unwrap().title, "changed");
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

        assert!(patch_title(&mut app, "one"));
        assert!(!patch_title(&mut app, "two"));

        assert_eq!(app.board.task("a", "a-cached").unwrap().title, "one");
        assert_eq!(cmds.try_iter().count(), 1);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("current save")));
    }

    #[test]
    fn a_response_for_another_operation_is_ignored() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        patch_title(&mut app, "optimistic");
        let (id, _) = sent(&cmds);

        app.on_msg(finished(
            id + 1,
            vec![(
                "a",
                vec![Task {
                    title: "wrong response".into(),
                    ..task("a-cached", None, "1", false)
                }],
            )],
        ));

        assert_eq!(app.pending_operation.as_ref().map(|p| p.id), Some(id));
        assert_eq!(app.columns[0].rows[0].task.title, "optimistic");
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
        let (id, _) = sent(&cmds);
        app.focus = 1; // navigation remains available while the create is pending
        app.on_msg(refused(id));

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

    fn editing_two(app: &mut App) {
        set_tasks(
            app,
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
    }

    fn still_editing_two(app: &App) {
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
    fn a_delayed_update_does_not_hide_another_tasks_editor() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        editing_two(&mut app);
        app.submit(
            "save",
            Op::Patch {
                list: "a".into(),
                task: "a-one".into(),
                patch: TaskPatch::default(),
                etag: String::new(),
            },
            &["a"],
            |_| {},
        );
        let (id, _) = sent(&cmds);

        app.on_msg(Msg::Finished {
            id,
            result: Ok(Outcome {
                upserted: vec![(
                    "a".into(),
                    Task {
                        title: "updated one".into(),
                        ..task("a-one", None, "1", false)
                    },
                )],
                ..Outcome::default()
            }),
        });

        still_editing_two(&app);
    }

    #[test]
    fn a_delayed_create_does_not_hide_another_tasks_editor() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.submit_create(
            new_edit("a", Some("a-one"), "new task"),
            Op::Create {
                list: "a".into(),
                task: crate::google::CreateTask {
                    title: "new task".into(),
                    ..Default::default()
                },
                parent: None,
            },
        );
        let (id, _) = sent(&cmds);
        editing_two(&mut app);

        app.on_msg(Msg::Finished {
            id,
            result: Ok(Outcome {
                upserted: vec![(
                    "a".into(),
                    Task {
                        id: "new".into(),
                        title: "new task".into(),
                        position: "0".into(),
                        ..Default::default()
                    },
                )],
                ..Outcome::default()
            }),
        });

        still_editing_two(&app);
    }

    #[test]
    fn a_created_task_is_selected_when_the_user_has_not_moved() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        app.start_create(false);
        app.edit_mut().unwrap().buf = "fresh".into();
        app.commit_edit();
        let (id, _) = sent(&cmds);

        app.on_msg(Msg::Finished {
            id,
            result: Ok(Outcome {
                upserted: vec![(
                    "a".into(),
                    Task {
                        title: "fresh".into(),
                        ..task("fresh", None, "0", false)
                    },
                )],
                ..Outcome::default()
            }),
        });

        assert_eq!(app.columns[0].selected_id.as_deref(), Some("fresh"));
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
        app.submit(
            "move",
            Op::MoveToList {
                from: "a".into(),
                to: "b".into(),
                ids: vec!["parent".into(), "child".into()],
            },
            &["a", "b"],
            |board| {
                board.move_tree("a", "b", "parent");
            },
        );
        let (id, _) = sent(&cmds);
        let moved_parent = Task {
            position: "9".into(),
            ..parent
        };

        app.on_msg(Msg::Finished {
            id,
            result: Ok(Outcome {
                replaced: vec![
                    ("a".into(), vec![child]),
                    ("b".into(), vec![there, moved_parent]),
                ],
                upserted: Vec::new(),
                warning: Some(Warning::PartialMove {
                    moved: 1,
                    failed: "child".into(),
                    error: Error::Transport("cannot move assigned task".into()),
                }),
            }),
        });

        assert!(app.board.task("a", "parent").is_none());
        assert_eq!(
            app.board
                .task("b", "parent")
                .map(|task| task.position.as_str()),
            Some("9")
        );
        assert_eq!(
            app.error.as_deref(),
            Some("move partially completed (1 moved); child failed: cannot move assigned task")
        );
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }
}
