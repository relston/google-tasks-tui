use std::{env, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    /// OAuth client JSON path: `GOOGLE_TASKS_CREDENTIALS` or
    /// `~/.gcloud/credentials.json`. When the file is missing and
    /// [`Self::require_credentials_file`] is false, auth uses the shipped
    /// Desktop client instead.
    pub credentials: PathBuf,
    /// True when `GOOGLE_TASKS_CREDENTIALS` was set — a missing file is then
    /// an error rather than falling back to the built-in client.
    pub require_credentials_file: bool,
    pub config_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        let home = PathBuf::from(env::var_os("HOME").unwrap_or_default());
        let (credentials, require_credentials_file) = match env::var_os("GOOGLE_TASKS_CREDENTIALS")
        {
            Some(path) => (PathBuf::from(path), true),
            None => (home.join(".gcloud/credentials.json"), false),
        };
        let config_dir = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("google-tasks-tui");
        Self {
            credentials,
            require_credentials_file,
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
