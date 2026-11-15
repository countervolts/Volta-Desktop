//! Server address normalization. Port of `normalizeServer` / `serverCandidates`
//! in `src/lib/navidrome.ts`.
//!
//! The web relies on WHATWG `URL`. This port reproduces the parts of that
//! behavior the web code depends on: scheme detection, lowercase scheme and
//! host, rejection of embedded credentials, removal of a trailing `/rest…`
//! segment and trailing slashes, and dropping the query and fragment.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerError {
    Empty,
    Invalid,
    Scheme,
    Credentials,
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ServerError::Empty => "Enter your Navidrome server address.",
            ServerError::Invalid => "Enter a valid Navidrome server address.",
            ServerError::Scheme => "Your server address must use HTTP or HTTPS.",
            ServerError::Credentials => {
                "Enter your username and password in the sign-in fields, not the server address."
            }
        })
    }
}

impl std::error::Error for ServerError {}

/// `/^[a-z][a-z\d+.-]*:\/\//i`
fn has_scheme(value: &str) -> bool {
    let Some(index) = value.find("://") else {
        return false;
    };
    let scheme = &value[..index];
    let mut chars = scheme.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Normalizes one address that already carries a scheme.
pub fn normalize_server(raw: &str) -> Result<String, ServerError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(ServerError::Empty);
    }
    let with_scheme = if has_scheme(value) {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let split = with_scheme.find("://").ok_or(ServerError::Invalid)?;
    let scheme = with_scheme[..split].to_ascii_lowercase();
    let rest = &with_scheme[split + 3..];
    if scheme != "http" && scheme != "https" {
        return Err(ServerError::Scheme);
    }
    // Authority ends at the first `/`, `?` or `#`.
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let mut remainder = &rest[authority_end..];
    if authority.contains('@') {
        return Err(ServerError::Credentials);
    }
    if authority.is_empty() || authority.contains(char::is_whitespace) {
        return Err(ServerError::Invalid);
    }
    let host = normalize_authority(authority, &scheme)?;
    // Drop query and fragment.
    if let Some(cut) = remainder.find(['?', '#']) {
        remainder = &remainder[..cut];
    }
    let mut path = remainder.to_string();
    path = strip_rest_suffix(&path);
    while path.ends_with('/') {
        path.pop();
    }
    Ok(format!("{scheme}://{host}{path}"))
}

fn normalize_authority(authority: &str, scheme: &str) -> Result<String, ServerError> {
    let (host, port) = if authority.starts_with('[') {
        let close = authority.find(']').ok_or(ServerError::Invalid)?;
        let host = &authority[..=close];
        let after = &authority[close + 1..];
        let port = after.strip_prefix(':');
        if !after.is_empty() && port.is_none() {
            return Err(ServerError::Invalid);
        }
        (host.to_ascii_lowercase(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_ascii_lowercase(), Some(port)),
            None => (authority.to_ascii_lowercase(), None),
        }
    };
    if host.is_empty() {
        return Err(ServerError::Invalid);
    }
    let port = match port {
        None | Some("") => None,
        Some(port) => {
            let number: u16 = port.parse().map_err(|_| ServerError::Invalid)?;
            let default = if scheme == "https" { 443 } else { 80 };
            (number != default).then_some(number)
        }
    };
    Ok(match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// `pathname.replace(/\/rest(?:\/.*)?$/i, "")`
fn strip_rest_suffix(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    let mut search = 0;
    while let Some(found) = lower[search..].find("/rest") {
        let index = search + found;
        let after = &lower[index + 5..];
        if after.is_empty() || after.starts_with('/') {
            return path[..index].to_string();
        }
        search = index + 1;
    }
    path.to_string()
}

/// Addresses to try in order. A bare host prefers HTTPS and keeps an HTTP
/// fallback; an explicit scheme is used as given.
pub fn server_candidates(raw: &str) -> Result<Vec<String>, ServerError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(ServerError::Empty);
    }
    if has_scheme(value) {
        return Ok(vec![normalize_server(value)?]);
    }
    Ok(vec![
        normalize_server(&format!("https://{value}"))?,
        normalize_server(&format!("http://{value}"))?,
    ])
}

/// Strips the scheme for display, like `server.replace(/^https?:\/\//, "")`.
pub fn display_server(server: &str) -> &str {
    server
        .strip_prefix("https://")
        .or_else(|| server.strip_prefix("http://"))
        .unwrap_or(server)
}

#[cfg(test)]
mod tests {
    use super::*;

    // tests/desktop.spec.ts: "server addresses without a protocol prefer HTTPS
    // and retain HTTP fallback"
    #[test]
    fn bare_host_prefers_https_with_http_fallback() {
        assert_eq!(
            server_candidates("music.example.com/library").unwrap(),
            vec![
                "https://music.example.com/library",
                "http://music.example.com/library"
            ]
        );
        assert_eq!(
            server_candidates("http://music.example.com/library").unwrap(),
            vec!["http://music.example.com/library"]
        );
    }

    #[test]
    fn strips_rest_endpoint_and_trailing_slash() {
        assert_eq!(
            normalize_server("https://Music.Example.com/proxy/rest/ping.view?u=a").unwrap(),
            "https://music.example.com/proxy"
        );
        assert_eq!(
            normalize_server("https://music.example.com/").unwrap(),
            "https://music.example.com"
        );
        assert_eq!(
            normalize_server("https://music.example.com:443/REST").unwrap(),
            "https://music.example.com"
        );
        assert_eq!(
            normalize_server("http://localhost:4533").unwrap(),
            "http://localhost:4533"
        );
        assert_eq!(
            normalize_server("https://music.example.com/restful").unwrap(),
            "https://music.example.com/restful"
        );
    }

    #[test]
    fn rejects_bad_addresses() {
        assert_eq!(server_candidates("  "), Err(ServerError::Empty));
        assert_eq!(normalize_server("ftp://x"), Err(ServerError::Scheme));
        assert_eq!(
            normalize_server("https://me:pw@host"),
            Err(ServerError::Credentials)
        );
        assert_eq!(normalize_server("https://host:99999"), Err(ServerError::Invalid));
    }

    #[test]
    fn display_strips_scheme() {
        assert_eq!(display_server("https://demo.navidrome.org"), "demo.navidrome.org");
    }
}
