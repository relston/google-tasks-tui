//! Google OAuth for an installed ("desktop") client. Reads the client secret
//! from `~/.gcloud/credentials.json` (override with `GOOGLE_TASKS_CREDENTIALS`)
//! and caches the refresh token in `$XDG_CONFIG_HOME/google-tasks-tui/token.json`.
//! On first run it reuses a refresh token from a sibling `token.json` (the
//! google-auth quickstart format) if it covers the Tasks scope; otherwise it
//! runs the loopback browser flow.

use std::{
    collections::HashMap,
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const SCOPE: &str = "https://www.googleapis.com/auth/tasks";

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
}

impl Auth {
    /// Load credentials and make sure we hold a working token. May open a
    /// browser, so call this before the TUI takes over the terminal.
    pub fn new() -> Result<Self> {
        let path = credentials_path();
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let client = serde_json::from_str::<ClientFile>(&raw)?.installed;
        let agent = ureq::Agent::new_with_defaults();

        let cached = fs::read_to_string(cache_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .or_else(|| {
                borrow_existing(&client.client_id).map(|refresh_token| Cached {
                    refresh_token,
                    access_token: String::new(),
                    expires_at: 0,
                })
            });

        let mut auth = match cached {
            Some(tok) => Self { client, tok, agent },
            None => {
                let tok = authorize(&agent, &client)?;
                Self { client, tok, agent }
            }
        };
        // A revoked or expired refresh token means signing in again.
        if auth.token().is_err() {
            auth.tok = authorize(&auth.agent, &auth.client)?;
        }
        auth.save();
        Ok(auth)
    }

    /// Build from the cached refresh token without touching the network, so
    /// startup doesn't wait on Google. `None` when there's nothing to reuse
    /// and the browser flow is needed.
    pub fn cached() -> Option<Self> {
        let raw = fs::read_to_string(credentials_path()).ok()?;
        let client = serde_json::from_str::<ClientFile>(&raw).ok()?.installed;
        let tok = fs::read_to_string(cache_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .or_else(|| {
                borrow_existing(&client.client_id).map(|refresh_token| Cached {
                    refresh_token,
                    access_token: String::new(),
                    expires_at: 0,
                })
            })?;
        Some(Self { client, tok, agent: ureq::Agent::new_with_defaults() })
    }

    /// A valid access token, refreshed when it's within a minute of expiry.
    pub fn token(&mut self) -> Result<String> {
        if now() + 60 >= self.tok.expires_at {
            let sent = self.agent.post(&self.client.token_uri).send_form([
                ("client_id", self.client.client_id.as_str()),
                ("client_secret", self.client.client_secret.as_str()),
                ("refresh_token", self.tok.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ]);
            let r: TokenResponse = match sent {
                Ok(mut resp) => resp.body_mut().read_json()?,
                // Google rejects the token itself, rather than failing to answer.
                Err(ureq::Error::StatusCode(400 | 401)) => {
                    return Err("sign-in expired — quit and start the app again to sign in".into())
                }
                Err(e) => return Err(e.into()),
            };
            self.tok.access_token = r.access_token;
            self.tok.expires_at = now() + r.expires_in;
            if let Some(rt) = r.refresh_token {
                self.tok.refresh_token = rt;
            }
            self.save();
        }
        Ok(self.tok.access_token.clone())
    }

    fn save(&self) {
        let path = cache_path();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let Ok(json) = serde_json::to_string_pretty(&self.tok) else { return };
        let _ = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| f.write_all(json.as_bytes()));
    }
}

fn home() -> PathBuf {
    PathBuf::from(env::var_os("HOME").unwrap_or_default())
}

fn credentials_path() -> PathBuf {
    env::var_os("GOOGLE_TASKS_CREDENTIALS")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".gcloud/credentials.json"))
}

/// Where this app keeps its token and its board snapshot.
pub fn config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("google-tasks-tui")
}

fn cache_path() -> PathBuf {
    config_dir().join("token.json")
}

fn borrow_existing(client_id: &str) -> Option<String> {
    let raw = fs::read_to_string(credentials_path().with_file_name("token.json")).ok()?;
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

    #[test]
    fn percent_round_trip() {
        let s = "4/0Ab_c+d e/f?g=h&é";
        assert_eq!(decode(&pct(s)), s);
        assert_eq!(decode("a+b%2Fc%"), "a b/c%");
    }
}
