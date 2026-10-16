//! Refreshing from Google: generations, per-list load state, read-only
//! recovery, and saving the startup cache.

use ratatui::widgets::ListState;

use crate::cache;
use crate::google::Error;
use crate::model::{Task, TaskList};

use super::app::{App, QuitState};
use super::columns::{visible_rows, Column};
use super::worker::Cmd;

pub(super) const READ_ONLY: &str = "Syncing with Google — read-only for a moment";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LoadState {
    Cached,
    Loading,
    Loaded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SyncState {
    Cached,
    Refreshing,
    Ready,
    Failed,
}

impl App {
    pub(super) fn refresh(&mut self) {
        self.begin_refresh(None);
    }

    pub(super) fn begin_refresh(&mut self, error: Option<String>) {
        self.error = error;
        self.sync = SyncState::Refreshing;
        if !self.cache_meta.account.is_empty() {
            if let Ok((_, meta)) = cache::load_for(&self.paths, &self.cache_meta.account) {
                self.cache_meta = meta;
            }
        }
        for col in &mut self.columns {
            col.load = LoadState::Cached;
        }
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        if self
            .cmds
            .send(Cmd::Refresh(self.refresh_generation))
            .is_err()
        {
            self.sync = SyncState::Failed;
            self.error = Some(match self.error.take() {
                Some(error) => format!("{error}; network worker stopped"),
                None => "network worker stopped".into(),
            });
        }
    }

    pub(super) fn recover(&mut self, error: String) {
        if self.quit == QuitState::Running {
            self.begin_refresh(Some(error));
        } else {
            self.error = Some(error);
            self.sync = SyncState::Failed;
        }
    }

    /// Editing waits until this run has seen Google's copy of every list.
    pub(super) fn writable(&mut self) -> bool {
        if self.sync != SyncState::Ready {
            self.error = Some(READ_ONLY.into());
            return false;
        }
        if self.pending_operation.is_some() {
            self.error = Some("wait for the current save to finish".into());
            return false;
        }
        true
    }

    pub(super) fn check_synced(&mut self) {
        if self.columns.iter().any(|c| c.load == LoadState::Failed) {
            self.sync = SyncState::Failed;
            return;
        }
        if !self.columns.iter().all(|c| c.load == LoadState::Loaded) {
            return;
        }
        self.sync = SyncState::Ready;
        if self.error.as_deref() == Some(READ_ONLY) {
            self.error = None;
        }
        self.save_cache();
    }

    pub(super) fn save_cache(&mut self) -> bool {
        if self.sync != SyncState::Ready || self.pending_operation.is_some() {
            return false;
        }
        match cache::save_for(&self.paths, &self.board.snapshot(), &mut self.cache_meta) {
            Ok(()) => true,
            Err(error) => {
                self.error = Some(error.to_string());
                false
            }
        }
    }

    pub(super) fn loading(&self) -> bool {
        self.sync == SyncState::Refreshing
    }

    pub(super) fn lists_loaded(&mut self, generation: u64, result: Result<Vec<TaskList>, Error>) {
        if generation != self.refresh_generation {
            return;
        }
        let lists = match result {
            Ok(lists) => lists,
            Err(e) => {
                self.sync = SyncState::Failed;
                self.error = Some(e.to_string());
                return;
            }
        };
        // Keep existing columns (and their selection) until fresh tasks arrive.
        let focused = self.columns.get(self.focus).map(|c| c.list.id.clone());
        let mut old = std::mem::take(&mut self.columns);
        self.board.replace_lists(lists.clone());
        self.columns = lists
            .into_iter()
            .map(|list| {
                let rows = visible_rows(&self.board, &list.id, self.show_completed);
                let mut column = match old.iter().position(|column| column.list.id == list.id) {
                    Some(index) => old.swap_remove(index),
                    None => Column {
                        list: list.clone(),
                        rows: Vec::new(),
                        load: LoadState::Loading,
                        selected_id: None,
                        state: ListState::default(),
                    },
                };
                let selected = column.selected_id.clone();
                column.list = list;
                column.rows = rows;
                column.load = LoadState::Loading;
                column.select_id(
                    selected.or_else(|| column.rows.first().map(|row| row.task.id.clone())),
                );
                column
            })
            .collect();
        self.focus = focused
            .and_then(|id| self.columns.iter().position(|c| c.list.id == id))
            .unwrap_or(0);
        self.check_synced(); // an account with no lists is already in sync
    }

    pub(super) fn tasks_loaded(
        &mut self,
        generation: u64,
        id: String,
        res: Result<Vec<Task>, Error>,
    ) {
        if generation != self.refresh_generation {
            return;
        }
        let Some(col) = self.columns.iter_mut().find(|c| c.list.id == id) else {
            return;
        };
        match res {
            Ok(tasks) => {
                self.board.replace_tasks(&id, tasks);
                col.load = LoadState::Loaded;
                let keep = col.selected_id.clone();
                let prev = col.state.selected();
                col.rows = visible_rows(&self.board, &id, self.show_completed);
                let last = col.rows.len().checked_sub(1);
                let sel = keep
                    .and_then(|id| col.rows.iter().position(|r| r.task.id == id))
                    .or_else(|| last.map(|l| prev.unwrap_or(0).min(l)));
                col.select(sel);
            }
            Err(e) => {
                col.load = LoadState::Failed;
                self.error = Some(format!("{}: {e}", col.list.title));
            }
        }
        self.check_synced();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::*;

    #[test]
    fn a_failed_list_sync_keeps_the_board_read_only() {
        let (mut app, _cmds) = app_with_lists(&["a", "b"]);
        let generation = app.refresh_generation;
        let lists = app.columns.iter().map(|c| c.list.clone()).collect();

        app.on_msg(Msg::Lists(generation, Ok(lists)));
        app.on_msg(Msg::Tasks(
            generation,
            "a".into(),
            Ok(vec![task("a-fresh", None, "1", false)]),
        ));
        app.on_msg(Msg::Tasks(
            generation,
            "b".into(),
            Err(crate::google::Error::Transport("offline".into())),
        ));

        assert_ne!(
            app.sync,
            SyncState::Ready,
            "a partial sync must not enable writes"
        );
        assert!(!app.writable());
        assert_eq!(app.columns[1].rows[0].task.id, "b-cached");
    }

    #[test]
    fn pending_mutations_prevent_cache_snapshots() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;
        pending(&mut app);

        assert!(!app.save_cache());
    }

    #[test]
    fn an_old_refresh_cannot_complete_a_newer_one() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let first = match cmds.recv().unwrap() {
            Cmd::Refresh(generation) => generation,
            _ => panic!("expected startup refresh"),
        };

        app.refresh();
        let _ = cmds.recv().unwrap(); // second refresh
        app.on_msg(Msg::Lists(first, Ok(Vec::new()))); // late response from the first

        assert_ne!(app.sync, SyncState::Ready);
        assert_eq!(app.columns.len(), 1);
        assert_eq!(app.columns[0].list.id, "a");
    }
}
