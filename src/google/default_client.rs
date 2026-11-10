//! Shipped Desktop OAuth client for first-run sign-in without a local
//! `credentials.json`. Desktop clients are public; BYO overrides still win.

/// Google Cloud project that owns the shipped client.
#[allow(dead_code)] // documented for operators; asserted in tests
pub(super) const PROJECT_ID: &str = "gtasks-tui-pub";

pub(super) const CLIENT_ID: &str =
    "230591369474-unsclbulsvoqttohqr9c1kd3g7grj4nk.apps.googleusercontent.com";
pub(super) const CLIENT_SECRET: &str = "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";
pub(super) const AUTH_URI: &str = "https://accounts.google.com/o/oauth2/auth";
pub(super) const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

#[cfg(test)]
mod tests {
    #[test]
    fn shipped_client_is_the_publisher_desktop_app() {
        assert_eq!(super::PROJECT_ID, "gtasks-tui-pub");
        assert!(super::CLIENT_ID.ends_with(".apps.googleusercontent.com"));
        assert!(!super::CLIENT_SECRET.is_empty());
        assert_eq!(super::AUTH_URI, "https://accounts.google.com/o/oauth2/auth");
        assert_eq!(super::TOKEN_URI, "https://oauth2.googleapis.com/token");
    }
}
