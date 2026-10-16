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
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{paths::Paths, platform, storage};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// Google rejected the refresh token, so the user must sign in again.
    SignInExpired {
        invalidation_error: Option<String>,
    },
    /// The OAuth client file is missing or malformed.
    Credentials(String),
    /// The browser sign-in did not produce usable tokens.
    Authorization(String),
    /// The token endpoint could not be reached or gave an unusable answer.
    Http(String),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SignInExpired { invalidation_error } => {
                f.write_str("sign-in expired — restart the app to sign in again")?;
                match invalidation_error {
                    Some(error) => write!(f, "; {error}"),
                    None => Ok(()),
                }
            }
            Self::Credentials(message) | Self::Authorization(message) | Self::Http(message) => {
                f.write_str(message)
            }
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        Self::Http(error.to_string())
    }
}

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
    #[cfg(test)]
    pub(crate) fn for_test(
        agent: ureq::Agent,
        token_uri: String,
        token_path: PathBuf,
        access_token: &str,
    ) -> Self {
        Self {
            client: ClientSecret {
                client_id: "test-client".into(),
                client_secret: "test-secret".into(),
                auth_uri: String::new(),
                token_uri,
            },
            tok: Cached {
                client_id: "test-client".into(),
                refresh_token: "test-refresh".into(),
                access_token: access_token.into(),
                expires_at: u64::MAX,
            },
            agent,
            token_path,
        }
    }

    /// The cached token when there is one, otherwise the browser sign-in.
    pub fn load(paths: &Paths) -> Result<Self> {
        match Self::cached(paths) {
            Some(auth) => Ok(auth),
            None => Self::sign_in(paths),
        }
    }

    /// Load credentials and make sure we hold a working token. May open a
    /// browser, so call this before the TUI takes over the terminal.
    pub fn sign_in(paths: &Paths) -> Result<Self> {
        let token_path = paths.token();
        let credentials = paths.credentials.display();
        let raw = fs::read_to_string(&paths.credentials)
            .map_err(|e| Error::Credentials(format!("reading {credentials}: {e}")))?;
        let client = serde_json::from_str::<ClientFile>(&raw)
            .map_err(|e| Error::Credentials(format!("{credentials}: {e}")))?
            .installed;
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
        match auth.token() {
            Ok(_) => {}
            Err(Error::SignInExpired {
                invalidation_error: None,
            }) => auth.tok = authorize(&auth.agent, &auth.client)?,
            Err(error) => return Err(error),
        }
        auth.save()?;
        Ok(auth)
    }

    /// Build from the cached refresh token without touching the network, so
    /// startup doesn't wait on Google. `None` when there's nothing to reuse
    /// and the browser flow is needed.
    pub fn cached(paths: &Paths) -> Option<Self> {
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
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn save(&self) -> Result<()> {
        let json = serde_json::to_vec_pretty(&self.tok).map_err(std::io::Error::other)?;
        storage::atomic_private_write(&self.token_path, &json)?;
        Ok(())
    }
}

fn validate_token_status(status: u16, token_path: &std::path::Path) -> Result<()> {
    if matches!(status, 400 | 401) {
        let invalidation_error = invalidate_token(token_path);
        return Err(Error::SignInExpired { invalidation_error });
    }
    if !(200..300).contains(&status) {
        return Err(Error::Http(format!(
            "token endpoint returned HTTP {status}"
        )));
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Loopback OAuth flow: open the consent page, catch the redirect on a local
/// port, and trade the code for tokens.
fn authorize(agent: &ureq::Agent, c: &ClientSecret) -> Result<Cached> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let state = random_urlsafe(16)?;
    let verifier = random_urlsafe(32)?;
    let challenge = pkce_challenge(&verifier);
    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}&code_challenge={}&code_challenge_method=S256",
        c.auth_uri,
        pct(&c.client_id),
        pct(&redirect),
        pct(SCOPE),
        state,
        challenge,
    );
    eprintln!(
        "Authorize Google Tasks access in your browser. If it doesn't open, visit:\n\n  {url}\n"
    );
    if let Err(error) = platform::open_url(&url) {
        eprintln!("Could not open the browser: {error}");
    }

    // Browsers may open speculative connections; wait for the real redirect.
    let deadline = Instant::now() + Duration::from_secs(300);
    let params = loop {
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(Error::Authorization(
                        "authorization timed out after 5 minutes".into(),
                    ));
                }
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut line = String::new();
        BufReader::new(&stream).take(8192).read_line(&mut line)?;
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
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
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
        Error::Authorization(format!(
            "authorization failed: {}",
            params
                .get("error")
                .map_or("no code returned", String::as_str)
        ))
    })?;
    let r: TokenResponse = agent
        .post(&c.token_uri)
        .send_form([
            ("code", code.as_str()),
            ("client_id", c.client_id.as_str()),
            ("client_secret", c.client_secret.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier.as_str()),
        ])?
        .body_mut()
        .read_json()?;
    Ok(Cached {
        client_id: c.client_id.clone(),
        refresh_token: r
            .refresh_token
            .ok_or_else(|| Error::Authorization("Google returned no refresh token".into()))?,
        access_token: r.access_token,
        expires_at: now() + r.expires_in,
    })
}

fn random_urlsafe(bytes: usize) -> Result<String> {
    let mut data = vec![0; bytes];
    getrandom::getrandom(&mut data)
        .map_err(|error| Error::Authorization(format!("OS random source failed: {error}")))?;
    Ok(base64_url(&data))
}

fn pkce_challenge(verifier: &str) -> String {
    base64_url(&Sha256::digest(verifier.as_bytes()))
}

fn base64_url(data: &[u8]) -> String {
    const SET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let value = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        let count = chunk.len() + 1;
        for index in 0..count {
            out.push(SET[(value >> (18 - index * 6)) as usize & 63] as char);
        }
    }
    out
}

pub fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
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
    use std::os::unix::fs::PermissionsExt;

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
    fn pkce_matches_the_rfc_7636_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_rejected_refresh_token_is_typed_and_invalidated() {
        let root = test_root("rejected");
        fs::create_dir_all(&root).unwrap();
        let token_path = root.join("token.json");
        fs::write(&token_path, "{}").unwrap();
        let err = validate_token_status(400, &token_path).unwrap_err();
        assert!(matches!(
            err,
            Error::SignInExpired {
                invalidation_error: None
            }
        ));
        assert!(err.to_string().contains("restart the app to sign in again"));
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
        assert!(matches!(err, Error::Http(message) if message.contains("HTTP 500")));
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

        assert!(Auth::cached(&paths).is_none());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn malformed_cached_tokens_are_rejected() {
        let root = test_root("malformed");
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
        fs::write(paths.token(), "{not json").unwrap();

        assert!(Auth::cached(&paths).is_none());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn token_saves_are_atomic_private_and_report_failures() {
        let root = test_root("save");
        let token_path = root.join("config/token.json");
        let auth = Auth::for_test(
            http_agent(),
            "https://token.invalid".into(),
            token_path.clone(),
            "access",
        );

        auth.save().unwrap();
        auth.save().unwrap();
        assert_eq!(
            fs::metadata(&token_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let saved: Cached = serde_json::from_slice(&fs::read(&token_path).unwrap()).unwrap();
        assert_eq!(saved.client_id, "test-client");
        assert_eq!(saved.refresh_token, "test-refresh");
        assert_eq!(
            fs::read_dir(token_path.parent().unwrap())
                .unwrap()
                .filter_map(std::result::Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
                .count(),
            0
        );

        let blocked = root.join("blocked");
        fs::write(&blocked, "not a directory").unwrap();
        let auth = Auth::for_test(
            http_agent(),
            "https://token.invalid".into(),
            blocked.join("token.json"),
            "access",
        );
        assert!(auth.save().is_err());
        fs::remove_dir_all(root).ok();
    }
}
