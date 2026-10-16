use std::collections::HashMap;

use crate::{
    api::{Task, TaskList},
    cache,
};

#[derive(Clone)]
pub struct Row {
    pub task: Task,
    pub depth: u8,
}

#[derive(Clone, Default)]
pub struct Board {
    list_order: Vec<String>,
    lists: HashMap<String, TaskList>,
    task_order: HashMap<String, Vec<String>>,
    tasks: HashMap<String, HashMap<String, Task>>,
}

impl Board {
    pub fn from_cache(cache: cache::Cache) -> Self {
        let mut board = Self::default();
        for cached in cache.lists {
            let (order, tasks) = keyed(cached.tasks);
            board.list_order.push(cached.list.id.clone());
            board.task_order.insert(cached.list.id.clone(), order);
            board.tasks.insert(cached.list.id.clone(), tasks);
            board.lists.insert(cached.list.id.clone(), cached.list);
        }
        board
    }

    pub fn snapshot(&self) -> cache::Cache {
        cache::Cache {
            lists: self
                .list_order
                .iter()
                .filter_map(|id| {
                    Some(cache::CachedList {
                        list: self.lists.get(id)?.clone(),
                        tasks: self.rows(id).into_iter().map(|row| row.task).collect(),
                    })
                })
                .collect(),
        }
    }

    pub fn rows(&self, list_id: &str) -> Vec<Row> {
        ordered(self.tasks(list_id))
    }

    pub fn lists(&self) -> impl Iterator<Item = &TaskList> {
        self.list_order.iter().filter_map(|id| self.lists.get(id))
    }

    pub fn list(&self, list_id: &str) -> Option<&TaskList> {
        self.lists.get(list_id)
    }

    pub fn task(&self, list_id: &str, task_id: &str) -> Option<&Task> {
        self.tasks.get(list_id)?.get(task_id)
    }

    pub fn task_mut(&mut self, list_id: &str, task_id: &str) -> Option<&mut Task> {
        self.tasks.get_mut(list_id)?.get_mut(task_id)
    }

    pub fn tasks(&self, list_id: &str) -> Vec<Task> {
        let Some(tasks) = self.tasks.get(list_id) else {
            return Vec::new();
        };
        let mut output: Vec<Task> = self
            .task_order
            .get(list_id)
            .into_iter()
            .flatten()
            .filter_map(|id| tasks.get(id))
            .cloned()
            .collect();
        let mut extras: Vec<&Task> = tasks
            .values()
            .filter(|task| {
                !self
                    .task_order
                    .get(list_id)
                    .is_some_and(|ids| ids.contains(&task.id))
            })
            .collect();
        extras.sort_by(|left, right| left.id.cmp(&right.id));
        output.extend(extras.into_iter().cloned());
        output
    }

    pub fn replace_lists(&mut self, lists: Vec<TaskList>) {
        let ids: Vec<String> = lists.iter().map(|list| list.id.clone()).collect();
        self.lists = lists
            .into_iter()
            .map(|list| (list.id.clone(), list))
            .collect();
        self.tasks.retain(|id, _| self.lists.contains_key(id));
        self.task_order.retain(|id, _| self.lists.contains_key(id));
        for id in &ids {
            self.tasks.entry(id.clone()).or_default();
            self.task_order.entry(id.clone()).or_default();
        }
        self.list_order = ids;
    }

    pub fn upsert_list(&mut self, list: TaskList) {
        if !self.lists.contains_key(&list.id) {
            self.list_order.push(list.id.clone());
            self.tasks.insert(list.id.clone(), HashMap::new());
            self.task_order.insert(list.id.clone(), Vec::new());
        }
        self.lists.insert(list.id.clone(), list);
    }

    pub fn remove_list(&mut self, list_id: &str) {
        self.list_order.retain(|id| id != list_id);
        self.lists.remove(list_id);
        self.task_order.remove(list_id);
        self.tasks.remove(list_id);
    }

    pub fn replace_tasks(&mut self, list_id: &str, tasks: Vec<Task>) {
        let (order, tasks) = keyed(tasks);
        self.task_order.insert(list_id.to_string(), order);
        self.tasks.insert(list_id.to_string(), tasks);
    }

    pub fn upsert_task(&mut self, list_id: &str, task: Task) -> bool {
        let Some(tasks) = self.tasks.get_mut(list_id) else {
            return false;
        };
        if !tasks.contains_key(&task.id) {
            self.task_order
                .entry(list_id.to_string())
                .or_default()
                .push(task.id.clone());
        }
        tasks.insert(task.id.clone(), task);
        true
    }

    pub fn remove_tree(&mut self, list_id: &str, task_id: &str) -> Vec<Task> {
        let source_order = self.task_order.get(list_id).cloned().unwrap_or_default();
        let Some(tasks) = self.tasks.get_mut(list_id) else {
            return Vec::new();
        };
        let ids: Vec<String> = source_order
            .into_iter()
            .filter(|id| {
                tasks.get(id).is_some_and(|task| {
                    task.id == task_id || task.parent.as_deref() == Some(task_id)
                })
            })
            .collect();
        if let Some(order) = self.task_order.get_mut(list_id) {
            order.retain(|id| !ids.contains(id));
        }
        ids.into_iter().filter_map(|id| tasks.remove(&id)).collect()
    }

    pub fn restore(&mut self, list_id: &str, tasks: Vec<Task>) {
        let Some(current) = self.tasks.get_mut(list_id) else {
            return;
        };
        let order = self.task_order.entry(list_id.to_string()).or_default();
        for task in tasks {
            if !current.contains_key(&task.id) {
                order.push(task.id.clone());
            }
            current.insert(task.id.clone(), task);
        }
    }

    pub fn move_tree(&mut self, from: &str, to: &str, task_id: &str) -> Vec<Task> {
        if !self.tasks.contains_key(to) {
            return Vec::new();
        }
        let moved = self.remove_tree(from, task_id);
        let moved_ids: std::collections::HashSet<String> =
            moved.iter().map(|task| task.id.clone()).collect();
        let moved: Vec<Task> = moved
            .into_iter()
            .map(|mut task| {
                if task
                    .parent
                    .as_deref()
                    .is_some_and(|parent| !moved_ids.contains(parent))
                {
                    task.parent = None;
                }
                task
            })
            .collect();
        self.restore(to, moved.clone());
        moved
    }
}

fn keyed(tasks: Vec<Task>) -> (Vec<String>, HashMap<String, Task>) {
    let tasks: Vec<Task> = tasks.into_iter().filter(|task| !task.deleted).collect();
    let order = tasks.iter().map(|task| task.id.clone()).collect();
    let tasks = tasks
        .into_iter()
        .map(|task| (task.id.clone(), task))
        .collect();
    (order, tasks)
}

pub fn ordered(tasks: Vec<Task>) -> Vec<Row> {
    let mut tasks: Vec<Task> = tasks.into_iter().filter(|task| !task.deleted).collect();
    tasks.sort_by(|a, b| {
        a.is_done().cmp(&b.is_done()).then_with(|| {
            if a.is_done() {
                b.completed.cmp(&a.completed)
            } else {
                a.position.cmp(&b.position)
            }
        })
    });
    let ids: std::collections::HashSet<String> = tasks.iter().map(|task| task.id.clone()).collect();
    let (children, top): (Vec<Task>, Vec<Task>) = tasks.into_iter().partition(|task| {
        task.parent
            .as_ref()
            .is_some_and(|parent| ids.contains(parent))
    });

    let mut rows = Vec::with_capacity(top.len() + children.len());
    for task in top {
        let id = task.id.clone();
        rows.push(Row { task, depth: 0 });
        rows.extend(
            children
                .iter()
                .filter(|child| child.parent.as_deref() == Some(id.as_str()))
                .cloned()
                .map(|task| Row { task, depth: 1 }),
        );
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Status;

    fn task(id: &str, parent: Option<&str>, position: &str, done: bool) -> Task {
        Task {
            id: id.into(),
            parent: parent.map(Into::into),
            position: position.into(),
            status: if done {
                Status::Completed
            } else {
                Status::NeedsAction
            },
            ..Default::default()
        }
    }

    #[test]
    fn board_keys_tasks_and_derives_the_shared_row_order() {
        let board = Board::from_cache(cache::Cache {
            lists: vec![cache::CachedList {
                list: TaskList {
                    id: "list".into(),
                    title: "Inbox".into(),
                    ..Default::default()
                },
                tasks: vec![
                    task("done", None, "0", true),
                    task("b", None, "2", false),
                    task("a1", Some("a"), "1", false),
                    task("a", None, "1", false),
                    Task {
                        deleted: true,
                        ..task("deleted", None, "3", false)
                    },
                ],
            }],
        });

        let rows = board.rows("list");
        let got: Vec<(&str, u8)> = rows
            .iter()
            .map(|row| (row.task.id.as_str(), row.depth))
            .collect();
        assert_eq!(got, [("a", 0), ("a1", 1), ("b", 0), ("done", 0)]);

        let snapshot = board.snapshot();
        assert_eq!(snapshot.lists[0].tasks.len(), 4);
    }

    #[test]
    fn board_moves_and_restores_a_task_tree() {
        let mut board = Board::from_cache(cache::Cache {
            lists: vec![
                cache::CachedList {
                    list: TaskList {
                        id: "from".into(),
                        title: "From".into(),
                        ..Default::default()
                    },
                    tasks: vec![
                        task("parent", None, "1", false),
                        task("child", Some("parent"), "2", false),
                    ],
                },
                cache::CachedList {
                    list: TaskList {
                        id: "to".into(),
                        title: "To".into(),
                        ..Default::default()
                    },
                    tasks: vec![task("there", None, "1", false)],
                },
            ],
        });
        let from = board.tasks("from");
        let to = board.tasks("to");

        let moved = board.move_tree("from", "to", "parent");
        assert_eq!(moved.len(), 2);
        assert!(board.task("from", "parent").is_none());
        assert_eq!(
            board
                .task("to", "child")
                .and_then(|task| task.parent.as_deref()),
            Some("parent")
        );

        board.replace_tasks("from", from);
        board.replace_tasks("to", to);
        assert!(board.task("from", "parent").is_some());
        assert!(board.task("to", "parent").is_none());
    }

    #[test]
    fn equal_sort_keys_preserve_source_order() {
        let source = vec![
            task("first", None, "same", false),
            task("second", None, "same", false),
            task("third", None, "same", false),
        ];
        let board = Board::from_cache(cache::Cache {
            lists: vec![cache::CachedList {
                list: TaskList {
                    id: "list".into(),
                    title: "List".into(),
                    ..Default::default()
                },
                tasks: source,
            }],
        });

        assert_eq!(
            board
                .tasks("list")
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert_eq!(
            board
                .rows("list")
                .iter()
                .map(|row| row.task.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
    }
}
