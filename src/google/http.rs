//! The shared HTTP agent and URL encoding.

use std::time::Duration;

/// Every network phase is bounded. Statuses come back as responses so
/// callers can map them.
pub(super) fn agent() -> ureq::Agent {
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

/// Percent-encode everything except RFC 3986 unreserved characters.
pub(super) fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Decode a query-string value, treating `+` as a space.
pub(super) fn decode(s: &str) -> String {
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

    #[test]
    fn the_http_agent_bounds_every_network_phase() {
        let agent = agent();
        let timeouts = agent.config().timeouts();

        assert_eq!(timeouts.connect, Some(Duration::from_secs(10)));
        assert_eq!(timeouts.send_request, Some(Duration::from_secs(15)));
        assert_eq!(timeouts.send_body, Some(Duration::from_secs(30)));
        assert_eq!(timeouts.recv_response, Some(Duration::from_secs(30)));
        assert_eq!(timeouts.recv_body, Some(Duration::from_secs(30)));
        assert_eq!(timeouts.global, Some(Duration::from_secs(60)));
    }
}
