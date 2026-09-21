//! A snapshot of the board on disk, so startup paints before the network
//! answers. Written whenever a sync finishes or an edit lands.

use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::{Task, TaskList};

#[derive(Default, Serialize, Deserialize)]
pub struct Cache {
    pub lists: Vec<CachedList>,
}

#[derive(Serialize, Deserialize)]
pub struct CachedList {
    pub list: TaskList,
    pub tasks: Vec<Task>,
}

fn path() -> PathBuf {
    crate::auth::config_dir().join("cache.json")
}

/// The last snapshot, or an empty one when there's nothing usable on disk.
pub fn load() -> Cache {
    fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Write via a temp file so an interrupted write can't leave a torn cache.
pub fn save(cache: &Cache) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let Ok(json) = serde_json::to_string(cache) else { return };
    let tmp = path.with_extension("tmp");
    let written = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(json.as_bytes()));
    match written {
        Ok(()) => {
            let _ = fs::rename(&tmp, &path);
        }
        Err(_) => {
            let _ = fs::remove_file(&tmp);
        }
    }
}
