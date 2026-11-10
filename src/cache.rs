//! A snapshot of the board on disk, so startup paints before the network
//! answers. Written whenever a sync finishes or an edit lands.

use std::{fmt, fs, io};

use serde::{Deserialize, Serialize};

use crate::{
    atomic_file,
    model::{Task, TaskList},
    paths::Paths,
};

const VERSION: u32 = 1;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Cache {
    pub lists: Vec<CachedList>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CachedList {
    pub list: TaskList,
    pub tasks: Vec<Task>,
}

#[derive(Clone, Debug)]
pub struct Meta {
    pub account: String,
    pub generation: u64,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    account: String,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    lists: Vec<CachedList>,
}

#[derive(Debug)]
pub enum SaveError {
    Io(io::Error),
    Stale,
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cache write failed: {error}"),
            Self::Stale => f.write_str("cache changed in another process; refresh before saving"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<io::Error> for SaveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn load_for(paths: &Paths, account: &str) -> io::Result<(Cache, Meta)> {
    let path = paths.cache();
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((
                Cache::default(),
                Meta {
                    account: account.into(),
                    generation: 0,
                },
            ));
        }
        Err(error) => return Err(error),
    };
    let file: CacheFile = serde_json::from_str(&raw)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if file.version != VERSION || file.account != account {
        return Ok((
            Cache::default(),
            Meta {
                account: account.into(),
                generation: 0,
            },
        ));
    }
    Ok((
        Cache { lists: file.lists },
        Meta {
            account: file.account,
            generation: file.generation,
        },
    ))
}

pub fn save_for(paths: &Paths, cache: &Cache, meta: &mut Meta) -> Result<(), SaveError> {
    let path = paths.cache();
    atomic_file::with_exclusive_lock(&path, || {
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(current) = serde_json::from_str::<CacheFile>(&raw) {
                if current.version == VERSION
                    && current.account == meta.account
                    && current.generation != meta.generation
                {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "stale cache"));
                }
            }
        }
        let generation = meta.generation.saturating_add(1);
        let file = CacheFile {
            version: VERSION,
            account: meta.account.clone(),
            generation,
            lists: cache.lists.clone(),
        };
        let json = serde_json::to_vec(&file)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        atomic_file::write_private(&path, &json)?;
        meta.generation = generation;
        Ok(())
    })
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists && error.to_string() == "stale cache" {
            SaveError::Stale
        } else {
            SaveError::Io(error)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn paths(label: &str) -> Paths {
        let root = std::env::temp_dir().join(format!(
            "gtt-cache-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Paths {
            credentials: root.join("credentials.json"),
            require_credentials_file: false,
            config_dir: root,
        }
    }

    #[test]
    fn cache_is_versioned_scoped_and_rejects_stale_writers() {
        let paths = paths("generation");
        let cache = Cache::default();
        let mut first = Meta {
            account: "account-a".into(),
            generation: 0,
        };
        let mut stale = first.clone();

        save_for(&paths, &cache, &mut first).unwrap();
        assert_eq!(first.generation, 1);
        assert!(matches!(
            save_for(&paths, &cache, &mut stale),
            Err(SaveError::Stale)
        ));

        let (_, loaded) = load_for(&paths, "account-a").unwrap();
        assert_eq!(loaded.generation, 1);
        let (other, meta) = load_for(&paths, "account-b").unwrap();
        assert!(other.lists.is_empty());
        assert_eq!(meta.account, "account-b");
        assert_eq!(meta.generation, 0);
        fs::remove_dir_all(paths.config_dir).ok();
    }

    #[test]
    fn cache_writes_are_atomic_private_and_malformed_files_are_reported() {
        let paths = paths("atomic");
        let mut meta = Meta {
            account: "account".into(),
            generation: 0,
        };
        let cache = Cache {
            lists: vec![CachedList {
                list: TaskList {
                    id: "list".into(),
                    title: "Inbox".into(),
                    ..Default::default()
                },
                tasks: vec![Task {
                    id: "task".into(),
                    title: "First".into(),
                    ..Default::default()
                }],
            }],
        };

        save_for(&paths, &cache, &mut meta).unwrap();
        assert_eq!(
            fs::metadata(paths.cache()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::read_dir(&paths.config_dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
                .count(),
            0
        );

        fs::write(paths.cache(), "{not json").unwrap();
        let error = match load_for(&paths, "account") {
            Ok(_) => panic!("malformed cache was accepted"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        fs::remove_dir_all(paths.config_dir).ok();
    }

    #[test]
    fn cache_save_failures_are_returned() {
        let paths = paths("failure");
        fs::write(&paths.config_dir, "not a directory").unwrap();
        let mut meta = Meta {
            account: "account".into(),
            generation: 0,
        };

        assert!(matches!(
            save_for(&paths, &Cache::default(), &mut meta),
            Err(SaveError::Io(_))
        ));
        fs::remove_file(paths.config_dir).ok();
    }

    #[test]
    fn concurrent_writers_are_serialized_and_one_stale_writer_loses() {
        let paths = paths("concurrent");
        let barrier = Arc::new(Barrier::new(3));
        let writers: Vec<_> = (0..2)
            .map(|_| {
                let paths = paths.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    let mut meta = Meta {
                        account: "account".into(),
                        generation: 0,
                    };
                    barrier.wait();
                    save_for(&paths, &Cache::default(), &mut meta)
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(SaveError::Stale)))
                .count(),
            1
        );
        let (_, meta) = load_for(&paths, "account").unwrap();
        assert_eq!(meta.generation, 1);
        fs::remove_dir_all(paths.config_dir).ok();
    }
}
