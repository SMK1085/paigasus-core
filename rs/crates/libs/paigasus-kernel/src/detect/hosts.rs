// SPDX-License-Identifier: Apache-2.0
//! Host extraction from tool call arguments (§ 6.3.3).

use std::collections::BTreeSet;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use url::{Host, Url};

use super::ToolInput;
use super::walk::walk_strings;

/// The most hosts a set keeps (D36).
pub const HOSTS_CAP: usize = 256;
/// The entry that stands for the hosts past the cap. No URL parser gives it as a host.
pub const HOSTS_OVERFLOW: &str = "<overflow>";

/// Keys whose string value counts as a bare host, with no scheme (D15). The match ignores case.
pub(crate) const BARE_HOST_KEYS: [&str; 10] = ["url", "uri", "href", "link", "host", "hostname", "domain", "endpoint", "base_url", "website"];

static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(https?|wss?|ftp):[/\\]*").expect("the scheme pattern is valid"));

/// A sorted set of normalised host names. It keeps the smallest 256 hosts.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct HostSet {
    hosts: BTreeSet<String>,
    overflow: bool,
}

impl fmt::Debug for HostSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HostSet {{ len: {} }}", self.len())
    }
}

impl HostSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Add a host. Past the cap, the largest host drops out and the set is marked as overflowed.
    pub fn insert(&mut self, host: &str) {
        self.hosts.insert(host.to_owned());
        if self.hosts.len() > HOSTS_CAP {
            self.hosts.pop_last();
            self.overflow = true;
        }
    }
    /// The number of entries that `iter` gives.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hosts.len()
    }
    /// Whether the set has no host.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }
    /// Whether `host` is in the set. The sentinel is in an overflowed set.
    #[must_use]
    pub fn contains(&self, host: &str) -> bool {
        self.hosts.contains(host) || (self.overflow && host == HOSTS_OVERFLOW)
    }
    /// Whether more than `HOSTS_CAP` hosts were added.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.overflow
    }
    /// The hosts in order. An overflowed set gives the first 255 and then `<overflow>`.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        let keep = if self.overflow { HOSTS_CAP - 1 } else { usize::MAX };
        self.hosts.iter().take(keep).map(String::as_str).chain(self.overflow.then_some(HOSTS_OVERFLOW))
    }
    /// The hosts of this set that are not in `seen`.
    pub fn difference<'a>(&'a self, seen: &'a HostSet) -> impl Iterator<Item = &'a str> {
        self.iter().filter(move |h| !seen.contains(h))
    }
    /// Add every host of `other`.
    pub fn extend(&mut self, other: &HostSet) {
        for h in &other.hosts {
            self.insert(h);
        }
        self.overflow |= other.overflow;
    }
}

fn is_terminator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`')
}

/// Trim the end of a candidate (step 2), until nothing changes.
fn trim_end(mut s: &str) -> &str {
    while let Some(last) = s.chars().next_back() {
        let rest = &s[..s.len() - last.len_utf8()];
        let open = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' => None,
            ')' => Some('('),
            ']' => Some('['),
            '}' => Some('{'),
            _ => return s,
        };
        if let Some(o) = open {
            let opens = rest.chars().filter(|c| *c == o).count();
            let closes = rest.chars().filter(|c| *c == last).count();
            if opens > closes {
                return s;
            }
        }
        s = rest;
    }
    s
}

/// The trimmed URL candidates of a text.
fn candidates(text: &str) -> Vec<&str> {
    SCHEME
        .find_iter(text)
        .map(|m| {
            let tail = &text[m.start()..];
            let end = tail.find(is_terminator).unwrap_or(tail.len());
            trim_end(&tail[..end])
        })
        .collect()
}

/// The normalised host of a parsed URL.
fn host_of(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Add the hosts of the URLs in `text`.
pub fn hosts_in_text(text: &str, into: &mut HostSet) {
    for c in candidates(text) {
        if let Some(host) = Url::parse(c).ok().as_ref().and_then(host_of) {
            into.insert(&host);
        }
    }
}

/// The host of a bare value under a key of `BARE_HOST_KEYS` (D15).
fn bare_host(key: &str, text: &str) -> Option<String> {
    if !BARE_HOST_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key)) {
        return None;
    }
    let value = text.trim();
    if value.is_empty() || value.chars().any(char::is_whitespace) || SCHEME.is_match(value) {
        return None;
    }
    let url = Url::parse(&format!("http://{}", trim_end(value))).ok()?;
    let host = host_of(&url)?;
    let is_ip = matches!(url.host(), Some(Host::Ipv4(_) | Host::Ipv6(_)));
    (host.contains('.') || is_ip).then_some(host)
}

/// The hosts of one leaf.
fn leaf_hosts(key: Option<&str>, text: &str) -> HostSet {
    let mut set = HostSet::new();
    hosts_in_text(text, &mut set);
    if let Some(host) = key.and_then(|k| bare_host(k, text)) {
        set.insert(&host);
    }
    set
}

/// Call `f` with the hosts of each leaf and the JSON pointer of that leaf.
fn for_each_leaf(input: &ToolInput, f: &mut dyn FnMut(&HostSet, &str)) {
    walk_strings(input, &mut |leaf| f(&leaf_hosts(leaf.key, leaf.text), leaf.pointer));
}

/// Every host in the arguments of one call.
#[must_use]
pub fn call_hosts(input: &ToolInput) -> HostSet {
    let mut set = HostSet::new();
    for_each_leaf(input, &mut |hosts, _| set.extend(hosts));
    set
}

/// Each host with the JSON pointer of the leaf that holds it. Free text has no pointer.
// Only its test calls it for now. The `unseen_host` detector takes it over, then the gate goes.
#[cfg(test)]
pub(crate) fn hosts_with_pointers(input: &ToolInput) -> Vec<(String, Option<String>)> {
    let is_text = matches!(input, ToolInput::Text(_));
    let mut out = Vec::new();
    for_each_leaf(input, &mut |hosts, pointer| {
        out.extend(hosts.iter().map(|h| (h.to_owned(), (!is_text).then(|| pointer.to_owned()))));
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::ToolInput;

    fn hosts(raw: &str) -> Vec<String> {
        call_hosts(&ToolInput::Json(raw.to_owned())).iter().map(str::to_owned).collect()
    }
    fn text_hosts(text: &str) -> Vec<String> {
        call_hosts(&ToolInput::Text(text.to_owned())).iter().map(str::to_owned).collect()
    }

    #[test]
    fn url_forms_normalise_to_one_host() {
        for raw in [
            r#"{"a":"https:evil.example/x"}"#,
            r#"{"a":"https:/evil.example"}"#,
            r#"{"a":"https:\\\\evil.example"}"#,
            r#"{"a":"HTTPS://EVIL.EXAMPLE"}"#,
        ] {
            assert_eq!(hosts(raw), ["evil.example"], "{raw}");
        }
    }
    #[test]
    fn nested_array_text_and_raw_inputs() {
        assert_eq!(hosts(r#"{"a":{"b":["x https://a.example/p y"]}}"#), ["a.example"]);
        assert_eq!(text_hosts("see http://b.example."), ["b.example"]);
        assert_eq!(hosts(r#"{"a":"https:\/\/c.example\/x", "#), ["c.example"]);
    }
    #[test]
    fn trailing_characters_are_trimmed() {
        for tail in [".", ",", ";", ":", "!", "?", ")", "]", "}", "'", "*"] {
            assert_eq!(text_hosts(&format!("https://t.example/p{tail}")), ["t.example"], "{tail}");
        }
        assert_eq!(candidates("https://w.example/a_(b)"), ["https://w.example/a_(b)"]);
        assert_eq!(candidates("(https://w.example/a)"), ["https://w.example/a"]);
    }
    #[test]
    fn scheme_allow_list() {
        for s in ["http", "https", "ws", "wss", "ftp"] {
            assert_eq!(text_hosts(&format!("{s}://s.example/")), ["s.example"]);
        }
        assert!(text_hosts("mailto:a@m.example file:///etc/passwd javascript:x").is_empty());
    }
    #[test]
    fn normalising() {
        assert_eq!(text_hosts("http://bücher.example/"), ["xn--bcher-kva.example"]);
        assert_eq!(text_hosts("http://dot.example./ http://u:p@port.example:8443/"), ["dot.example", "port.example"]);
        assert_eq!(text_hosts("http://10.0.0.1/ http://[::1]:80/"), ["10.0.0.1", "[::1]"]);
    }
    #[test]
    fn bare_hosts_only_under_listed_keys() {
        assert_eq!(
            hosts(r#"{"url":"bare.example","path":"main.rs","host":"localhost","endpoint":"10.1.1.1"}"#),
            ["10.1.1.1", "bare.example"]
        );
    }
    #[test]
    fn a_bare_host_under_an_upper_case_key_counts() {
        assert_eq!(hosts(r#"{"URL":"up.example","Base_Url":"b.example"}"#), ["b.example", "up.example"]);
    }
    #[test]
    fn double_encoded_arguments_are_walked() {
        assert_eq!(hosts(r#""{\"url\":\"https://evil.example/x\"}""#), ["evil.example"]);
    }
    #[test]
    fn duplicate_keys_behind_escapes_are_walked() {
        assert_eq!(hosts(r#"{"a":"x","a":"https://dup.example/"}"#), ["dup.example"]);
    }
    #[test]
    fn the_cap_keeps_255_and_the_sentinel() {
        let raw = format!("{{\"a\":\"{}\"}}", (0..257).map(|i| format!("https://h{i:03}.example/")).collect::<Vec<_>>().join(" "));
        let h = call_hosts(&ToolInput::Json(raw));
        assert!(h.overflowed());
        let v: Vec<_> = h.iter().collect();
        assert_eq!((v.len(), v[254], v[255]), (256, "h254.example", HOSTS_OVERFLOW));
    }
    #[test]
    fn the_sentinel_is_not_a_parsable_host() {
        assert!(url::Url::parse("http://<overflow>/").is_err());
    }
    #[test]
    fn debug_prints_the_count() {
        let mut set = HostSet::new();
        set.insert("canary.example");
        assert_eq!(format!("{set:?}"), "HostSet { len: 1 }");
    }
    #[test]
    fn set_operations() {
        let mut a = HostSet::new();
        a.insert("b.example");
        a.insert("a.example");
        a.insert("a.example");
        let mut seen = HostSet::new();
        seen.insert("a.example");
        assert_eq!((a.len(), a.is_empty(), a.contains("a.example")), (2, false, true));
        assert_eq!(a.difference(&seen).collect::<Vec<_>>(), ["b.example"]);
        seen.extend(&a);
        assert_eq!(seen.difference(&a).count(), 0);
        assert_eq!(a, seen);
    }
    #[test]
    fn pointers_follow_the_hosts() {
        let got = hosts_with_pointers(&ToolInput::Json(r#"{"a/b":["https://p.example/"]}"#.to_owned()));
        assert_eq!(got, [("p.example".to_owned(), Some("/a~1b/0".to_owned()))]);
        let got = hosts_with_pointers(&ToolInput::Text("http://t.example".to_owned()));
        assert_eq!(got, [("t.example".to_owned(), None)]);
    }
}
