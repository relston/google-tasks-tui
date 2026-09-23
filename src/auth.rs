//! Google OAuth for an installed ("desktop") client. Reads the client secret
//! from `~/.gcloud/credentials.json` (override with `GOOGLE_TASKS_CREDENTIALS`)
//! and caches the refresh token in `$XDG_CONFIG_HOME/google-tasks-tui/token.json`.
//! On first run it reuses a refresh token from a sibling `token.json` (the
//! google-auth quickstart format) if it covers the Tasks scope; otherwise it
//! runs the loopback browser flow.

use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{paths::Paths, storage};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const SCOPE: &str = "https://www.googleapis.com/auth/tasks";

fn http_agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_send_request(Some(Duration::from_secs(15)))
        .timeout_send_body(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .timeout_global(Some(Duration::from_secs(60)))
        .build();
    ureq::Agent::new_with_config(config)
}

#[derive(Debug)]
pub struct SignInExpired {
    invalidation_error: Option<String>,
}

impl std::fmt::Display for SignInExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("sign-in expired — restart the app to sign in again")?;
        if let Some(error) = &self.invalidation_error {
            write!(f, "; {error}")
        } else {
            Ok(())
        }
    }
}

impl std::error::Error for SignInExpired {}

#[derive(Deserialize)]
struct ClientFile {
    #[serde(alias = "web")]
    installed: ClientSecret,
}

#[derive(Deserialize)]
struct ClientSecret {
    client_id: String,
    client_secret: String,
    auth_uri: String,
    token_uri: String,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    #[serde(default)]
    client_id: String,
    refresh_token: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    expires_at: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

/// A token written by google-auth's `Credentials.to_json()`.
#[derive(Deserialize)]
struct ExistingToken {
    refresh_token: String,
    client_id: String,
    #[serde(default)]
    scopes: Vec<String>,
}

pub struct Auth {
    client: ClientSecret,
    tok: Cached,
    pub agent: ureq::Agent,
    token_path: PathBuf,
}

impl Auth {
    /// Load credentials and make sure we hold a working token. May open a
    /// browser, so call this before the TUI takes over the terminal.
    pub fn new() -> Result<Self> {
        Self::new_with_paths(&Paths::from_env())
    }

    pub fn new_with_paths(paths: &Paths) -> Result<Self> {
        let token_path = paths.token();
        let raw = fs::read_to_string(&paths.credentials)
            .map_err(|e| format!("reading {}: {e}", paths.credentials.display()))?;
        let client = serde_json::from_str::<ClientFile>(&raw)?.installed;
        let agent = http_agent();

        let cached = fs::read_to_string(&token_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .filter(|token: &Cached| {
                token.client_id.is_empty() || token.client_id == client.client_id
            })
            .or_else(|| {
                borrow_existing(&paths.credentials, &client.client_id).map(|refresh_token| Cached {
                    client_id: client.client_id.clone(),
                    refresh_token,
                    access_token: String::new(),
                    expires_at: 0,
                })
            });

        let mut auth = match cached {
            Some(mut tok) => {
                tok.client_id = client.client_id.clone();
                Self {
                client,
                tok,
                agent,
                token_path,
                }
            }
            None => {
                let tok = authorize(&agent, &client)?;
                Self {
                    client,
                    tok,
                    agent,
                    token_path,
                }
            }
        };
        // A revoked or expired refresh token means signing in again.
        if let Err(e) = auth.token() {
            let Some(expired) = e.downcast_ref::<SignInExpired>() else {
                return Err(e);
            };
            if expired.invalidation_error.is_some() {
                return Err(e);
            }
            auth.tok = authorize(&auth.agent, &auth.client)?;
        }
        auth.save()?;
        Ok(auth)
    }

    /// Build from the cached refresh token without touching the network, so
    /// startup doesn't wait on Google. `None` when there's nothing to reuse
    /// and the browser flow is needed.
    pub fn cached() -> Option<Self> {
        Self::cached_with_paths(&Paths::from_env())
    }

    pub fn cached_with_paths(paths: &Paths) -> Option<Self> {
        let raw = fs::read_to_string(&paths.credentials).ok()?;
        let client = serde_json::from_str::<ClientFile>(&raw).ok()?.installed;
        let token_path = paths.token();
        let tok = fs::read_to_string(&token_path)
            .ok()
            .and_then(|s| serde_json::from_str::<Cached>(&s).ok())
            .filter(|token| token.client_id == client.client_id)?;
        Some(Self {
            client,
            tok,
            agent: http_agent(),
            token_path,
        })
    }

    /// A valid access token, refreshed when it's within a minute of expiry.
    pub fn token(&mut self) -> Result<String> {
        if now() + 60 >= self.tok.expires_at {
            let mut response = self.agent.post(&self.client.token_uri).send_form([
                ("client_id", self.client.client_id.as_str()),
                ("client_secret", self.client.client_secret.as_str()),
                ("refresh_token", self.tok.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])?;
            let status = response.status().as_u16();
            validate_token_status(status, &self.token_path)?;
            let r: TokenResponse = response.body_mut().read_json()?;
            self.tok.access_token = r.access_token;
            self.tok.expires_at = now() + r.expires_in;
            if let Some(rt) = r.refresh_token {
                self.tok.refresh_token = rt;
            }
            self.save()?;
        }
        Ok(self.tok.access_token.clone())
    }

    pub(crate) fn force_refresh(&mut self) {
        self.tok.access_token.clear();
        self.tok.expires_at = 0;
    }

    pub fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.client.client_id.as_bytes());
        hash.update([0]);
        hash.update(self.tok.refresh_token.as_bytes());
        hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn save(&self) -> Result<()> {
        let json = serde_json::to_vec_pretty(&self.tok)?;
        storage::atomic_private_write(&self.token_path, &json)?;
        Ok(())
    }
}

fn validate_token_status(status: u16, token_path: &std::path::Path) -> Result<()> {
    if matches!(status, 400 | 401) {
        let invalidation_error = invalidate_token(token_path);
        return Err(SignInExpired { invalidation_error }.into());
    }
    if !(200..300).contains(&status) {
        return Err(format!("token endpoint returned HTTP {status}").into());
    }
    Ok(())
}

fn invalidate_token(path: &std::path::Path) -> Option<String> {
    match fs::remove_file(path) {
        Ok(()) => None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(format!(
            "could not remove {}: {e}; delete it manually",
            path.display()
        )),
    }
}

fn borrow_existing(credentials: &std::path::Path, client_id: &str) -> Option<String> {
    let raw = fs::read_to_string(credentials.with_file_name("token.json")).ok()?;
    let t: ExistingToken = serde_json::from_str(&raw).ok()?;
    (t.client_id == client_id && t.scopes.iter().any(|s| s == SCOPE)).then_some(t.refresh_token)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Loopback OAuth flow: open the consent page, catch the redirect on a local
/// port, and trade the code for tokens.
fn authorize(agent: &ureq::Agent, c: &ClientSecret) -> Result<Cached> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let state = random_hex()?;
    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}",
        c.auth_uri,
        pct(&c.client_id),
        pct(&redirect),
        pct(SCOPE),
        state,
    );
    eprintln!("Authorize Google Tasks access in your browser. If it doesn't open, visit:\n\n  {url}\n");
    let _ = Command::new("xdg-open")
        .arg(&url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    // Browsers may open speculative connections; wait for the real redirect.
    let params = loop {
        let (mut stream, _) = listener.accept()?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;
        let query = line
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.split_once('?'))
            .map_or("", |(_, q)| q);
        let params: HashMap<String, String> = query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_string(), decode(v)))
            .collect();
        if params.get("state") != Some(&state) {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        }
        let body = if params.contains_key("code") {
            "Authorized. You can close this tab and return to the terminal."
        } else {
            "Authorization failed. Check the terminal."
        };
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        break params;
    };

    let code = params.get("code").ok_or_else(|| {
        format!(
            "authorization failed: {}",
            params.get("error").map_or("no code returned", String::as_str)
        )
    })?;
    let r: TokenResponse = agent
        .post(&c.token_uri)
        .send_form([
            ("code", code.as_str()),
            ("client_id", c.client_id.as_str()),
            ("client_secret", c.client_secret.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
        ])?
        .body_mut()
        .read_json()?;
    Ok(Cached {
        client_id: c.client_id.clone(),
        refresh_token: r.refresh_token.ok_or("Google returned no refresh token")?,
        access_token: r.access_token,
        expires_at: now() + r.expires_in,
    })
}

fn random_hex() -> Result<String> {
    let mut buf = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (_, Some(v)) => {
                out.push(v);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn test_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("gtt-auth-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn percent_round_trip() {
        let s = "4/0Ab_c+d e/f?g=h&é";
        assert_eq!(decode(&pct(s)), s);
        assert_eq!(decode("a+b%2Fc%"), "a b/c%");
    }

    #[test]
    fn a_rejected_refresh_token_is_typed_and_invalidated() {
        let root = test_root("rejected");
        fs::create_dir_all(&root).unwrap();
        let token_path = root.join("token.json");
        fs::write(&token_path, "{}").unwrap();
        let err = validate_token_status(400, &token_path).unwrap_err();
        assert!(err.downcast_ref::<SignInExpired>().is_some());
        assert!(!token_path.exists());

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_transient_refresh_failure_preserves_the_cached_token() {
        let root = test_root("transient");
        fs::create_dir_all(&root).unwrap();
        let token_path = root.join("token.json");
        fs::write(&token_path, "{}").unwrap();
        let err = validate_token_status(500, &token_path).unwrap_err();
        assert!(err.downcast_ref::<SignInExpired>().is_none());
        assert!(token_path.exists());

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn token_invalidation_failure_is_reported() {
        let root = test_root("invalidation");
        let token_path = root.join("token.json");
        fs::create_dir_all(&token_path).unwrap();
        let error = invalidate_token(&token_path).unwrap();
        assert!(error.contains("could not remove"));
        assert!(token_path.exists());

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_http_agent_bounds_every_network_phase() {
        let agent = http_agent();
        let timeouts = agent.config().timeouts();

        assert_eq!(timeouts.connect, Some(Duration::from_secs(10)));
        assert_eq!(timeouts.send_request, Some(Duration::from_secs(15)));
        assert_eq!(timeouts.send_body, Some(Duration::from_secs(30)));
        assert_eq!(timeouts.recv_response, Some(Duration::from_secs(30)));
        assert_eq!(timeouts.recv_body, Some(Duration::from_secs(30)));
        assert_eq!(timeouts.global, Some(Duration::from_secs(60)));
    }

    #[test]
    fn cached_token_must_match_the_configured_client() {
        let root = test_root("client-id");
        fs::create_dir_all(&root).unwrap();
        let paths = Paths {
            credentials: root.join("credentials.json"),
            config_dir: root.join("config"),
        };
        fs::write(
            &paths.credentials,
            r#"{"installed":{"client_id":"right","client_secret":"secret","auth_uri":"https://auth","token_uri":"https://token"}}"#,
        )
        .unwrap();
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(
            paths.token(),
            r#"{"client_id":"wrong","refresh_token":"refresh"}"#,
        )
        .unwrap();

        assert!(Auth::cached_with_paths(&paths).is_none());
        fs::remove_dir_all(root).ok();
    }
}
