//! What the user types in the address bar becomes a URL (or a web search).

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn looks_like_host(s: &str) -> bool {
    if s.contains(char::is_whitespace) || s.is_empty() {
        return false;
    }
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    let name = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let name = name
        .rsplit_once(':')
        .filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty())
        .map_or(name, |(h, _)| h);
    name == "localhost" || (name.contains('.') && !name.starts_with('.') && !name.ends_with('.'))
}

fn is_local(s: &str) -> bool {
    let host = s.split(['/', '?', '#', ':']).next().unwrap_or("");
    host == "localhost"
        || host == "127.0.0.1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_private() || ip.is_loopback())
}

/// A path on this machine as a `file://` URL: `/abs`, `./rel`, `../rel` or `~/x`.
fn local_path(s: &str) -> Option<String> {
    if s.contains(char::is_whitespace) && !std::path::Path::new(s).exists() {
        return None;
    }
    let path = if s.starts_with('/') {
        std::path::PathBuf::from(s)
    } else if let Some(rest) = s.strip_prefix("~/") {
        std::path::PathBuf::from(std::env::var_os("HOME")?).join(rest)
    } else if s.starts_with("./") || s.starts_with("../") {
        std::env::current_dir().ok()?.join(s)
    } else {
        return None;
    };
    Some(format!("file://{}", path.display()))
}

/// `example.com` → `https://example.com`, `localhost:3000` → `http://localhost:3000`,
/// anything else without a scheme → a DuckDuckGo search.
pub fn normalize(input: &str) -> String {
    let s = input.trim();
    if s.contains("://")
        || ["about:", "file:", "data:", "view-source:"]
            .iter()
            .any(|p| s.starts_with(p))
    {
        return s.to_string();
    }
    if let Some(f) = local_path(s) {
        return f;
    }
    if looks_like_host(s) {
        let scheme = if is_local(s) { "http" } else { "https" };
        return format!("{scheme}://{s}");
    }
    format!("https://duckduckgo.com/?q={}", percent_encode(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_a_scheme_to_hosts() {
        assert_eq!(normalize("example.com"), "https://example.com");
        assert_eq!(
            normalize("  es.wikipedia.org/wiki/Linux "),
            "https://es.wikipedia.org/wiki/Linux"
        );
        assert_eq!(normalize("localhost:3000/x"), "http://localhost:3000/x");
        assert_eq!(normalize("192.168.1.5:8080"), "http://192.168.1.5:8080");
        assert_eq!(normalize("8.8.8.8"), "https://8.8.8.8");
    }

    #[test]
    fn keeps_full_urls_and_special_schemes() {
        for u in [
            "https://a.org/x?y=1",
            "http://a.org",
            "file:///tmp/a.html",
            "about:blank",
            "view-source:https://a.org",
        ] {
            assert_eq!(normalize(u), u);
        }
    }

    #[test]
    fn paths_become_file_urls() {
        assert_eq!(normalize("/tmp/a.html"), "file:///tmp/a.html");
        assert_eq!(
            normalize("/tmp/con espacio.html".trim_end_matches("x")),
            "https://duckduckgo.com/?q=%2Ftmp%2Fcon+espacio.html"
        );
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            normalize("./x.html"),
            format!("file://{}/./x.html", cwd.display())
        );
        let home = std::env::var("HOME").unwrap();
        assert_eq!(normalize("~/p.html"), format!("file://{home}/p.html"));
    }

    #[test]
    fn everything_else_is_a_search() {
        assert_eq!(
            normalize("rust ownership"),
            "https://duckduckgo.com/?q=rust+ownership"
        );
        assert_eq!(normalize("linux"), "https://duckduckgo.com/?q=linux");
        assert_eq!(
            normalize("¿qué es esto?"),
            "https://duckduckgo.com/?q=%C2%BFqu%C3%A9+es+esto%3F"
        );
        assert_eq!(normalize("a.b c.d"), "https://duckduckgo.com/?q=a.b+c.d");
        assert_eq!(normalize(".hidden"), "https://duckduckgo.com/?q=.hidden");
    }
}
