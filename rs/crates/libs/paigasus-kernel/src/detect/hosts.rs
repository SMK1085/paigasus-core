// SPDX-License-Identifier: Apache-2.0
//! Host extraction from tool call arguments (§ 6.3.3).

use std::collections::BTreeSet;
use std::fmt;
use std::net::Ipv6Addr;
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
        if self.overflow { self.hosts.len().min(HOSTS_CAP - 1) + 1 } else { self.hosts.len() }
    }
    /// Whether the set has no host.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

/// The longest authority (user information, host and port) that the extraction parses. A longer
/// one marks the set as overflowed, so the finding path reports it. A real host is at most 253
/// bytes, so the bound leaves room for long user information.
const AUTHORITY_MAX: usize = 1024;

/// A character that ends a candidate.
fn is_terminator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`')
}

/// A character that ends the authority of a URL: a terminator, or the start of the path, query or
/// fragment (a backslash is a slash for a special scheme).
fn ends_authority(c: char) -> bool {
    is_terminator(c) || matches!(c, '/' | '\\' | '?' | '#')
}

/// Trim the end of a candidate (step 2), until nothing changes. The count of each bracket is made
/// once and then updated per removed character, so the cost is linear.
fn trim_end(mut s: &str) -> &str {
    // The counts of `(`, `[`, `{`, `)`, `]` and `}`.
    let mut count = [0usize; 6];
    for c in s.chars() {
        if let Some(k) = "([{)]}".find(c) {
            count[k] += 1;
        }
    }
    while let Some(last) = s.chars().next_back() {
        match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' => {}
            ')' | ']' | '}' => {
                let k = "([{)]}".find(last).unwrap_or(3);
                // `count[k]` includes this character. Keep it when the open partner outnumbers the
                // other closers before it.
                if count[k - 3] > count[k] - 1 {
                    return s;
                }
                count[k] -= 1;
            }
            _ => return s,
        }
        s = &s[..s.len() - last.len_utf8()];
    }
    s
}

/// The URL candidates of a text: for each scheme match, the scheme and the authority, trimmed.
/// The second value is true when an authority is longer than `AUTHORITY_MAX`. Such a candidate
/// is not parsed. The cost is linear: one pass finds the end characters, and each match then
/// takes one binary search.
fn candidates(text: &str) -> (Vec<&str>, bool) {
    let ends: Vec<usize> = text.char_indices().filter(|(_, c)| ends_authority(*c)).map(|(i, _)| i).collect();
    let mut overflow = false;
    let mut out = Vec::new();
    for m in SCHEME.find_iter(text) {
        let end = ends.get(ends.partition_point(|p| *p < m.end())).copied().unwrap_or(text.len());
        if end - m.end() > AUTHORITY_MAX {
            overflow = true;
        } else {
            out.push(trim_end(&text[m.start()..end]));
        }
    }
    (out, overflow)
}

/// The normalised host of a parsed URL.
fn host_of(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Add the hosts of the URLs in `text`.
pub fn hosts_in_text(text: &str, into: &mut HostSet) {
    let (found, overflow) = candidates(text);
    for c in found {
        if let Some(host) = Url::parse(c).ok().as_ref().and_then(host_of) {
            into.insert(&host);
        }
    }
    into.overflow |= overflow;
}

/// Add the host of a bare value under a key of `BARE_HOST_KEYS` (D15). A value that starts with a
/// scheme is a URL, which `hosts_in_text` reads.
fn bare_host(key: &str, text: &str, into: &mut HostSet) {
    if !BARE_HOST_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key)) {
        return;
    }
    let value = text.trim();
    if value.is_empty() || value.chars().any(char::is_whitespace) || SCHEME.find(value).is_some_and(|m| m.start() == 0) {
        return;
    }
    // A scheme-relative value (`//host/x`, `\\host`) has the authority after the slashes, as the
    // scheme pattern allows `[/\\]*`.
    let value = value.trim_start_matches(['/', '\\']);
    if value.is_empty() {
        return;
    }
    let end = value.find(ends_authority).unwrap_or(value.len());
    if end > AUTHORITY_MAX {
        into.overflow = true;
        return;
    }
    let authority = &value[..end];
    let authority = if authority.parse::<Ipv6Addr>().is_ok() {
        format!("[{authority}]")
    } else {
        trim_end(authority).to_owned()
    };
    let Ok(url) = Url::parse(&format!("http://{authority}")) else {
        return;
    };
    let Some(host) = host_of(&url) else {
        return;
    };
    let is_ip = matches!(url.host(), Some(Host::Ipv4(_) | Host::Ipv6(_)));
    if host.contains('.') || is_ip {
        into.insert(&host);
    }
}

/// The hosts of one leaf.
fn leaf_hosts(key: Option<&str>, text: &str) -> HostSet {
    let mut set = HostSet::new();
    hosts_in_text(text, &mut set);
    if let Some(key) = key {
        bare_host(key, text, &mut set);
    }
    set
}

/// Call `f` with the hosts of each leaf and the JSON pointer of that leaf. Free text has no
/// pointer. The pointer is borrowed, so a caller copies it only when it needs to.
pub(crate) fn scan_hosts(input: &ToolInput, f: &mut dyn FnMut(&HostSet, Option<&str>)) {
    let is_text = matches!(input, ToolInput::Text(_));
    walk_strings(input, &mut |leaf| f(&leaf_hosts(leaf.key, leaf.text), (!is_text).then_some(leaf.pointer)));
}

/// Every host in the arguments of one call.
#[must_use]
pub fn call_hosts(input: &ToolInput) -> HostSet {
    let mut set = HostSet::new();
    scan_hosts(input, &mut |hosts, _| set.extend(hosts));
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::ToolInput;

    fn hosts_with_pointers(input: &ToolInput) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        scan_hosts(input, &mut |hosts, pointer| {
            out.extend(hosts.iter().map(|h| (h.to_owned(), pointer.map(str::to_owned))));
        });
        out
    }

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
        // A candidate is the scheme and the authority. The path does not matter for the host.
        assert_eq!(candidates("https://w.example/a_(b)").0, ["https://w.example"]);
        assert_eq!(candidates("(https://w.example)").0, ["https://w.example"]);
        assert_eq!(candidates("https://[::1])").0, ["https://[::1]"]);
        assert_eq!(candidates("https://w.example.),;").0, ["https://w.example"]);
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
        assert_eq!(hosts(r#"{"a":"x","a":"https:\/\/dup.example\/"}"#), ["dup.example"]);
        assert_eq!(hosts(r#"{"a":"x","a":"https://\u0064up.example/"}"#), ["dup.example"]);
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

    #[test]
    fn a_bare_host_with_a_scheme_later_in_the_value_counts() {
        assert_eq!(hosts(r#"{"url":"evil.example/?next=https://ok.example"}"#), ["evil.example", "ok.example"]);
        assert_eq!(hosts(r#"{"url":"https://ok.example/x"}"#), ["ok.example"]);
    }

    #[test]
    fn a_scheme_relative_bare_host_counts() {
        assert_eq!(hosts(r#"{"url":"//evil.example/x"}"#), ["evil.example"]);
        assert_eq!(hosts(r#"{"url":"\\\\evil.example/x"}"#), ["evil.example"]);
        assert!(hosts(r#"{"url":"///"}"#).is_empty());
    }

    #[test]
    fn a_bare_ipv6_address_counts() {
        assert_eq!(hosts(r#"{"host":"::1"}"#), ["[::1]"]);
        assert_eq!(hosts(r#"{"host":"[::1]:8080"}"#), ["[::1]"]);
    }

    #[test]
    fn depth_128_is_walked_for_hosts() {
        let nest = |n: usize| format!("{}\"https://\\u0065vil.example/\"{}", "[".repeat(n), "]".repeat(n));
        assert_eq!(hosts(&nest(128)), ["evil.example"]);
        assert_eq!(hosts(&nest(129)), ["evil.example"]);
    }

    #[test]
    fn raw_text_decodes_unicode_escapes_in_hosts() {
        assert_eq!(hosts(r#"{"a":"https://\u0065vil.example/ "#), ["evil.example"]);
        assert_eq!(hosts(r#"{"a":"https:\\\\b.example "#), ["b.example"]);
        assert!(hosts(r#"{"a":"https://\ud800.example/ "#).iter().all(|h| !h.contains("ud800")));
    }

    #[test]
    fn a_long_authority_marks_the_set_as_overflowed() {
        let long = format!("https://u:{}@x.example/", "p".repeat(AUTHORITY_MAX + 1));
        let h = call_hosts(&ToolInput::Text(long));
        assert!(h.overflowed());
        assert!(h.contains(HOSTS_OVERFLOW));
        let ok = format!("https://u:{}@x.example/", "p".repeat(AUTHORITY_MAX - 20));
        let h = call_hosts(&ToolInput::Text(ok));
        assert!(!h.overflowed());
        assert_eq!(h.iter().collect::<Vec<_>>(), ["x.example"]);
        let bare = call_hosts(&ToolInput::Json(format!(r#"{{"url":"{}.example"}}"#, "a".repeat(AUTHORITY_MAX + 1))));
        assert!(bare.overflowed());
        // One overflowing authority and no host: the set is not empty, and `len` counts the sentinel.
        let only = call_hosts(&ToolInput::Text(format!("https://{}", "a".repeat(AUTHORITY_MAX + 1))));
        assert!(only.overflowed() && !only.is_empty());
        assert_eq!((only.len(), only.iter().count()), (1, 1));
        let mut one = HostSet::new();
        one.insert("a.example");
        one.extend(&only);
        assert_eq!((one.len(), one.iter().count()), (2, 2));
    }

    /// The time for `f`, the best of five runs.
    fn best_of_five(f: impl Fn()) -> std::time::Duration {
        (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                f();
                t.elapsed()
            })
            .min()
            .unwrap_or_default()
    }

    #[test]
    fn extraction_time_is_linear_in_the_input_size() {
        type Make = Box<dyn Fn(usize) -> String>;
        let cases: [(&str, Make); 4] = [
            ("scheme repeat", Box::new(|n| "http:".repeat(n))),
            ("closer repeat", Box::new(|n| format!("https://x.example{}", ")".repeat(n)))),
            ("long authority", Box::new(|n| format!("https://{}@x.example/", "a:".repeat(n)))),
            ("slash repeat", Box::new(|n| format!("https://x.example/{}", "/http:".repeat(n)))),
        ];
        for (name, make) in &cases {
            let (small, large) = (make(5_000), make(80_000));
            let t1 = best_of_five(|| drop(call_hosts(&ToolInput::Text(small.clone()))));
            let t16 = best_of_five(|| drop(call_hosts(&ToolInput::Text(large.clone()))));
            // The size grows 16 times. Linear time grows about 16 times, quadratic time about 256
            // times. The limit of 40 leaves a margin of 2.5 for a loaded host. A floor of 5 ms on
            // the small run keeps a very fast run from giving a noisy limit.
            let floor = std::time::Duration::from_millis(5);
            assert!(t16 < t1.max(floor) * 40, "{name}: {t1:?} for N, {t16:?} for 16N");
        }
    }
}
