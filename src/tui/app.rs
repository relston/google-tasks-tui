use std::collections::HashSet;
use std::sync::mpsc::Sender;

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::api::{CreateTask, Status, Task, TaskList, TaskPatch};
#[cfg(test)]
use crate::board::ordered as order;
use crate::board::{Board, Row};
use crate::cache;
use crate::config::Config;
use crate::keybindings::{Action, BindingContext, Keymap};
use crate::paths::Paths;
use crate::platform::{self, Clipboard};
use crate::presentation::{md_yank, normalize_title, urls_in};
use crate::theme::{self, Theme};

use super::board::{
    eligible_parents, move_ids, other_lists, previous_sibling, reorder_after, reorder_step,
    subtask_parent, ReorderPlan,
};
use super::text::{byte_at_column, clean, cursor_rc, title_of, wrap};
use super::worker::{Cmd, Msg};
#[cfg(test)]
use super::worker::{MoveFailure, MoveOutcome};

const READ_ONLY: &str = "Syncing with Google — read-only for a moment";

/// A delete waiting on a y/n answer in the status bar.
pub(super) struct Pending {
    list_id: String,
    task_id: String,
    etag: String,
    pub(super) prompt: String,
}

/// One row of the Browse overlay.
pub(super) struct BrowseLink {
    /// The source's title, for links Google attached to the task. URLs picked
    /// out of the notes have nothing to call themselves.
    pub(super) name: Option<String>,
    pub(super) url: String,
}

/// One row of a modal list. `value` is what the caller acts on — a URL for
/// Browse, a list id for Move, nothing for the Keyboard table — and `filter`
/// is the lowercased haystack the `/` query matches against.
pub(super) struct PickerRow {
    pub(super) title: String,
    pub(super) subtitle: Option<String>,
    pub(super) value: String,
    filter: String,
}

impl PickerRow {
    pub(super) fn new(title: String, subtitle: Option<String>, value: String) -> Self {
        let mut filter = title.to_lowercase();
        if let Some(subtitle) = &subtitle {
            filter.push(' ');
            filter.push_str(&subtitle.to_lowercase());
        }
        Self {
            title,
            subtitle,
            value,
            filter,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PickerStyle {
    /// Rows to choose from, each a title above its subtitle.
    Choose,
    /// A read-only card: `title` is a key column beside `subtitle`, and
    /// nothing is selectable.
    Reference,
}

/// A modal list with a `/` filter. One of these backs Browse, Move and the
/// Keyboard overlay, so all three share layout, filtering and empty state.
pub(super) struct Picker {
    rows: Vec<PickerRow>,
    /// Indexes into `rows` that survive the current query, in source order.
    shown: Vec<usize>,
    query: String,
    /// `/` was pressed: keys go to the query instead of the list.
    typing: bool,
    style: PickerStyle,
    pub(super) state: ListState,
}

impl Picker {
    pub(super) fn new(rows: Vec<PickerRow>) -> Self {
        Self {
            shown: (0..rows.len()).collect(),
            rows,
            query: String::new(),
            typing: false,
            style: PickerStyle::Choose,
            state: ListState::default().with_selected(Some(0)),
        }
    }

    pub(super) fn reference(rows: Vec<PickerRow>) -> Self {
        Self {
            style: PickerStyle::Reference,
            state: ListState::default(),
            ..Self::new(rows)
        }
    }

    pub(super) fn rows(&self) -> impl Iterator<Item = &PickerRow> {
        self.shown.iter().map(|index| &self.rows[*index])
    }

    pub(super) fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    pub(super) fn query(&self) -> &str {
        &self.query
    }

    pub(super) fn typing(&self) -> bool {
        self.typing
    }

    pub(super) fn style(&self) -> PickerStyle {
        self.style
    }

    pub(super) fn selected(&self) -> Option<&PickerRow> {
        self.state
            .selected()
            .and_then(|index| self.shown.get(index))
            .map(|index| &self.rows[*index])
    }

    /// Every value still passing the filter, so "open all" opens what you
    /// narrowed to rather than everything.
    pub(super) fn values(&self) -> Vec<String> {
        self.rows().map(|row| row.value.clone()).collect()
    }

    pub(super) fn step(&mut self, delta: isize) {
        let Some(last) = self.shown.len().checked_sub(1) else {
            return;
        };
        let current = self.state.selected().unwrap_or(0) as isize;
        self.state
            .select(Some((current + delta).clamp(0, last as isize) as usize));
    }

    /// Case-insensitive substring match over the title and subtitle. Selection
    /// returns to the top, because the row it pointed at may be gone.
    fn refilter(&mut self) {
        let needle = self.query.to_lowercase();
        self.shown = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| needle.is_empty() || row.filter.contains(&needle))
            .map(|(index, _)| index)
            .collect();
        let has_selection = self.style == PickerStyle::Choose && !self.shown.is_empty();
        self.state.select(has_selection.then_some(0));
    }
}

/// What a key meant to the overlay that owns the picker.
enum PickerKey {
    Consumed,
    Close,
    Accept,
    AcceptAll,
}

/// Apply an already-resolved picker action. While filtering, unmatched
/// printable keys extend the query, so command letters remain typeable.
fn picker_key(picker: &mut Picker, action: Option<Action>, k: KeyEvent) -> PickerKey {
    if picker.typing {
        return match action {
            Some(Action::StopFilter) => {
                picker.typing = false;
                PickerKey::Consumed
            }
            Some(Action::FilterAcceptAll) => PickerKey::AcceptAll,
            Some(Action::FilterAccept) => PickerKey::Accept,
            Some(Action::FilterBackspace) => {
                picker.query.pop();
                picker.refilter();
                PickerKey::Consumed
            }
            Some(Action::FilterNext) => {
                picker.step(1);
                PickerKey::Consumed
            }
            Some(Action::FilterPrevious) => {
                picker.step(-1);
                PickerKey::Consumed
            }
            None if matches!(k.code, KeyCode::Char(_)) && typed(k.modifiers) => {
                let KeyCode::Char(c) = k.code else {
                    unreachable!()
                };
                picker.query.push(c);
                picker.refilter();
                PickerKey::Consumed
            }
            _ => PickerKey::Consumed,
        };
    }
    match action {
        Some(Action::ClosePicker) => PickerKey::Close,
        Some(Action::StartFilter) => {
            picker.typing = true;
            PickerKey::Consumed
        }
        Some(Action::PickerNext) => {
            picker.step(1);
            PickerKey::Consumed
        }
        Some(Action::PickerPrevious) => {
            picker.step(-1);
            PickerKey::Consumed
        }
        Some(Action::PickerAcceptAll) => PickerKey::AcceptAll,
        Some(Action::PickerAccept) => PickerKey::Accept,
        _ => PickerKey::Consumed,
    }
}

/// Overlay listing possible parent tasks in the current list.
/// Reparenting needs the task being moved as well as the candidates, so the
/// picker carries the context the rows cannot.
pub(super) struct ParentPick {
    pub(super) list_id: String,
    pub(super) task_id: String,
    pub(super) picker: Picker,
}

/// The editable fields of the shelf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldId {
    Title,
    Notes,
}

impl FieldId {
    fn other(self) -> Self {
        match self {
            Self::Title => Self::Notes,
            Self::Notes => Self::Title,
        }
    }
}

/// The details shelf beside the focused column, and the field the keyboard
/// is on while it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shelf {
    Closed,
    Open(FieldId),
}

impl Shelf {
    pub(super) fn is_open(self) -> bool {
        matches!(self, Self::Open(_))
    }
}

/// Shutdown progress. Only a quit that waited for a save joins the worker
/// afterwards, so that save is never cut off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuitState {
    Running,
    /// Quit was requested while a save was in flight.
    Waiting,
    /// The awaited save landed.
    Drained,
    /// Nothing was pending, or the user forced it.
    Now,
}

pub(super) enum EditTarget {
    Existing {
        list_id: String,
        task_id: String,
        field: FieldId,
    },
    New {
        list_id: String,
        parent: Option<String>,
        selection: Option<String>,
    },
}

/// An in-progress edit. `cursor` is a byte offset into `buf`.
pub(super) struct Edit {
    pub(super) target: EditTarget,
    pub(super) buf: String,
    pub(super) cursor: usize,
}

impl Edit {
    pub(super) fn field(&self) -> FieldId {
        match self.target {
            EditTarget::Existing { field, .. } => field,
            EditTarget::New { .. } => FieldId::Title,
        }
    }

    pub(super) fn existing(&self) -> Option<(&str, &str, FieldId)> {
        match &self.target {
            EditTarget::Existing {
                list_id,
                task_id,
                field,
            } => Some((list_id, task_id, *field)),
            EditTarget::New { .. } => None,
        }
    }

    fn is_new(&self) -> bool {
        matches!(self.target, EditTarget::New { .. })
    }

    fn insert(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn left(&mut self) {
        if let Some((i, _)) = self.buf[..self.cursor].grapheme_indices(true).next_back() {
            self.cursor = i;
        }
    }

    fn right(&mut self) {
        if let Some(grapheme) = self.buf[self.cursor..].graphemes(true).next() {
            self.cursor += grapheme.len();
        }
    }

    fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.buf.replace_range(self.cursor..end, "");
    }

    fn delete(&mut self) {
        if let Some(grapheme) = self.buf[self.cursor..].graphemes(true).next() {
            let end = self.cursor + grapheme.len();
            self.buf.replace_range(self.cursor..end, "");
        }
    }

    fn kill_word(&mut self) {
        let head = self.buf[..self.cursor].trim_end_matches(char::is_whitespace);
        let start = head
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
        self.buf.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn kill_to_start(&mut self) {
        let start = self.line_start();
        self.buf.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn line_start(&self) -> usize {
        self.buf[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.buf[self.cursor..]
            .find('\n')
            .map_or(self.buf.len(), |i| self.cursor + i)
    }

    fn step_visual_line(&mut self, width: usize, down: bool) {
        let segments = wrap(&self.buf, width);
        let (row, column) = cursor_rc(&segments, &self.buf, self.cursor);
        let target = if down {
            (row + 1).min(segments.len().saturating_sub(1))
        } else {
            row.saturating_sub(1)
        };
        if target == row {
            return;
        }
        let (start, end) = segments[target];
        self.cursor = byte_at_column(&self.buf, start, end, column);
    }

    /// The slice that fits `width` columns with the cursor in view, and the
    /// cursor's offset within it.
    pub(super) fn view(&self, width: usize) -> (&str, usize) {
        let width = width.max(1);
        let mut boundaries: Vec<usize> = self
            .buf
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .collect();
        boundaries.push(self.buf.len());
        let current = boundaries
            .iter()
            .position(|index| *index >= self.cursor)
            .unwrap_or(boundaries.len() - 1);
        let mut start = current;
        while start > 0
            && UnicodeWidthStr::width(&self.buf[boundaries[start - 1]..self.cursor]) < width
        {
            start -= 1;
        }
        let mut end = current;
        while end + 1 < boundaries.len()
            && UnicodeWidthStr::width(&self.buf[boundaries[start]..boundaries[end + 1]]) <= width
        {
            end += 1;
        }
        let from = boundaries[start];
        let to = boundaries[end];
        (
            &self.buf[from..to],
            UnicodeWidthStr::width(&self.buf[from..self.cursor]),
        )
    }
}

pub(super) enum Interaction {
    Idle,
    Editing(Edit),
    ConfirmDelete(Pending),
    Browse(Picker),
    Move(Picker),
    Parent(ParentPick),
    Help(Picker),
}

impl Interaction {
    pub(super) fn picker(&self) -> Option<&Picker> {
        match self {
            Interaction::Browse(p) | Interaction::Move(p) | Interaction::Help(p) => Some(p),
            Interaction::Parent(parent) => Some(&parent.picker),
            _ => None,
        }
    }

    pub(super) fn picker_mut(&mut self) -> Option<&mut Picker> {
        match self {
            Interaction::Browse(p) | Interaction::Move(p) | Interaction::Help(p) => Some(p),
            Interaction::Parent(parent) => Some(&mut parent.picker),
            _ => None,
        }
    }

    /// The heading drawn in the overlay's border.
    pub(super) fn picker_title(&self) -> &'static str {
        match self {
            Interaction::Browse(_) => " Browse Link ",
            Interaction::Move(_) => " Move ",
            Interaction::Parent(_) => " Select Parent ",
            _ => " Keyboard ",
        }
    }
}

enum MutationContext {
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
    id: u64,
    context: MutationContext,
}

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

pub(super) struct Column {
    pub(super) list: TaskList,
    pub(super) rows: Vec<Row>,
    pub(super) load: LoadState,
    pub(super) selected_id: Option<String>,
    pub(super) state: ListState,
}

impl Column {
    pub(super) fn selected(&self) -> Option<&Row> {
        let selected = self.selected_id.as_deref()?;
        self.rows.iter().find(|row| row.task.id == selected)
    }

    fn select(&mut self, index: Option<usize>) {
        self.selected_id = index
            .and_then(|index| self.rows.get(index))
            .map(|row| row.task.id.clone());
        self.state.select(index);
    }

    fn select_id(&mut self, task_id: Option<String>) {
        let index = task_id
            .as_deref()
            .and_then(|id| self.rows.iter().position(|row| row.task.id == id));
        self.select(index);
    }
}

/// Column rows. Hiding completed tasks keeps a completed parent while it has
/// open subtasks: Google scopes subtask positions to their parent, so those
/// subtasks have no meaningful place among the top-level tasks.
fn visible_rows(board: &Board, list_id: &str, show_completed: bool) -> Vec<Row> {
    let rows = board.rows(list_id);
    if show_completed {
        return rows;
    }
    let open_parents: HashSet<String> = rows
        .iter()
        .filter(|row| row.depth > 0 && !row.task.is_done())
        .filter_map(|row| row.task.parent.clone())
        .collect();
    rows.into_iter()
        .filter(|row| !row.task.is_done() || open_parents.contains(&row.task.id))
        .collect()
}

pub(super) struct App {
    pub(super) config: Config,
    pub(super) keymap: Keymap<Action>,
    pub(super) theme: Theme,
    pub(super) fingerprint: theme::Fingerprint,
    pub(super) transparent: bool,
    /// When false, completed tasks stay on the board but leave the columns.
    pub(super) show_completed: bool,
    pub(super) board: Board,
    pub(super) paths: Paths,
    pub(super) cache_meta: cache::Meta,
    pub(super) columns: Vec<Column>,
    pub(super) focus: usize,
    /// Index of the leftmost visible column.
    pub(super) scroll: usize,
    pub(super) shelf: Shelf,
    pub(super) edit_width: usize,
    pub(super) interaction: Interaction,
    /// Identifies the only refresh whose responses may update the board.
    pub(super) refresh_generation: u64,
    pub(super) sync: SyncState,
    pub(super) next_operation: u64,
    pub(super) pending_operation: Option<PendingOperation>,
    pub(super) error: Option<String>,
    /// A one-off confirmation, cleared by the next keypress.
    pub(super) notice: Option<String>,
    pub(super) cmds: Sender<Cmd>,
    pub(super) quit: QuitState,
}

impl App {
    /// A test app whose files live in a fresh temporary directory.
    #[cfg(test)]
    pub(super) fn new(cmds: Sender<Cmd>, cached: cache::Cache, config: Config) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gtt-app-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self::new_with_storage(
            cmds,
            cached,
            config,
            Paths {
                credentials: root.join("credentials.json"),
                config_dir: root,
            },
            cache::Meta {
                account: String::new(),
                generation: 0,
            },
            None,
        )
    }

    pub(super) fn new_with_storage(
        cmds: Sender<Cmd>,
        cached: cache::Cache,
        config: Config,
        paths: Paths,
        cache_meta: cache::Meta,
        warning: Option<String>,
    ) -> Self {
        let (keymap, key_diagnostics) = config.keymap();
        let key_warning = (!key_diagnostics.is_empty()).then(|| key_diagnostics.join("; "));
        let warning = match (warning, key_warning) {
            (Some(existing), Some(keys)) => Some(format!("{existing}; {keys}")),
            (warning, None) | (None, warning) => warning,
        };
        // Paint the last known board straight away; the sync replaces it.
        let board = Board::from_cache(cached);
        let columns = board
            .lists()
            .map(|list| {
                let rows = visible_rows(&board, &list.id, true);
                let selected_id = rows.first().map(|row| row.task.id.clone());
                Column {
                    list: list.clone(),
                    state: ListState::default().with_selected((!rows.is_empty()).then_some(0)),
                    rows,
                    load: LoadState::Cached,
                    selected_id,
                }
            })
            .collect();
        let theme = theme::load(&config.theme);
        let mut app = Self {
            config,
            keymap,
            theme,
            fingerprint: theme::fingerprint(),
            transparent: false,
            show_completed: true,
            board,
            paths,
            cache_meta,
            columns,
            focus: 0,
            scroll: 0,
            shelf: Shelf::Closed,
            edit_width: 0,
            interaction: Interaction::Idle,
            refresh_generation: 0,
            sync: SyncState::Cached,
            next_operation: 0,
            pending_operation: None,
            error: None,
            notice: None,
            cmds,
            quit: QuitState::Running,
        };
        app.begin_refresh(warning);
        app
    }

    pub(super) fn edit(&self) -> Option<&Edit> {
        match &self.interaction {
            Interaction::Editing(edit) => Some(edit),
            _ => None,
        }
    }

    fn edit_mut(&mut self) -> Option<&mut Edit> {
        match &mut self.interaction {
            Interaction::Editing(edit) => Some(edit),
            _ => None,
        }
    }

    pub(super) fn is_creating(&self) -> bool {
        self.edit().is_some_and(Edit::is_new)
    }

    fn refresh(&mut self) {
        self.begin_refresh(None);
    }

    fn begin_refresh(&mut self, error: Option<String>) {
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

    fn recover(&mut self, error: String) {
        if self.quit == QuitState::Running {
            self.begin_refresh(Some(error));
        } else {
            self.error = Some(error);
            self.sync = SyncState::Failed;
        }
    }

    pub(super) fn should_exit(&self) -> bool {
        matches!(self.quit, QuitState::Drained | QuitState::Now)
    }

    fn request_quit(&mut self) {
        self.quit = if self.quit == QuitState::Running && self.pending_operation.is_some() {
            let quit = self.keymap.labels_for(Action::Quit).join("/");
            self.notice = Some(format!(
                "waiting for pending save · {quit} again to force quit"
            ));
            QuitState::Waiting
        } else {
            QuitState::Now
        };
    }

    fn mutation_finished(&mut self, operation: u64) -> Option<MutationContext> {
        if self.pending_operation.as_ref().map(|pending| pending.id) != Some(operation) {
            return None;
        }
        let context = self.pending_operation.take().map(|pending| pending.context);
        if self.quit == QuitState::Waiting {
            self.quit = QuitState::Drained;
        }
        context
    }

    fn send_mutation(&mut self, context: MutationContext, build: impl FnOnce(u64) -> Cmd) -> bool {
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

    fn rollback(&mut self, context: MutationContext) {
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

    /// Editing waits until this run has seen Google's copy of every list.
    fn writable(&mut self) -> bool {
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

    fn check_synced(&mut self) {
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

    fn save_cache(&mut self) -> bool {
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

    /// Start a new task in the focused list: an empty title input in the shelf.
    /// `under` makes it a subtask of the selection (see `subtask_parent`).
    pub(super) fn start_create(&mut self, under: bool) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let list_id = col.list.id.clone();
        let selected = col.selected().map(|row| row.task.id.clone());
        if under
            && col
                .selected()
                .is_some_and(|row| row.task.assignment_info.is_some())
        {
            self.error = Some("assigned tasks cannot have subtasks".into());
            return;
        }
        let parent = match (under, col.selected().map(subtask_parent)) {
            (false, _) => None,
            (true, Some(p)) => p,
            (true, None) => {
                self.error = Some("select a task to put the subtask under".into());
                return;
            }
        };
        self.interaction = Interaction::Editing(Edit {
            target: EditTarget::New {
                list_id,
                parent,
                selection: selected,
            },
            buf: String::new(),
            cursor: 0,
        });
        self.shelf = Shelf::Open(FieldId::Title);
    }

    /// Edit the focused shelf field, opening the shelf on the title if it's shut.
    fn start_edit(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let Some(row) = col.selected() else { return };
        let field = match self.shelf {
            Shelf::Open(field) => field,
            Shelf::Closed => FieldId::Title,
        };
        if field == FieldId::Notes && row.task.assignment_info.is_some() {
            self.error = Some("tasks assigned from Docs or Chat cannot have notes".into());
            return;
        }
        let (list_id, task_id) = (col.list.id.clone(), row.task.id.clone());
        let buf = match field {
            FieldId::Title => row.task.title.clone(),
            FieldId::Notes => row.task.notes.clone().unwrap_or_default(),
        };
        self.shelf = Shelf::Open(field);
        self.interaction = Interaction::Editing(Edit {
            target: EditTarget::Existing {
                list_id,
                task_id,
                field,
            },
            cursor: buf.len(),
            buf,
        });
    }

    fn toggle_completed(&mut self) {
        let selected = self.selected_task().map(|row| row.task.id.clone());
        self.show_completed = !self.show_completed;
        let ids: Vec<String> = self
            .columns
            .iter()
            .map(|column| column.list.id.clone())
            .collect();
        for id in ids {
            self.rebuild_column(&id);
        }
        let still_selected = self
            .selected_task()
            .is_some_and(|row| Some(row.task.id.as_str()) == selected.as_deref());
        if !still_selected {
            self.shelf = Shelf::Closed;
        }
    }

    fn rebuild_column(&mut self, list_id: &str) {
        let rows = visible_rows(&self.board, list_id, self.show_completed);
        let Some(column) = self
            .columns
            .iter_mut()
            .find(|column| column.list.id == list_id)
        else {
            return;
        };
        let selected = column.selected_id.clone();
        let previous = column.state.selected().unwrap_or(0);
        column.rows = rows;
        if selected
            .as_deref()
            .is_some_and(|id| column.rows.iter().any(|row| row.task.id == id))
        {
            column.select_id(selected);
        } else {
            let last = column.rows.len().checked_sub(1);
            column.select(last.map(|last| previous.min(last)));
        }
    }

    fn commit_edit(&mut self) {
        if !self.writable() {
            return;
        }
        let interaction = std::mem::replace(&mut self.interaction, Interaction::Idle);
        let Interaction::Editing(e) = interaction else {
            self.interaction = interaction;
            return;
        };
        if let EditTarget::New {
            list_id,
            parent,
            selection,
        } = &e.target
        {
            let title = match normalize_title(&e.buf) {
                Ok(title) => title,
                Err(error) => {
                    self.error = Some(error.into());
                    self.interaction = Interaction::Editing(e);
                    return;
                }
            };
            self.error = None;
            let (list_id, parent, selection) = (list_id.clone(), parent.clone(), selection.clone());
            self.send_mutation(MutationContext::Create(e), |operation| Cmd::Create {
                operation,
                list_id,
                task: CreateTask {
                    title,
                    ..Default::default()
                },
                parent,
                selection,
            });
            return;
        }
        // Show the change straight away; Google confirms it a moment later.
        let Some((list_id, task_id, field)) = e.existing() else {
            return;
        };
        let (list_id, task_id) = (list_id.to_string(), task_id.to_string());
        let Some(before) = self.board.task(&list_id, &task_id).cloned() else {
            self.recover("edited task disappeared".into());
            return;
        };
        let fields = match field {
            FieldId::Title => {
                let title = match normalize_title(&e.buf) {
                    Ok(title) => title,
                    Err(error) => {
                        self.error = Some(error.into());
                        self.interaction = Interaction::Editing(e);
                        return;
                    }
                };
                if let Some(task) = self.board.task_mut(&list_id, &task_id) {
                    task.title = title.clone();
                }
                self.rebuild_column(&list_id);
                TaskPatch {
                    title: Some(title),
                    ..Default::default()
                }
            }
            FieldId::Notes => {
                let notes = e.buf.trim_end().to_string();
                if let Some(task) = self.board.task_mut(&list_id, &task_id) {
                    task.notes = Some(notes.clone()).filter(|notes| !notes.is_empty());
                }
                self.rebuild_column(&list_id);
                TaskPatch {
                    notes: Some(notes),
                    ..Default::default()
                }
            }
        };
        self.error = None;
        self.send_patch(list_id, task_id, fields, before);
    }

    /// Tick or untick the selected task.
    fn toggle_done(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let list_id = col.list.id.clone();
        let Some(task_id) = col.selected_id.clone() else {
            return;
        };
        let Some(task) = self.board.task_mut(&list_id, &task_id) else {
            return;
        };
        let before = task.clone();
        // Show the new state now; Google's copy replaces it when it answers.
        task.status = if task.is_done() {
            task.completed = None;
            Status::NeedsAction
        } else {
            Status::Completed
        };
        let fields = TaskPatch::status(task.status);
        self.rebuild_column(&list_id);
        self.send_patch(list_id, task_id, fields, before);
    }

    /// Ask before deleting; `confirm_delete` does the work after the configured
    /// confirmation key.
    fn ask_delete(&mut self) {
        if !self.writable() {
            return;
        }
        let confirm = self.keymap.labels_for(Action::ConfirmDelete).join("/");
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let Some(row) = col.selected() else { return };
        let subtasks = self
            .board
            .rows(&col.list.id)
            .iter()
            .filter(|candidate| candidate.task.parent.as_deref() == Some(row.task.id.as_str()))
            .count();
        let also = match subtasks {
            0 => String::new(),
            1 => " and its subtask".into(),
            n => format!(" and its {n} subtasks"),
        };
        let assigned = if row.task.assignment_info.is_some() {
            " This also deletes the original Docs/Chat task."
        } else {
            ""
        };
        self.interaction = Interaction::ConfirmDelete(Pending {
            list_id: col.list.id.clone(),
            task_id: row.task.id.clone(),
            etag: row.task.etag.clone(),
            prompt: format!(
                "Delete \"{}\"{also}?{assigned}  {confirm} confirms · any other key cancels",
                title_of(&row.task)
            ),
        });
    }

    fn confirm_delete(&mut self) {
        let interaction = std::mem::replace(&mut self.interaction, Interaction::Idle);
        let Interaction::ConfirmDelete(p) = interaction else {
            self.interaction = interaction;
            return;
        };
        // Drop the task and its subtasks locally; Google cascades the same way.
        let removed = self.board.remove_tree(&p.list_id, &p.task_id);
        self.rebuild_column(&p.list_id);
        self.error = None;
        let (list_id, task_id, etag) = (p.list_id, p.task_id, p.etag);
        self.send_mutation(
            MutationContext::Delete {
                list_id: list_id.clone(),
                task_id: task_id.clone(),
                removed,
            },
            |operation| Cmd::Delete {
                operation,
                list_id,
                task_id,
                etag,
            },
        );
    }

    fn send_patch(&mut self, list_id: String, task_id: String, patch: TaskPatch, before: Task) {
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

    /// Reload the palette only when the fingerprint changes (cheap stat calls).
    pub(super) fn check_theme(&mut self) {
        let fp = theme::fingerprint();
        if fp != self.fingerprint {
            self.fingerprint = fp;
            self.theme = theme::load(&self.config.theme);
        }
    }

    pub(super) fn on_msg(&mut self, msg: Msg) {
        match &msg {
            Msg::Lists(generation, _) | Msg::Tasks(generation, _, _)
                if *generation != self.refresh_generation =>
            {
                return;
            }
            _ => {}
        }
        match msg {
            Msg::Lists(_, Err(e)) => {
                self.sync = SyncState::Failed;
                self.error = Some(e);
            }
            Msg::Lists(_, Ok(lists)) => {
                // Keep existing columns (and their selection) until fresh tasks arrive.
                let focused = self.columns.get(self.focus).map(|c| c.list.id.clone());
                let mut old = std::mem::take(&mut self.columns);
                self.board.replace_lists(lists.clone());
                self.columns = lists
                    .into_iter()
                    .map(|list| {
                        let rows = visible_rows(&self.board, &list.id, self.show_completed);
                        let mut column =
                            match old.iter().position(|column| column.list.id == list.id) {
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
            Msg::Tasks(_, id, res) => {
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

    fn move_task(&mut self, delta: isize) {
        let Some(col) = self.columns.get_mut(self.focus) else {
            return;
        };
        let Some(last) = col.rows.len().checked_sub(1) else {
            return;
        };
        let cur = col.state.selected().unwrap_or(0) as isize;
        col.select(Some((cur + delta).clamp(0, last as isize) as usize));
    }

    pub(super) fn selected_task(&self) -> Option<&Row> {
        self.columns.get(self.focus)?.selected()
    }

    /// Copy the selected task as markdown, ready to paste into a note.
    fn yank(&mut self) {
        let Some(row) = self.selected_task() else {
            return;
        };
        // The same control characters that would wreck a row would paste badly.
        let md = md_yank(&row.task);
        let title = title_of(&row.task).into_owned();
        match platform::copy(&md) {
            Ok(Clipboard::Confirmed) => {
                self.error = None;
                self.notice = Some(format!("copied \"{title}\" as markdown"));
            }
            Ok(Clipboard::SequenceSent) => {
                self.error = None;
                self.notice = Some(format!("sent \"{title}\" to the terminal clipboard"));
            }
            Err(error) => {
                self.notice = None;
                self.error = Some(format!("clipboard failed: {error}"));
            }
        }
    }

    fn start_browse(&mut self) {
        let Some(row) = self.selected_task() else {
            return;
        };
        let links = task_links(&row.task);
        if links.is_empty() {
            self.error = Some("no links on this task".into());
            return;
        }
        self.error = None;
        // A named link shows its URL underneath; a bare one is its own label.
        let rows = links
            .into_iter()
            .map(|link| match link.name {
                Some(name) => PickerRow::new(name, Some(link.url.clone()), link.url),
                None => PickerRow::new(link.url.clone(), None, link.url),
            })
            .collect();
        self.interaction = Interaction::Browse(Picker::new(rows));
    }

    fn open_browse(&mut self, all: bool) {
        let urls: Vec<String> = {
            let Interaction::Browse(picker) = &self.interaction else {
                return;
            };
            match all {
                true => picker.values(),
                false => picker
                    .selected()
                    .map(|row| vec![row.value.clone()])
                    .unwrap_or_default(),
            }
        };
        if urls.is_empty() {
            return;
        }
        for url in &urls {
            if let Err(error) = platform::open_url(url) {
                self.error = Some(format!("could not open {url}: {error}"));
                return;
            }
        }
        self.interaction = Interaction::Idle;
    }

    fn start_parent_pick(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(column) = self.columns.get(self.focus) else {
            return;
        };
        let Some(selected) = column.selected() else {
            return;
        };
        if selected.task.parent.is_some() {
            self.error = Some("unparent this task before choosing a new parent".into());
            return;
        }
        if selected.task.assignment_info.is_some() {
            self.error = Some("assigned tasks cannot become subtasks".into());
            return;
        }
        let tree = self.board.rows(&column.list.id);
        if tree
            .iter()
            .any(|row| row.task.parent.as_deref() == Some(selected.task.id.as_str()))
        {
            self.error = Some("a task with subtasks cannot become a subtask".into());
            return;
        }
        if selected.task.hidden && selected.task.is_done() {
            self.error = Some("a completed hidden task cannot become a subtask".into());
            return;
        }
        let tasks = eligible_parents(&tree, &selected.task.id);
        if tasks.is_empty() {
            self.error = Some("no available parent tasks in this list".into());
            return;
        }
        self.error = None;
        let rows = tasks
            .into_iter()
            .map(|task| PickerRow::new(title_of(&task).into_owned(), None, task.id))
            .collect();
        self.interaction = Interaction::Parent(ParentPick {
            list_id: column.list.id.clone(),
            task_id: selected.task.id.clone(),
            picker: Picker::new(rows),
        });
    }

    fn confirm_parent(&mut self) {
        let selection = {
            let Interaction::Parent(parent) = &self.interaction else {
                return;
            };
            let Some(row) = parent.picker.selected() else {
                return;
            };
            (
                parent.list_id.clone(),
                parent.task_id.clone(),
                row.value.clone(),
            )
        };
        self.interaction = Interaction::Idle;
        self.reparent_task(&selection.0, &selection.1, Some(selection.2), None);
    }

    fn unparent_task(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(column) = self.columns.get(self.focus) else {
            return;
        };
        let Some(selected) = column.selected() else {
            return;
        };
        if selected.task.parent.is_none() {
            self.error = Some("task is already top-level".into());
            return;
        }
        let list_id = column.list.id.clone();
        let task_id = selected.task.id.clone();
        let previous = selected.task.parent.clone();
        self.reparent_task(&list_id, &task_id, None, previous);
    }

    fn reparent_task(
        &mut self,
        list_id: &str,
        task_id: &str,
        parent: Option<String>,
        previous: Option<String>,
    ) {
        let Some(before) = self.board.task(list_id, task_id).cloned() else {
            self.error = Some("task disappeared before it could move".into());
            return;
        };
        if let Some(task) = self.board.task_mut(list_id, task_id) {
            task.parent = parent.clone();
        }
        self.rebuild_column(list_id);
        self.error = None;
        let list_id = list_id.to_string();
        let task_id = task_id.to_string();
        self.send_mutation(
            MutationContext::Reparent {
                list_id: list_id.clone(),
                task_id: task_id.clone(),
                before: Box::new(before),
            },
            |operation| Cmd::Reparent {
                operation,
                list_id,
                task_id,
                parent,
                previous,
            },
        );
    }

    /// Move the selected task one place among its open siblings. Presses made
    /// while that task's reorder is saving move it locally and are sent as
    /// one move when the save lands.
    fn reorder_task(&mut self, down: bool) {
        let Some(column) = self.columns.get(self.focus) else {
            return;
        };
        let Some(selected) = column.selected() else {
            return;
        };
        let list_id = column.list.id.clone();
        let task_id = selected.task.id.clone();
        let queue = matches!(
            &self.pending_operation,
            Some(PendingOperation {
                context: MutationContext::Reorder {
                    list_id: pending_list,
                    task_id: pending_task,
                    ..
                },
                ..
            }) if *pending_list == list_id && *pending_task == task_id
        );
        if queue && self.sync != SyncState::Ready {
            self.error = Some(READ_ONLY.into());
            return;
        }
        if !queue && !self.writable() {
            return;
        }
        let plan = match reorder_step(&self.board.rows(&list_id), &task_id, down) {
            Ok(plan) => plan,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        self.error = None;
        if !queue {
            self.send_reorder(list_id, task_id, plan);
            return;
        }
        self.apply_positions(&list_id, &plan.positions);
        if let Some(PendingOperation {
            context: MutationContext::Reorder { queued, .. },
            ..
        }) = &mut self.pending_operation
        {
            *queued = true;
        }
    }

    /// Send the presses made while a reorder was saving as one move to where
    /// the board showed the task.
    fn send_queued_reorder(&mut self, list_id: String, task_id: String, previous: Option<String>) {
        if self.sync != SyncState::Ready || self.quit == QuitState::Now {
            return;
        }
        let rows = self.board.rows(&list_id);
        match reorder_after(&rows, &task_id, previous.as_deref()) {
            Ok(plan) if plan.positions.is_empty() => {}
            Ok(plan) => {
                if self.send_reorder(list_id, task_id, plan) && self.quit == QuitState::Drained {
                    // The queued move is a pending write, so a waiting quit keeps waiting.
                    self.quit = QuitState::Waiting;
                }
            }
            Err(error) => self.error = Some(format!("queued reorder dropped: {error}")),
        }
    }

    fn send_reorder(&mut self, list_id: String, task_id: String, plan: ReorderPlan) -> bool {
        let before = self.board.tasks(&list_id);
        self.apply_positions(&list_id, &plan.positions);
        self.send_mutation(
            MutationContext::Reorder {
                list_id: list_id.clone(),
                task_id: task_id.clone(),
                before,
                queued: false,
            },
            |operation| Cmd::Reorder {
                operation,
                list_id,
                task_id,
                parent: plan.parent,
                previous: plan.previous,
            },
        )
    }

    fn apply_positions(&mut self, list_id: &str, positions: &[(String, String)]) {
        for (task_id, position) in positions {
            if let Some(task) = self.board.task_mut(list_id, task_id) {
                task.position = position.clone();
            }
        }
        self.rebuild_column(list_id);
    }

    fn start_move(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let Some(selected) = col.selected() else {
            return;
        };
        if selected.task.assignment_info.is_some()
            || self.board.rows(&col.list.id).iter().any(|row| {
                row.task.parent.as_deref() == Some(selected.task.id.as_str())
                    && row.task.assignment_info.is_some()
            })
        {
            self.error = Some("assigned tasks cannot move between lists".into());
            return;
        }
        let current = col.list.id.clone();
        let all: Vec<TaskList> = self.columns.iter().map(|c| c.list.clone()).collect();
        let lists: Vec<TaskList> = other_lists(&all, &current).into_iter().cloned().collect();
        if lists.is_empty() {
            self.error = Some("no other lists".into());
            return;
        }
        self.error = None;
        let rows = lists
            .into_iter()
            .map(|list| PickerRow::new(clean(&list.title).into_owned(), None, list.id))
            .collect();
        self.interaction = Interaction::Move(Picker::new(rows));
    }

    fn confirm_move(&mut self) {
        let dest_id = {
            let Interaction::Move(picker) = &self.interaction else {
                return;
            };
            let Some(row) = picker.selected() else { return };
            row.value.clone()
        };
        self.interaction = Interaction::Idle;
        let Some(src) = self.columns.get(self.focus) else {
            return;
        };
        let from_id = src.list.id.clone();
        let Some(row) = src.selected() else { return };
        let task_id = row.task.id.clone();
        let ids = move_ids(&self.board.rows(&from_id), &task_id);
        if ids.is_empty() {
            return;
        }
        let Some(dest_idx) = self.columns.iter().position(|c| c.list.id == dest_id) else {
            return;
        };
        let source_before = self.board.tasks(&from_id);
        let destination_before = self.board.tasks(&dest_id);
        self.board.move_tree(&from_id, &dest_id, &task_id);
        self.rebuild_column(&from_id);
        self.rebuild_column(&dest_id);
        self.columns[dest_idx].select_id(Some(task_id.clone()));
        self.focus = dest_idx;
        self.error = None;
        self.send_mutation(
            MutationContext::Move {
                from: from_id.clone(),
                to: dest_id.clone(),
                ids: ids.clone(),
                source_before,
                destination_before,
            },
            |operation| Cmd::Move {
                operation,
                from: from_id,
                to: dest_id,
                ids,
            },
        );
    }

    fn start_help(&mut self) {
        let rows = Action::ALL
            .iter()
            .filter_map(|action| {
                let labels = self.keymap.labels_for(*action);
                (!labels.is_empty()).then(|| {
                    let key = labels.join(" / ");
                    let description = action.description();
                    PickerRow::new(key, Some(description.to_string()), String::new())
                })
            })
            .collect();
        self.interaction = Interaction::Help(Picker::reference(rows));
    }

    pub(super) fn on_key(&mut self, k: KeyEvent) {
        // A notice is for the keypress that earned it, and no longer.
        self.notice = None;
        if self.quit != QuitState::Running {
            if self.keymap.resolve(BindingContext::Board, &k) == Some(Action::Quit) {
                self.request_quit();
            }
            return;
        }
        if matches!(self.interaction, Interaction::Editing(_)) {
            self.edit_key(k);
            return;
        }
        if matches!(self.interaction, Interaction::ConfirmDelete(_)) {
            // Only "y" deletes; every other key cancels.
            match self.keymap.resolve(BindingContext::Confirm, &k) {
                Some(Action::ConfirmDelete) => self.confirm_delete(),
                _ => self.interaction = Interaction::Idle,
            }
            return;
        }
        if self.interaction.picker().is_some() {
            let context = if self.interaction.picker().is_some_and(Picker::typing) {
                BindingContext::Filter
            } else {
                BindingContext::Picker
            };
            let action = self.keymap.resolve(context, &k);
            let close_on_help =
                matches!(self.interaction, Interaction::Help(_)) && action == Some(Action::Help);
            let outcome = match self.interaction.picker_mut() {
                Some(picker) if !close_on_help => picker_key(picker, action, k),
                _ => PickerKey::Close,
            };
            match outcome {
                PickerKey::Consumed => {}
                PickerKey::Close => self.interaction = Interaction::Idle,
                PickerKey::Accept => match self.interaction {
                    Interaction::Browse(_) => self.open_browse(false),
                    Interaction::Move(_) => self.confirm_move(),
                    Interaction::Parent(_) => self.confirm_parent(),
                    _ => self.interaction = Interaction::Idle,
                },
                PickerKey::AcceptAll => {
                    if matches!(self.interaction, Interaction::Browse(_)) {
                        self.open_browse(true);
                    }
                }
            }
            return;
        }
        let Some(action) = self.keymap.resolve(BindingContext::Board, &k) else {
            return;
        };
        match action {
            Action::Quit => self.request_quit(),
            Action::CloseShelf => self.shelf = Shelf::Closed,
            Action::PreviousList => self.focus = self.focus.saturating_sub(1),
            Action::NextList => {
                self.focus = (self.focus + 1).min(self.columns.len().saturating_sub(1))
            }
            Action::NextTask => self.move_task(1),
            Action::PreviousTask => self.move_task(-1),
            Action::FirstTask => self.move_task(isize::MIN / 2),
            Action::LastTask => self.move_task(isize::MAX / 2),
            Action::ToggleDetails => {
                self.shelf = if !self.shelf.is_open() && self.selected_task().is_some() {
                    Shelf::Open(FieldId::Title)
                } else {
                    Shelf::Closed
                };
            }
            Action::NextField | Action::PreviousField => {
                if let Shelf::Open(field) = self.shelf {
                    self.shelf = Shelf::Open(field.other());
                }
            }
            Action::Edit => self.start_edit(),
            Action::ToggleDone => self.toggle_done(),
            Action::ToggleCompleted => self.toggle_completed(),
            Action::DeleteTask => self.ask_delete(),
            Action::CreateTask => self.start_create(false),
            Action::CreateSubtask => self.start_create(true),
            Action::Refresh => self.refresh(),
            Action::ToggleTransparent => self.transparent = !self.transparent,
            Action::CopyTask => self.yank(),
            Action::BrowseLinks => self.start_browse(),
            Action::MoveTask => self.start_move(),
            Action::ReorderUp => self.reorder_task(false),
            Action::ReorderDown => self.reorder_task(true),
            Action::UnparentTask => self.unparent_task(),
            Action::ChooseParent => self.start_parent_pick(),
            Action::Help => self.start_help(),
            _ => {}
        }
    }

    /// Keys while editing a field. Enter saves, Esc discards; in notes,
    /// Shift+Enter (or Alt+Enter / Ctrl+J where the terminal can't report it)
    /// starts a new line.
    fn edit_key(&mut self, k: KeyEvent) {
        let action = self.keymap.resolve(BindingContext::Editor, &k);
        if self.edit_width == 0 && action != Some(Action::CancelEdit) {
            let cancel = self.keymap.labels_for(Action::CancelEdit).join("/");
            self.error = Some(format!(
                "terminal too small to show the editor; {cancel} cancels"
            ));
            return;
        }
        let notes = self.edit().is_some_and(|e| e.field() == FieldId::Notes);
        match action {
            Some(Action::CancelEdit) => {
                self.interaction = Interaction::Idle;
                return;
            }
            Some(Action::SaveEdit) => {
                self.commit_edit();
                return;
            }
            _ => {}
        }
        let width = self.edit_width.max(1);
        let Some(e) = self.edit_mut() else { return };
        match action {
            Some(Action::InsertNewline) if notes => e.insert('\n'),
            Some(Action::DeleteWord) => e.kill_word(),
            Some(Action::ClearToLineStart) => e.kill_to_start(),
            Some(Action::CursorLineStart) => e.cursor = e.line_start(),
            Some(Action::CursorLineEnd) => e.cursor = e.line_end(),
            Some(Action::EditorBackspace) => e.backspace(),
            Some(Action::EditorDelete) => e.delete(),
            Some(Action::CursorLeft) => e.left(),
            Some(Action::CursorRight) => e.right(),
            Some(Action::CursorUp) if notes => e.step_visual_line(width, false),
            Some(Action::CursorDown) if notes => e.step_visual_line(width, true),
            None if matches!(k.code, KeyCode::Char(_))
                && (no_modifiers(k.modifiers) || k.modifiers == KeyModifiers::SHIFT) =>
            {
                let KeyCode::Char(c) = k.code else {
                    unreachable!()
                };
                e.insert(c)
            }
            _ => {}
        }
    }
}

fn no_modifiers(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty()
}

/// A printable key the user typed, with or without shift held.
fn typed(modifiers: KeyModifiers) -> bool {
    no_modifiers(modifiers) || modifiers == KeyModifiers::SHIFT
}

/// Everything the task links to: the links Google attached to it (a Gmail
/// thread, a Doc) keep the source's title, then any bare URLs typed into the
/// notes. First-seen order, no duplicate URLs.
fn task_links(task: &Task) -> Vec<BrowseLink> {
    let mut out: Vec<BrowseLink> = Vec::new();
    for l in &task.links {
        let name = l.description.trim();
        if l.link.is_empty() || out.iter().any(|b| b.url == l.link) {
            continue;
        }
        out.push(BrowseLink {
            name: (!name.is_empty()).then(|| clean(name).into_owned()),
            url: clean(&l.link).into_owned(),
        });
    }
    for url in urls_in(task.notes.as_deref().unwrap_or("")) {
        if !out.iter().any(|b| b.url == url) {
            out.push(BrowseLink { name: None, url });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybindings::BindingConfig;
    use crate::tui::board::move_requests;
    use crate::tui::render::{modal_rect, task_item};
    use crate::tui::text::ellipsize;
    use chrono::Local;
    use ratatui::layout::Rect;
    use std::borrow::Cow;
    use std::sync::mpsc::{self, Receiver};

    fn app_with_lists(ids: &[&str]) -> (App, Receiver<Cmd>) {
        app_with_lists_and_config(ids, Config::default())
    }

    fn app_with_lists_and_config(ids: &[&str], config: Config) -> (App, Receiver<Cmd>) {
        let (tx, rx) = mpsc::channel();
        let cached = cache::Cache {
            lists: ids
                .iter()
                .map(|id| cache::CachedList {
                    list: TaskList {
                        id: (*id).into(),
                        title: format!("List {id}"),
                        ..Default::default()
                    },
                    tasks: vec![task(&format!("{id}-cached"), None, "1", false)],
                })
                .collect(),
        };
        (App::new(tx, cached, config), rx)
    }

    fn set_tasks(app: &mut App, list_id: &str, tasks: Vec<Task>, selected: &str) {
        app.board.replace_tasks(list_id, tasks);
        app.rebuild_column(list_id);
        let column = app
            .columns
            .iter_mut()
            .find(|column| column.list.id == list_id)
            .unwrap();
        column.select_id(Some(selected.into()));
    }

    fn task(id: &str, parent: Option<&str>, pos: &str, done: bool) -> Task {
        Task {
            id: id.into(),
            parent: parent.map(Into::into),
            position: pos.into(),
            status: if done {
                Status::Completed
            } else {
                Status::NeedsAction
            },
            ..Default::default()
        }
    }

    fn edit(buf: &str) -> Edit {
        Edit {
            target: EditTarget::Existing {
                list_id: "l".into(),
                task_id: "t".into(),
                field: FieldId::Title,
            },
            cursor: buf.len(),
            buf: buf.into(),
        }
    }

    fn new_edit(list_id: &str, selection: Option<&str>, buf: &str) -> Edit {
        Edit {
            target: EditTarget::New {
                list_id: list_id.into(),
                parent: None,
                selection: selection.map(Into::into),
            },
            buf: buf.into(),
            cursor: buf.len(),
        }
    }

    fn pending(app: &mut App, context: MutationContext) -> u64 {
        app.pending_operation = Some(PendingOperation { id: 1, context });
        1
    }

    #[test]
    fn test_apps_never_touch_the_real_config_directory() {
        let (first, _) = app_with_lists(&["a"]);
        let (second, _) = app_with_lists(&["a"]);
        assert!(first.paths.config_dir.starts_with(std::env::temp_dir()));
        assert_ne!(first.paths.config_dir, Paths::from_env().config_dir);
        assert_ne!(first.paths.config_dir, second.paths.config_dir);
    }

    #[test]
    fn edits_title_around_the_cursor() {
        let mut e = edit("Buy milk");
        e.insert('!');
        assert_eq!(e.buf, "Buy milk!");
        e.backspace();
        e.kill_word();
        assert_eq!((e.buf.as_str(), e.cursor), ("Buy ", 4));
        e.insert('é');
        e.left();
        e.delete();
        assert_eq!(e.buf, "Buy ");
        e.kill_to_start();
        assert_eq!((e.buf.as_str(), e.cursor), ("", 0));

        let mut e = edit("e\u{301}x");
        e.left();
        assert_eq!(e.cursor, "e\u{301}".len());
        e.left();
        assert_eq!(e.cursor, 0);
    }

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
        app.on_msg(Msg::Tasks(generation, "b".into(), Err("offline".into())));

        assert_ne!(
            app.sync,
            SyncState::Ready,
            "a partial sync must not enable writes"
        );
        assert!(!app.writable());
        assert_eq!(app.columns[1].rows[0].task.id, "b-cached");
    }

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
    fn navigation_remains_available_while_mutations_are_blocked() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("a-one", None, "1", false),
                task("a-two", None, "2", false),
            ],
            "a-one",
        );

        app.send_patch(
            "a".into(),
            "a-one".into(),
            TaskPatch {
                title: Some("changed".into()),
                ..Default::default()
            },
            task("a-one", None, "1", false),
        );
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        assert_eq!(
            app.selected_task().map(|row| row.task.id.as_str()),
            Some("a-two")
        );
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("current save")));
        assert_eq!(
            cmds.try_iter()
                .filter(|command| matches!(command, Cmd::Patch { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn completion_reorders_immediately_and_selection_follows_its_id() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("a-one", None, "1", false),
                task("a-two", None, "2", false),
            ],
            "a-one",
        );

        app.toggle_done();

        assert_eq!(
            app.columns[0]
                .rows
                .iter()
                .map(|row| row.task.id.as_str())
                .collect::<Vec<_>>(),
            ["a-two", "a-one"]
        );
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("a-one"));
        assert_eq!(app.columns[0].state.selected(), Some(1));
    }

    #[test]
    fn v_toggles_completed_tasks_on_every_list() {
        let (mut app, commands) = app_with_lists(&["a", "b"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("open", None, "1", false),
                task("done-child", Some("open"), "2", true),
                task("done-parent", None, "3", true),
                task("open-child", Some("done-parent"), "4", false),
            ],
            "done-child",
        );
        set_tasks(
            &mut app,
            "b",
            vec![
                task("open-b", None, "1", false),
                task("done-b", None, "2", true),
            ],
            "open-b",
        );
        app.shelf = Shelf::Open(FieldId::Title);

        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));

        assert_eq!(
            shown(&app, "a"),
            [
                ("open".into(), 0),
                ("done-parent".into(), 0),
                ("open-child".into(), 1),
            ]
        );
        assert_eq!(shown(&app, "b"), [("open-b".into(), 0)]);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("done-parent"));
        assert_eq!(app.shelf, Shelf::Closed);
        assert!(app.board.task("a", "done-parent").unwrap().is_done());
        assert!(app.board.task("a", "done-child").unwrap().is_done());

        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));

        assert_eq!(
            shown(&app, "a"),
            [
                ("open".into(), 0),
                ("done-child".into(), 1),
                ("done-parent".into(), 0),
                ("open-child".into(), 1),
            ]
        );
        assert_eq!(
            shown(&app, "b"),
            [("open-b".into(), 0), ("done-b".into(), 0)]
        );
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("done-parent"));
    }

    fn shown(app: &App, list_id: &str) -> Vec<(String, u8)> {
        app.columns
            .iter()
            .find(|column| column.list.id == list_id)
            .unwrap()
            .rows
            .iter()
            .map(|row| (row.task.id.clone(), row.depth))
            .collect()
    }

    #[test]
    fn hidden_completed_parents_keep_their_open_subtasks_in_place() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("top", None, "1", false),
                task("done-parent", None, "0", true),
                task("one", Some("done-parent"), "1", false),
                task("two", Some("done-parent"), "2", false),
                task("done", None, "2", true),
            ],
            "two",
        );

        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(
            shown(&app, "a"),
            [
                ("top".into(), 0),
                ("done-parent".into(), 0),
                ("one".into(), 1),
                ("two".into(), 1),
            ]
        );

        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT));
        assert_eq!(
            shown(&app, "a"),
            [
                ("top".into(), 0),
                ("done-parent".into(), 0),
                ("two".into(), 1),
                ("one".into(), 1),
            ]
        );
        assert!(matches!(
            commands.recv().unwrap(),
            Cmd::Reorder {
                task_id,
                parent: Some(parent),
                previous: None,
                ..
            } if task_id == "two" && parent == "done-parent"
        ));
    }

    #[test]
    fn hidden_completed_subtasks_stay_part_of_their_parent() {
        let (mut app, commands) = app_with_lists(&["a", "b"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("open", None, "1", false),
                task("done-child", Some("open"), "2", true),
                task("other", None, "3", false),
            ],
            "open",
        );
        set_tasks(
            &mut app,
            "b",
            vec![task("there", None, "1", false)],
            "there",
        );
        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));

        app.ask_delete();
        let Interaction::ConfirmDelete(pending) = &app.interaction else {
            panic!("expected delete confirmation");
        };
        assert!(
            pending.prompt.contains("subtask"),
            "hidden completed subtask should still be mentioned: {}",
            pending.prompt
        );
        app.interaction = Interaction::Idle;

        app.start_parent_pick();
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("subtasks")));

        app.start_move();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match commands.recv().unwrap() {
            Cmd::Move { ids, from, to, .. } => {
                assert_eq!(from, "a");
                assert_eq!(to, "b");
                assert_eq!(ids, ["open", "done-child"]);
            }
            _ => panic!("expected move"),
        }
    }

    #[test]
    fn completing_a_task_while_completed_are_hidden_keeps_the_save() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("a-one", None, "1", false),
                task("a-two", None, "2", false),
            ],
            "a-one",
        );
        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        assert_eq!(
            app.columns[0]
                .rows
                .iter()
                .map(|row| row.task.id.as_str())
                .collect::<Vec<_>>(),
            ["a-two"]
        );
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("a-two"));
        assert!(app.board.task("a", "a-one").unwrap().is_done());

        let Cmd::Patch { operation, .. } = commands.recv().unwrap() else {
            panic!("expected patch");
        };
        let mut saved = task("a-one", None, "1", true);
        saved.title = "A one".into();
        app.on_msg(Msg::Updated {
            operation,
            list_id: "a".into(),
            task_id: "a-one".into(),
            result: Ok(saved),
        });

        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.sync, SyncState::Ready);
        assert_eq!(
            app.board.task("a", "a-one").map(|task| task.title.as_str()),
            Some("A one")
        );
        assert_eq!(
            app.columns[0]
                .rows
                .iter()
                .map(|row| row.task.id.as_str())
                .collect::<Vec<_>>(),
            ["a-two"]
        );
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

    fn typed_keys(app: &mut App, keys: &str) {
        for c in keys.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn slash_filters_a_picker_case_insensitively() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.interaction = Interaction::Move(Picker::new(vec![
            PickerRow::new("Inbox".into(), None, "l1".into()),
            PickerRow::new("Tinker Time".into(), None, "l2".into()),
            PickerRow::new("Writing".into(), None, "l3".into()),
        ]));

        // Without a query every row is offered.
        assert_eq!(app.interaction.picker().unwrap().rows().count(), 3);

        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.interaction.picker().unwrap().typing());
        typed_keys(&mut app, "TIN");

        // Substring, not fuzzy, and not anchored: "wriTINg" matches too. Case
        // is folded on both sides.
        let picker = app.interaction.picker().unwrap();
        assert_eq!(picker.query(), "TIN");
        let titles: Vec<&str> = picker.rows().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["Tinker Time", "Writing"]);
        // Selection follows the filter rather than pointing at a hidden row.
        assert_eq!(picker.selected().map(|row| row.value.as_str()), Some("l2"));

        typed_keys(&mut app, "ker");
        let picker = app.interaction.picker().unwrap();
        let titles: Vec<&str> = picker.rows().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["Tinker Time"]);

        // Backspace widens it again.
        for _ in 0..6 {
            app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        assert_eq!(app.interaction.picker().unwrap().rows().count(), 3);
    }

    #[test]
    fn a_filter_that_matches_nothing_has_no_selection_to_act_on() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.interaction = Interaction::Move(Picker::new(vec![PickerRow::new(
            "Inbox".into(),
            None,
            "l1".into(),
        )]));
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        typed_keys(&mut app, "zzz");
        let picker = app.interaction.picker().unwrap();
        assert!(picker.is_empty());
        assert!(picker.selected().is_none());
        // Enter cannot act on nothing, so the overlay simply stays put.
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Move(_)));
    }

    #[test]
    fn letters_type_into_the_query_instead_of_closing_the_picker() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.interaction = Interaction::Move(Picker::new(vec![PickerRow::new(
            "quilting".into(),
            None,
            "l1".into(),
        )]));
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        // `q` and `j` would close or move the list when not filtering.
        typed_keys(&mut app, "qj");
        assert!(matches!(app.interaction, Interaction::Move(_)));
        assert_eq!(app.interaction.picker().unwrap().query(), "qj");

        // Esc leaves the filter but keeps the overlay and the query.
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.interaction.picker().unwrap().typing());
        assert_eq!(app.interaction.picker().unwrap().query(), "qj");
        // A second Esc closes it.
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));
    }

    #[test]
    fn the_keyboard_table_filters_but_never_selects() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.interaction.picker().unwrap().selected().is_none());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        typed_keys(&mut app, "new subtask");
        let picker = app.interaction.picker().unwrap();
        assert_eq!(picker.rows().count(), 1);
        assert_eq!(picker.rows().next().unwrap().title, "shift+c");
        assert!(
            picker.selected().is_none(),
            "the reference card has no action"
        );
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
    fn pending_mutations_prevent_cache_snapshots() {
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
    fn quit_waits_for_writes_and_a_second_quit_forces_it() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                before: Box::new(task("a-cached", None, "1", false)),
            },
        );

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(
            !app.should_exit(),
            "the first quit must wait for the pending write"
        );

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(
            app.should_exit(),
            "the second quit is the explicit force-quit path"
        );
    }

    #[test]
    fn the_waiting_quit_notice_names_the_configured_quit_key() {
        let mut config = Config::default();
        config.keys.quit = Some(BindingConfig::one("ctrl+x"));
        let (mut app, _cmds) = app_with_lists_and_config(&["a"], config);
        pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                before: Box::new(task("a-cached", None, "1", false)),
            },
        );

        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));

        assert!(!app.should_exit());
        assert!(app
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("ctrl+x again to force quit")));
    }

    #[test]
    fn a_waiting_quit_finishes_when_the_last_write_returns() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        let operation = pending(
            &mut app,
            MutationContext::Create(new_edit("a", Some("a-cached"), "draft")),
        );

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.should_exit());
        app.on_msg(Msg::Created {
            operation,
            list_id: "a".into(),
            selection: Some("a-cached".into()),
            result: Err("offline".into()),
        });

        assert!(app.pending_operation.is_none());
        assert!(app.should_exit());
    }

    #[test]
    fn worker_disconnect_does_not_hang_a_waiting_quit() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                before: Box::new(task("a-cached", None, "1", false)),
            },
        );
        app.quit = QuitState::Waiting;

        app.worker_disconnected();

        assert!(app.pending_operation.is_none());
        assert!(app.should_exit());
        assert_eq!(app.sync, SyncState::Failed);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|e| e.contains("worker stopped")));
    }

    #[test]
    fn view_scrolls_to_keep_the_cursor_visible() {
        let mut e = edit("abcdefgh");
        assert_eq!(e.view(4), ("fgh", 3));
        e.cursor = 0;
        assert_eq!(e.view(4), ("abcd", 0));
        assert_eq!(edit("").view(4), ("", 0));
        assert_eq!(edit("任务a").view(3), ("a", 1));
    }

    #[test]
    fn visual_line_navigation_uses_the_same_wrapping_as_rendering() {
        let mut e = edit("abcdef");
        e.cursor = 2;
        e.step_visual_line(3, true);
        assert_eq!(e.cursor, 5);
        e.step_visual_line(3, false);
        assert_eq!(e.cursor, 2);
    }

    #[test]
    fn orders_open_then_done_with_subtasks_under_parents() {
        let rows = order(vec![
            task("done", None, "0", true),
            task("b", None, "2", false),
            task("a1", Some("a"), "1", false),
            task("a", None, "1", false),
            task("orphan", Some("gone"), "3", false),
        ]);
        let got: Vec<(&str, u8)> = rows.iter().map(|r| (r.task.id.as_str(), r.depth)).collect();
        assert_eq!(
            got,
            [("a", 0), ("a1", 1), ("b", 0), ("orphan", 0), ("done", 0)]
        );
    }

    #[test]
    fn urls_in_finds_http_and_https_and_skips_the_rest() {
        assert_eq!(urls_in(""), Vec::<String>::new());
        assert_eq!(urls_in("no links here"), Vec::<String>::new());
        assert_eq!(
            urls_in("see https://example.com/a"),
            vec!["https://example.com/a"]
        );
        assert_eq!(
            urls_in("http://a.test https://b.test/x"),
            vec!["http://a.test", "https://b.test/x"]
        );
        assert_eq!(
            urls_in("read [PR](https://github.com/org/repo/pull/1)."),
            vec!["https://github.com/org/repo/pull/1"]
        );
        assert_eq!(urls_in("visit https://ex.com."), vec!["https://ex.com"]);
        assert_eq!(urls_in("www.example.com"), Vec::<String>::new());
        assert_eq!(
            urls_in("https://dup.test then https://dup.test again"),
            vec!["https://dup.test"]
        );
    }

    #[test]
    fn task_links_lists_attached_links_then_notes_urls() {
        use crate::api::Link;
        let link = |d: &str, u: &str| Link {
            description: d.into(),
            link: u.into(),
        };
        let task = Task {
            notes: Some("see https://notes.test/a and https://mail.test/#all/1".into()),
            links: vec![
                link("Check In For Your Visit", "https://mail.test/#all/1"),
                link("  ", "https://nameless.test"),
                link("same url again", "https://mail.test/#all/1"),
                link("no url at all", ""),
            ],
            ..Default::default()
        };
        let links = task_links(&task);
        let got: Vec<(Option<&str>, &str)> = links
            .iter()
            .map(|l| (l.name.as_deref(), l.url.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                // Google's own link keeps the source's title and comes first.
                (Some("Check In For Your Visit"), "https://mail.test/#all/1"),
                (None, "https://nameless.test"),
                // The same URL in the notes is not offered twice.
                (None, "https://notes.test/a"),
            ]
        );
        assert!(task_links(&Task::default()).is_empty());
    }

    #[test]
    fn task_titles_wrap_only_when_asked() {
        let t = Theme::default();
        let today = Local::now().date_naive();
        let long = Task {
            title: "Extend irrigation to the raised beds before the frost".into(),
            ..task("a", None, "1", false)
        };
        let rows = order(vec![
            long.clone(),
            Task {
                parent: Some("a".into()),
                ..long
            },
        ]);
        // Clipped: one line per task, however long the title is.
        assert_eq!(task_item(&rows[0], &t, today, None).height(), 1);
        // Wrapped: a 53-character title needs several lines in a 20-wide column.
        let top = task_item(&rows[0], &t, today, Some(20)).height();
        assert!(top > 2, "expected the title to wrap, got {top} line(s)");
        // A subtask is indented, so it has less room and needs at least as many.
        assert!(task_item(&rows[1], &t, today, Some(20)).height() >= top);
        // A width narrower than the indent and marker still renders.
        assert!(task_item(&rows[1], &t, today, Some(1)).height() >= 1);
    }

    #[test]
    fn clean_swaps_control_characters_without_moving_offsets() {
        assert!(matches!(clean("plain text"), Cow::Borrowed(_)));
        // Newlines survive: wrap() splits lines on them.
        assert!(matches!(clean("two\nlines"), Cow::Borrowed(_)));
        assert_eq!(clean("print \r and bring"), "print   and bring");
        assert_eq!(clean("bell\u{7}tab\u{8}"), "bell tab ");
        // Byte offsets have to survive, or wrap()/cursor_rc() slice mid-char.
        for raw in ["print \r and", "a\u{7}b\u{1b}c", "\r\n\r\n", "héllo\rwörld"] {
            assert_eq!(clean(raw).len(), raw.len(), "{raw:?}");
        }
    }

    #[test]
    fn a_yank_carries_the_links_and_drops_the_bookkeeping() {
        let task = Task {
            id: "t1".into(),
            title: "Check In For Yearly Checkup".into(),
            notes: Some("Need my vax\r\n".into()),
            status: Status::NeedsAction,
            due: chrono::NaiveDate::from_ymd_opt(2026, 9, 25),
            parent: Some("p9".into()),
            links: vec![crate::api::Link {
                description: "Check In For Your Upcoming Visit".into(),
                link: "https://mail.google.com/mail/#all/1a0".into(),
            }],
            ..Default::default()
        };
        let md = md_yank(&task);
        assert_eq!(
            md,
            "# Check In For Yearly Checkup\n\nDue: 2026-09-25\n\nNeed my vax\n\n\
             - [Check In For Your Upcoming Visit](<https://mail.google.com/mail/#all/1a0>)\n"
        );
        // The things that identify a task to the API have no place in a note.
        for noise in ["t1", "needsAction", "p9", "id:", "status:", "parent:"] {
            assert!(!md.contains(noise), "{noise:?} is still in {md:?}");
        }
    }

    #[test]
    fn ellipsize_cuts_on_character_boundaries() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("exactfit", 8), "exactfit");
        assert_eq!(ellipsize("truncate me", 8), "truncat…");
        assert_eq!(ellipsize("héllo wörld", 6), "héllo…");
        assert_eq!(ellipsize("abc", 0), "…");
    }

    #[test]
    fn cursor_rc_places_the_cursor_at_the_end_of_a_wrapped_line() {
        let text = "aaa\nbbb";
        let segs = wrap(text, 10);
        assert_eq!(cursor_rc(&segs, text, 1), (0, 1));
        // End of a line that is not the last one: no segment contains it.
        assert_eq!(cursor_rc(&segs, text, 3), (0, 3));
        assert_eq!(cursor_rc(&segs, text, 4), (1, 0));
        assert_eq!(cursor_rc(&segs, text, 7), (1, 3));

        // A soft wrap shares the boundary, so the cursor belongs to the next row.
        let soft = "abcdefgh";
        let segs = wrap(soft, 4);
        assert_eq!(cursor_rc(&segs, soft, 4), (1, 0));

        // Trailing newline, and an empty buffer.
        let trail = "ab\n";
        let segs = wrap(trail, 10);
        assert_eq!(cursor_rc(&segs, trail, 3), (1, 0));
        assert_eq!(cursor_rc(&wrap("", 10), "", 0), (0, 0));

        let wide = "任务ab";
        let segments = wrap(wide, 3);
        assert_eq!(
            segments
                .iter()
                .map(|(start, end)| &wide[*start..*end])
                .collect::<Vec<_>>(),
            ["任", "务a", "b"]
        );
        assert_eq!(cursor_rc(&segments, wide, "任务".len()), (1, 2));
    }

    #[test]
    fn other_lists_skip_the_current_one() {
        let lists = [
            TaskList {
                id: "a".into(),
                title: "Inbox".into(),
                ..Default::default()
            },
            TaskList {
                id: "b".into(),
                title: "Work".into(),
                ..Default::default()
            },
            TaskList {
                id: "c".into(),
                title: "Home".into(),
                ..Default::default()
            },
        ];
        let got: Vec<&str> = other_lists(&lists, "b")
            .iter()
            .map(|l| l.id.as_str())
            .collect();
        assert_eq!(got, ["a", "c"]);
        assert!(other_lists(&lists, "missing")
            .iter()
            .map(|l| l.id.as_str())
            .eq(["a", "b", "c"]));
    }

    #[test]
    fn move_ids_are_the_task_and_its_subtasks() {
        let rows = order(vec![
            task("a", None, "1", false),
            task("a1", Some("a"), "1", false),
            task("b", None, "2", false),
        ]);
        assert_eq!(move_ids(&rows, "a"), ["a", "a1"]);
        assert_eq!(move_ids(&rows, "a1"), ["a1"]);
        assert!(move_ids(&rows, "gone").is_empty());
    }

    #[test]
    fn move_requests_preserve_subtask_sibling_order() {
        let ids = vec!["parent".into(), "first".into(), "second".into()];
        let requests = move_requests("destination", &ids);

        assert_eq!(requests[0].1.parent, None);
        assert_eq!(requests[0].1.previous, None);
        assert_eq!(requests[1].1.parent.as_deref(), Some("parent"));
        assert_eq!(requests[1].1.previous, None);
        assert_eq!(requests[2].1.parent.as_deref(), Some("parent"));
        assert_eq!(requests[2].1.previous.as_deref(), Some("first"));
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

    #[test]
    fn assigned_tasks_cannot_move_or_edit_notes() {
        let (mut app, _cmds) = app_with_lists(&["a", "b"]);
        app.sync = SyncState::Ready;
        let assigned = Task {
            assignment_info: Some(crate::api::AssignmentInfo {
                link_to_task: "https://docs.google.com/document/d/1".into(),
                surface_type: "DOCUMENT".into(),
            }),
            ..task("assigned", None, "1", false)
        };
        set_tasks(&mut app, "a", vec![assigned], "assigned");

        app.start_move();
        assert!(matches!(app.interaction, Interaction::Idle));
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cannot move")));

        app.error = None;
        app.shelf = Shelf::Open(FieldId::Notes);
        app.start_edit();
        assert!(matches!(app.interaction, Interaction::Idle));
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cannot have notes")));
    }

    #[test]
    fn editing_after_closing_the_shelf_on_notes_starts_at_the_title() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;
        let assigned = Task {
            assignment_info: Some(crate::api::AssignmentInfo {
                link_to_task: "https://docs.google.com/document/d/1".into(),
                surface_type: "DOCUMENT".into(),
            }),
            ..task("assigned", None, "1", false)
        };
        set_tasks(&mut app, "a", vec![assigned], "assigned");

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.edit().map(Edit::field), Some(FieldId::Title));
        assert_eq!(app.shelf, Shelf::Open(FieldId::Title));
    }

    #[test]
    fn less_than_unparents_the_selected_task() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("parent", None, "1", false),
                task("child", Some("parent"), "2", false),
            ],
            "child",
        );

        // Some enhanced terminals report the shifted symbol with no modifier.
        app.on_key(KeyEvent::new(KeyCode::Char('<'), KeyModifiers::NONE));

        assert_eq!(app.board.task("a", "child").unwrap().parent, None);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("child"));
        assert!(matches!(
            commands.recv().unwrap(),
            Cmd::Reparent {
                list_id,
                task_id,
                parent: None,
                previous: Some(previous),
                ..
            } if list_id == "a" && task_id == "child" && previous == "parent"
        ));
    }

    fn ids(app: &App, list_id: &str) -> Vec<String> {
        app.columns
            .iter()
            .find(|column| column.list.id == list_id)
            .unwrap()
            .rows
            .iter()
            .map(|row| row.task.id.clone())
            .collect()
    }

    #[test]
    fn reorder_steps_target_open_siblings_under_the_same_parent() {
        let rows = order(vec![
            task("a", None, "1", false),
            task("a1", Some("a"), "1", false),
            task("a2", Some("a"), "2", false),
            task("a3", Some("a"), "3", false),
            task("b", None, "2", false),
            task("b1", Some("b"), "1", false),
            task("c", None, "3", false),
            task("done", None, "0", true),
        ]);
        let shift = |id: &str, down: bool| {
            reorder_step(&rows, id, down).map(|plan| (plan.parent, plan.previous))
        };
        let ok = |parent: Option<&str>, previous: Option<&str>| {
            Ok((parent.map(Into::into), previous.map(Into::into)))
        };

        // Top-level tasks skip over subtasks and land after their neighbor.
        assert_eq!(shift("a", true), ok(None, Some("b")));
        assert_eq!(shift("c", false), ok(None, Some("a")));
        assert_eq!(shift("b", false), ok(None, None));
        // Subtasks only trade places under their own parent.
        assert_eq!(shift("a3", false), ok(Some("a"), Some("a1")));
        assert_eq!(shift("a2", false), ok(Some("a"), None));
        assert_eq!(shift("a1", true), ok(Some("a"), Some("a2")));
        // Edges, lone subtasks, and completed tasks don't move.
        assert_eq!(shift("a", false), Err("task is already first"));
        assert_eq!(shift("c", true), Err("task is already last"));
        assert_eq!(shift("a3", true), Err("task is already last"));
        assert_eq!(shift("b1", false), Err("task is already first"));
        assert_eq!(
            shift("done", false),
            Err("completed tasks stay in completion order")
        );
    }

    #[test]
    fn reorder_after_hands_sibling_positions_out_in_the_new_order() {
        let rows = order(vec![
            task("a", None, "1", false),
            task("b", None, "2", false),
            task("c", None, "3", false),
            task("d", None, "4", false),
        ]);
        let positions = |id: &str, previous: Option<&str>| {
            reorder_after(&rows, id, previous).map(|plan| plan.positions)
        };
        let moved = |pairs: &[(&str, &str)]| {
            Ok(pairs
                .iter()
                .map(|(id, position)| (id.to_string(), position.to_string()))
                .collect::<Vec<_>>())
        };

        assert_eq!(
            positions("a", Some("c")),
            moved(&[("b", "1"), ("c", "2"), ("a", "3")])
        );
        assert_eq!(
            positions("d", None),
            moved(&[("d", "1"), ("a", "2"), ("b", "3"), ("c", "4")])
        );
        assert_eq!(positions("b", Some("a")), moved(&[]));
        assert_eq!(
            positions("a", Some("gone")),
            Err("its new neighbor is no longer beside it")
        );
    }

    #[test]
    fn alt_j_swaps_with_the_next_sibling_and_keeps_the_selection() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("first", None, "1", false),
                task("child", Some("first"), "1", false),
                task("second", None, "2", false),
                task("third", None, "3", false),
            ],
            "first",
        );

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));

        assert_eq!(ids(&app, "a"), ["second", "first", "child", "third"]);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("first"));
        let Cmd::Reorder {
            operation,
            list_id,
            task_id,
            parent,
            previous,
        } = commands.recv().unwrap()
        else {
            panic!("expected a same-list move");
        };
        assert_eq!(
            (
                list_id.as_str(),
                task_id.as_str(),
                parent,
                previous.as_deref()
            ),
            ("a", "first", None, Some("second"))
        );

        // Google renumbers siblings as dense counters: the moved task takes
        // the neighbor's old slot and the neighbor is bumped. The reloaded
        // list replaces the local guess, so the row stays where it moved.
        app.on_msg(Msg::Reordered {
            operation,
            list_id: "a".into(),
            task_id: "first".into(),
            parent: None,
            result: Ok(task("first", None, "1", false)),
            tasks: Some(Ok(vec![
                task("second", None, "0", false),
                task("first", None, "1", false),
                task("child", Some("first"), "0", false),
                task("third", None, "2", false),
            ])),
        });
        assert_eq!(app.board.task("a", "second").unwrap().position, "0");
        assert_eq!(app.board.task("a", "first").unwrap().position, "1");
        assert_eq!(ids(&app, "a"), ["second", "first", "child", "third"]);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("first"));
        assert_eq!(app.sync, SyncState::Ready);
        assert!(app.pending_operation.is_none());
    }

    #[test]
    fn alt_k_moves_a_subtask_up_among_its_siblings() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("parent", None, "1", false),
                task("one", Some("parent"), "1", false),
                task("two", Some("parent"), "2", false),
            ],
            "two",
        );

        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT));

        assert_eq!(ids(&app, "a"), ["parent", "two", "one"]);
        assert!(matches!(
            commands.recv().unwrap(),
            Cmd::Reorder {
                task_id,
                parent: Some(parent),
                previous: None,
                ..
            } if task_id == "two" && parent == "parent"
        ));
    }

    #[test]
    fn failed_reorder_restores_both_tasks_and_enters_recovery() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("first", None, "1", false),
                task("second", None, "2", false),
            ],
            "second",
        );

        app.reorder_task(false);
        assert_eq!(ids(&app, "a"), ["second", "first"]);
        let Cmd::Reorder { operation, .. } = commands.recv().unwrap() else {
            panic!("expected a same-list move");
        };
        app.on_msg(Msg::Reordered {
            operation,
            list_id: "a".into(),
            task_id: "second".into(),
            parent: None,
            result: Err("offline".into()),
            tasks: None,
        });

        assert_eq!(ids(&app, "a"), ["first", "second"]);
        assert_eq!(app.board.task("a", "first").unwrap().position, "1");
        assert_eq!(app.board.task("a", "second").unwrap().position, "2");
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("second"));
        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("reorder failed")));
    }

    #[test]
    fn reorder_saved_but_not_reloaded_keeps_the_move_and_recovers() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("first", None, "0", false),
                task("second", None, "1", false),
            ],
            "second",
        );

        app.reorder_task(false);
        let Cmd::Reorder { operation, .. } = commands.recv().unwrap() else {
            panic!("expected a same-list move");
        };
        app.on_msg(Msg::Reordered {
            operation,
            list_id: "a".into(),
            task_id: "second".into(),
            parent: None,
            result: Ok(task("second", None, "0", false)),
            tasks: Some(Err("timeout".into())),
        });

        assert_eq!(ids(&app, "a"), ["second", "first"]);
        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("reorder saved")));
    }

    #[test]
    fn reorder_at_an_edge_reports_and_sends_nothing() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("first", None, "1", false),
                task("second", None, "2", false),
            ],
            "first",
        );

        app.reorder_task(false);

        assert_eq!(app.error.as_deref(), Some("task is already first"));
        assert!(commands.try_recv().is_err());
        assert!(app.pending_operation.is_none());
    }

    fn reordered(operation: u64, task_id: &str, list: &[&str]) -> Msg {
        let tasks: Vec<Task> = list
            .iter()
            .enumerate()
            .map(|(index, id)| task(id, None, &index.to_string(), false))
            .collect();
        let moved = tasks
            .iter()
            .find(|task| task.id == task_id)
            .unwrap()
            .clone();
        Msg::Reordered {
            operation,
            list_id: "a".into(),
            task_id: task_id.into(),
            parent: None,
            result: Ok(moved),
            tasks: Some(Ok(tasks)),
        }
    }

    fn reorder_list(app: &mut App, ids: &[&str]) {
        let tasks = ids
            .iter()
            .enumerate()
            .map(|(index, id)| task(id, None, &index.to_string(), false))
            .collect();
        set_tasks(app, "a", tasks, ids[0]);
    }

    #[test]
    fn reorder_presses_while_saving_become_one_follow_up_move() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        reorder_list(&mut app, &["a", "b", "c", "d"]);
        let down = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT);

        app.on_key(down);
        let Cmd::Reorder {
            operation,
            previous,
            ..
        } = commands.recv().unwrap()
        else {
            panic!("expected a same-list move");
        };
        assert_eq!(previous.as_deref(), Some("b"));

        app.on_key(down);
        app.on_key(down);
        assert!(commands.try_recv().is_err(), "presses wait for the save");
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(ids(&app, "a"), ["b", "c", "d", "a"]);

        app.on_msg(reordered(operation, "a", &["b", "a", "c", "d"]));
        assert_eq!(ids(&app, "a"), ["b", "c", "d", "a"]);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("a"));
        let Cmd::Reorder {
            operation,
            task_id,
            previous,
            ..
        } = commands.recv().unwrap()
        else {
            panic!("expected the queued presses as one move");
        };
        assert_eq!((task_id.as_str(), previous.as_deref()), ("a", Some("d")));

        app.on_msg(reordered(operation, "a", &["b", "c", "d", "a"]));
        assert!(commands.try_recv().is_err());
        assert_eq!(ids(&app, "a"), ["b", "c", "d", "a"]);
        assert!(app.pending_operation.is_none());
        assert_eq!(app.sync, SyncState::Ready);
    }

    #[test]
    fn queued_reorder_presses_that_cancel_out_still_land_where_shown() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        reorder_list(&mut app, &["a", "b", "c"]);

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        let Cmd::Reorder { operation, .. } = commands.recv().unwrap() else {
            panic!("expected a same-list move");
        };
        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT));
        assert_eq!(ids(&app, "a"), ["a", "b", "c"]);

        app.on_msg(reordered(operation, "a", &["b", "a", "c"]));
        assert_eq!(ids(&app, "a"), ["a", "b", "c"]);
        assert!(matches!(
            commands.recv().unwrap(),
            Cmd::Reorder { task_id, previous: None, .. } if task_id == "a"
        ));
    }

    #[test]
    fn a_failed_reorder_drops_queued_presses_and_restores_the_list() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        reorder_list(&mut app, &["a", "b", "c"]);

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        let Cmd::Reorder { operation, .. } = commands.recv().unwrap() else {
            panic!("expected a same-list move");
        };
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        assert_eq!(ids(&app, "a"), ["b", "c", "a"]);

        app.on_msg(Msg::Reordered {
            operation,
            list_id: "a".into(),
            task_id: "a".into(),
            parent: None,
            result: Err("offline".into()),
            tasks: None,
        });

        assert_eq!(ids(&app, "a"), ["a", "b", "c"]);
        assert!(matches!(commands.recv().unwrap(), Cmd::Refresh(_)));
        assert!(commands.try_recv().is_err());
        assert!(app.pending_operation.is_none());
    }

    #[test]
    fn a_waiting_quit_also_waits_for_queued_reorder_presses() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        reorder_list(&mut app, &["a", "b", "c"]);

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        let Cmd::Reorder { operation, .. } = commands.recv().unwrap() else {
            panic!("expected a same-list move");
        };
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.should_exit());

        app.on_msg(reordered(operation, "a", &["b", "a", "c"]));
        assert!(
            !app.should_exit(),
            "the queued move is still a pending write"
        );
        let Cmd::Reorder { operation, .. } = commands.recv().unwrap() else {
            panic!("expected the queued presses as one move");
        };

        app.on_msg(reordered(operation, "a", &["b", "c", "a"]));
        assert!(app.should_exit());
    }

    #[test]
    fn failed_reparent_rolls_back_and_enters_recovery() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("parent", None, "1", false),
                task("child", Some("parent"), "2", false),
            ],
            "child",
        );

        app.unparent_task();
        let Cmd::Reparent { operation, .. } = commands.recv().unwrap() else {
            panic!("expected reparent command");
        };
        app.on_msg(Msg::Reparented {
            operation,
            list_id: "a".into(),
            task_id: "child".into(),
            parent: None,
            result: Err("offline".into()),
        });

        assert_eq!(
            app.board
                .task("a", "child")
                .and_then(|task| task.parent.as_deref()),
            Some("parent")
        );
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("child"));
        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("reparent failed")));
    }

    #[test]
    fn greater_than_lists_only_available_parents_and_reparents() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        let assigned = Task {
            assignment_info: Some(crate::api::AssignmentInfo {
                link_to_task: "https://docs.google.com/document/d/1".into(),
                surface_type: "DOCUMENT".into(),
            }),
            ..task("assigned", None, "5", false)
        };
        let hidden = Task {
            hidden: true,
            ..task("hidden", None, "6", false)
        };
        set_tasks(
            &mut app,
            "a",
            vec![
                task("selected", None, "1", false),
                task("available", None, "2", false),
                task("completed", None, "3", true),
                task("subtask", Some("available"), "4", false),
                assigned,
                hidden,
            ],
            "selected",
        );

        // Other terminals retain the base key and report Shift separately.
        app.on_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::SHIFT));
        let Interaction::Parent(picker) = &app.interaction else {
            panic!("expected parent picker");
        };
        assert_eq!(
            picker
                .picker
                .rows()
                .map(|row| row.value.as_str())
                .collect::<Vec<_>>(),
            ["available"]
        );

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.board
                .task("a", "selected")
                .and_then(|task| task.parent.as_deref()),
            Some("available")
        );
        assert!(matches!(
            commands.recv().unwrap(),
            Cmd::Reparent {
                list_id,
                task_id,
                parent: Some(parent),
                ..
            } if list_id == "a" && task_id == "selected" && parent == "available"
        ));
    }

    #[test]
    fn tasks_with_children_or_completed_hidden_tasks_cannot_become_subtasks() {
        for selected in ["has-children", "completed-hidden"] {
            let (mut app, _commands) = app_with_lists(&["a"]);
            app.sync = SyncState::Ready;
            set_tasks(
                &mut app,
                "a",
                vec![
                    task("has-children", None, "1", false),
                    task("child", Some("has-children"), "2", false),
                    Task {
                        hidden: true,
                        ..task("completed-hidden", None, "3", true)
                    },
                    task("available", None, "4", false),
                ],
                selected,
            );

            app.start_parent_pick();

            assert!(matches!(app.interaction, Interaction::Idle));
            assert!(app
                .error
                .as_deref()
                .is_some_and(|error| { error.contains("cannot become a subtask") }));
        }
    }

    #[test]
    fn help_keys_lists_the_board_commands() {
        let (mut app, _commands) = app_with_lists(&["a"]);
        app.start_help();
        let picker = app.interaction.picker().unwrap();
        for need in [
            "previous list",
            "next task",
            "new task",
            "new subtask",
            "delete task",
            "move to another list",
            "choose parent",
            "show / hide completed",
            "keyboard commands",
            "quit",
        ] {
            assert!(
                picker
                    .rows()
                    .any(|row| row.subtitle.as_deref() == Some(need)),
                "missing {need}"
            );
        }
        assert!(picker
            .rows()
            .all(|row| !row.title.is_empty() && row.subtitle.is_some()));
    }

    #[test]
    fn modified_shortcuts_do_not_trigger_plain_commands() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(matches!(app.interaction, Interaction::Idle));

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.should_exit());
    }

    #[test]
    fn configured_board_binding_replaces_its_default() {
        let mut config = Config::default();
        config.keys.create_task = Some(BindingConfig::one("n"));
        let (mut app, _commands) = app_with_lists_and_config(&["a"], config);
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));

        app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Editing(_)));
    }

    #[test]
    fn empty_configured_binding_disables_the_action() {
        let mut config = Config::default();
        config.keys.delete_task = Some(BindingConfig::empty());
        let (mut app, _commands) = app_with_lists_and_config(&["a"], config);
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));
    }

    #[test]
    fn configured_modified_chord_dispatches_directly() {
        let mut config = Config::default();
        config.keys.create_task = Some(BindingConfig::one("ctrl+b"));
        let (mut app, _commands) = app_with_lists_and_config(&["a"], config);
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(matches!(app.interaction, Interaction::Editing(_)));
    }

    #[test]
    fn picker_and_editor_actions_use_their_contextual_bindings() {
        let mut config = Config::default();
        config.keys.close_picker = Some(BindingConfig::one("x"));
        config.keys.save_edit = Some(BindingConfig::one("ctrl+s"));
        let (mut app, _commands) = app_with_lists_and_config(&["a"], config);

        app.interaction = Interaction::Move(Picker::new(vec![PickerRow::new(
            "Inbox".into(),
            None,
            "l1".into(),
        )]));
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Move(_)));
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));

        app.sync = SyncState::Ready;
        app.edit_width = 20;
        app.interaction = Interaction::Editing(new_edit("a", None, "draft"));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Editing(_)));
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(matches!(app.interaction, Interaction::Idle));
    }

    #[test]
    fn escape_never_quits_from_the_main_board() {
        let (mut app, _commands) = app_with_lists(&["a"]);

        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.should_exit());

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.should_exit());
    }

    #[test]
    fn question_mark_works_with_both_terminal_key_reports() {
        for event in [
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::SHIFT),
        ] {
            let (mut app, _commands) = app_with_lists(&["a"]);
            app.on_key(event);
            assert!(matches!(app.interaction, Interaction::Help(_)));

            app.on_key(event);
            assert!(matches!(app.interaction, Interaction::Idle));
        }
    }

    #[test]
    fn keyboard_overlay_uses_active_bindings_and_omits_disabled_actions() {
        let mut config = Config::default();
        config.keys.help = Some(BindingConfig::one("f1"));
        config.keys.create_task = Some(BindingConfig::one("n"));
        config.keys.delete_task = Some(BindingConfig::empty());
        let (mut app, _commands) = app_with_lists_and_config(&["a"], config);

        app.on_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        let Interaction::Help(picker) = &app.interaction else {
            panic!("expected help");
        };
        assert!(picker
            .rows()
            .any(|row| { row.title == "n" && row.subtitle.as_deref() == Some("new task") }));
        assert!(!picker
            .rows()
            .any(|row| { row.subtitle.as_deref() == Some("delete task") }));
    }

    #[test]
    fn invalid_keybinding_diagnostics_survive_startup_refresh() {
        let mut config = Config::default();
        config.keys.help = Some(BindingConfig::one("not-a-key"));
        let (app, _commands) = app_with_lists_and_config(&["a"], config);

        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("invalid keybinding")));
    }

    #[test]
    fn modal_rect_never_exceeds_the_terminal() {
        let area = Rect::new(3, 4, 12, 2);
        let modal = modal_rect(area, 80, 30);
        assert_eq!(modal, area);
    }

    #[test]
    fn subtask_parent_is_the_row_or_its_own_parent() {
        let rows = order(vec![
            task("a", None, "1", false),
            task("a1", Some("a"), "1", false),
            task("b", None, "2", false),
            task("orphan", Some("gone"), "3", false),
        ]);
        let row = |id: &str| rows.iter().find(|r| r.task.id == id).unwrap();
        assert_eq!(subtask_parent(row("a")).as_deref(), Some("a"));
        assert_eq!(subtask_parent(row("b")).as_deref(), Some("b"));
        // A subtask hands over its own parent, so nesting stays one deep.
        assert_eq!(subtask_parent(row("a1")).as_deref(), Some("a"));
        // A task whose parent is missing from the list draws at depth 0, so it
        // parents the new subtask itself instead of pointing at a stranger.
        assert_eq!(subtask_parent(row("orphan")).as_deref(), Some("orphan"));
    }
}
