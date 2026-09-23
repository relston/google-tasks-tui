use std::{env, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    pub credentials: PathBuf,
    pub config_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        let home = PathBuf::from(env::var_os("HOME").unwrap_or_default());
        let credentials = env::var_os("GOOGLE_TASKS_CREDENTIALS")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".gcloud/credentials.json"));
        let config_dir = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("google-tasks-tui");
        Self {
            credentials,
            config_dir,
        }
    }

    pub fn token(&self) -> PathBuf {
        self.config_dir.join("token.json")
    }

    pub fn cache(&self) -> PathBuf {
        self.config_dir.join("cache.json")
    }

    pub fn config(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
}
