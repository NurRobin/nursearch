//! Web search quicklinks. Activated with the `g` keyword: `g rust traits`
//! offers a search on a few engines; activating opens it in the browser.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{ResultItem, ViewEvent};

/// (engine name, query URL template with `{}` for the encoded query).
const ENGINES: &[(&str, &str)] = &[
    ("Google", "https://www.google.com/search?q={}"),
    ("DuckDuckGo", "https://duckduckgo.com/?q={}"),
    (
        "Wikipedia",
        "https://en.wikipedia.org/w/index.php?search={}",
    ),
];

struct Web;

impl Plugin for Web {
    fn query(&mut self, _host: &mut dyn HostApi, text: &str) -> Vec<ResultItem> {
        let query = text.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let encoded = url_encode(query);
        ENGINES
            .iter()
            .enumerate()
            .map(|(index, (name, template))| ResultItem {
                id: name.to_string(),
                title: format!("Search {name} for “{query}”"),
                subtitle: Some(template.replace("{}", &encoded)),
                icon: Some("web-browser".to_string()),
                // First engine ranks highest.
                score: 6_000 - index as i64,
                command_id: template.replace("{}", &encoded),
                actions: Vec::new(),
            })
            .collect()
    }

    fn activate(
        &mut self,
        host: &mut dyn HostApi,
        command_id: &str,
        _item_id: Option<String>,
    ) -> Option<Response> {
        host.open(command_id);
        Some(Response::Close { hide: true })
    }

    fn event(&mut self, _host: &mut dyn HostApi, _event: ViewEvent) -> Option<Response> {
        None
    }
}

/// Minimal percent-encoding for query strings (RFC 3986 unreserved kept).
fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn main() {
    run(Web);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_spaces_and_specials() {
        assert_eq!(url_encode("rust traits"), "rust%20traits");
        assert_eq!(url_encode("a&b"), "a%26b");
    }
}
