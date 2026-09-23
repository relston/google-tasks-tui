//! A snapshot of the board on disk, so startup paints before the network
//! answers. Written whenever a sync finishes or an edit lands.

use std::{fmt, fs, io};

use serde::{Deserialize, Serialize};

use crate::{
    api::{Task, TaskList},
    paths::Paths,
    storage,
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

pub fn save_for(
    paths: &Paths,
    cache: &Cache,
    meta: &mut Meta,
) -> Result<(), SaveError> {
    let path = paths.cache();
    storage::with_exclusive_lock(&path, || {
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
        storage::atomic_private_write(&path, &json)?;
        meta.generation = generation;
        Ok(())
    })
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists
            && error.to_string() == "stale cache"
        {
            SaveError::Stale
        } else {
            SaveError::Io(error)
        }
    })
}

/// Compatibility helper for tests and callers without account context.
pub fn load() -> Cache {
    load_for(&Paths::from_env(), "")
        .map(|(cache, _)| cache)
        .unwrap_or_default()
}

pub fn save(cache: &Cache) {
    let mut meta = Meta {
        account: String::new(),
        generation: 0,
    };
    let _ = save_for(&Paths::from_env(), cache, &mut meta);
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
