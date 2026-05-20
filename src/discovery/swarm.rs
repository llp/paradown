use crate::error::Error;
use crate::p2p::MagnetLink;
use regex::Regex;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TorrentDiscoveryInputKind {
    Auto,
    Html,
    Feed,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentDiscoveryOptions {
    pub input_kind: TorrentDiscoveryInputKind,
    pub base_url: Option<String>,
    pub base_path: Option<PathBuf>,
}

impl Default for TorrentDiscoveryOptions {
    fn default() -> Self {
        Self {
            input_kind: TorrentDiscoveryInputKind::Auto,
            base_url: None,
            base_path: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TorrentDiscoveryKind {
    Magnet,
    TorrentFile,
    Tracker,
    WebSeed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentDiscoveryCandidate {
    pub kind: TorrentDiscoveryKind,
    pub locator: String,
    pub display_name: Option<String>,
    pub source: String,
}

pub fn discover_torrent_candidates(
    input: &str,
    options: &TorrentDiscoveryOptions,
) -> Result<Vec<TorrentDiscoveryCandidate>, Error> {
    let mut candidates = Vec::new();
    match options.input_kind {
        TorrentDiscoveryInputKind::Auto => {
            if let Ok(feed_candidates) = discover_feed(input, options) {
                candidates.extend(feed_candidates);
            }
            candidates.extend(discover_html(input, options)?);
            candidates.extend(discover_text(input, options)?);
        }
        TorrentDiscoveryInputKind::Html => candidates.extend(discover_html(input, options)?),
        TorrentDiscoveryInputKind::Feed => candidates.extend(discover_feed(input, options)?),
        TorrentDiscoveryInputKind::Text => candidates.extend(discover_text(input, options)?),
    }
    Ok(deduplicate(candidates))
}

fn discover_html(
    input: &str,
    options: &TorrentDiscoveryOptions,
) -> Result<Vec<TorrentDiscoveryCandidate>, Error> {
    let document = Html::parse_document(input);
    let selector = Selector::parse("a[href], area[href], link[href]")
        .map_err(|err| Error::Other(format!("failed to compile HTML selector: {err}")))?;
    let mut candidates = Vec::new();
    for element in document.select(&selector) {
        let Some(raw) = element.value().attr("href") else {
            continue;
        };
        let label = element
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        push_candidate_from_locator(
            &mut candidates,
            raw,
            non_empty(label),
            "html:href",
            options.base_url.as_deref(),
            options.base_path.as_deref(),
        );
    }
    candidates.extend(discover_text(input, options)?);
    Ok(candidates)
}

fn discover_feed(
    input: &str,
    options: &TorrentDiscoveryOptions,
) -> Result<Vec<TorrentDiscoveryCandidate>, Error> {
    let feed = feed_rs::parser::parse(input.as_bytes())
        .map_err(|err| Error::Other(format!("failed to parse feed: {err}")))?;
    let mut candidates = Vec::new();
    for entry in feed.entries {
        let title = entry.title.map(|title| title.content);
        for link in entry.links {
            push_candidate_from_locator(
                &mut candidates,
                &link.href,
                title.clone(),
                "feed:link",
                options.base_url.as_deref(),
                options.base_path.as_deref(),
            );
        }
        if let Some(summary) = entry.summary {
            candidates.extend(discover_text(
                &summary.content,
                &TorrentDiscoveryOptions {
                    input_kind: TorrentDiscoveryInputKind::Text,
                    base_url: options.base_url.clone(),
                    base_path: options.base_path.clone(),
                },
            )?);
        }
        if let Some(content) = entry.content
            && let Some(body) = content.body
        {
            candidates.extend(discover_torrent_candidates(
                &body,
                &TorrentDiscoveryOptions {
                    input_kind: TorrentDiscoveryInputKind::Auto,
                    base_url: options.base_url.clone(),
                    base_path: options.base_path.clone(),
                },
            )?);
        }
    }
    Ok(candidates)
}

fn discover_text(
    input: &str,
    options: &TorrentDiscoveryOptions,
) -> Result<Vec<TorrentDiscoveryCandidate>, Error> {
    let mut candidates = Vec::new();
    for found in magnet_regex().find_iter(input) {
        push_candidate_from_locator(
            &mut candidates,
            strip_trailing_punctuation(found.as_str()),
            None,
            "text:magnet",
            options.base_url.as_deref(),
            options.base_path.as_deref(),
        );
    }
    for found in url_regex().find_iter(input) {
        push_candidate_from_locator(
            &mut candidates,
            strip_trailing_punctuation(found.as_str()),
            None,
            "text:url",
            options.base_url.as_deref(),
            options.base_path.as_deref(),
        );
    }
    Ok(candidates)
}

fn push_candidate_from_locator(
    candidates: &mut Vec<TorrentDiscoveryCandidate>,
    raw: &str,
    display_name: Option<String>,
    source: &str,
    base_url: Option<&str>,
    base_path: Option<&std::path::Path>,
) {
    let Some(locator) = normalize_locator(raw, base_url, base_path) else {
        return;
    };
    if locator.starts_with("magnet:?") {
        candidates.push(TorrentDiscoveryCandidate {
            kind: TorrentDiscoveryKind::Magnet,
            locator: locator.clone(),
            display_name: display_name.clone(),
            source: source.into(),
        });
        if let Ok(magnet) = MagnetLink::parse(&locator) {
            for tracker in magnet.trackers {
                candidates.push(TorrentDiscoveryCandidate {
                    kind: TorrentDiscoveryKind::Tracker,
                    locator: tracker,
                    display_name: display_name.clone(),
                    source: "magnet:tr".into(),
                });
            }
            for web_seed in magnet.web_seeds {
                candidates.push(TorrentDiscoveryCandidate {
                    kind: TorrentDiscoveryKind::WebSeed,
                    locator: web_seed,
                    display_name: display_name.clone(),
                    source: "magnet:ws".into(),
                });
            }
        }
    } else if is_torrent_locator(&locator) {
        candidates.push(TorrentDiscoveryCandidate {
            kind: TorrentDiscoveryKind::TorrentFile,
            locator,
            display_name,
            source: source.into(),
        });
    } else if is_tracker_locator(&locator) {
        candidates.push(TorrentDiscoveryCandidate {
            kind: TorrentDiscoveryKind::Tracker,
            locator,
            display_name,
            source: source.into(),
        });
    }
}

fn normalize_locator(
    raw: &str,
    base_url: Option<&str>,
    base_path: Option<&std::path::Path>,
) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('#') {
        return None;
    }
    if raw.starts_with("magnet:?") {
        return Some(raw.to_string());
    }
    if let Ok(url) = url::Url::parse(raw) {
        if url.scheme() == "file" {
            return url
                .to_file_path()
                .ok()
                .map(|path| path.to_string_lossy().into_owned());
        }
        return Some(url.to_string());
    }
    if let Some(base) = base_url.and_then(|base| url::Url::parse(base).ok())
        && let Ok(url) = base.join(raw)
    {
        return Some(url.to_string());
    }
    if let Some(base_path) = base_path
        && raw.to_ascii_lowercase().ends_with(".torrent")
    {
        return Some(base_path.join(raw).to_string_lossy().into_owned());
    }
    if raw.to_ascii_lowercase().ends_with(".torrent") {
        return Some(raw.to_string());
    }
    None
}

fn is_torrent_locator(locator: &str) -> bool {
    if locator.to_ascii_lowercase().ends_with(".torrent") {
        return true;
    }
    url::Url::parse(locator)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .map(|last| last.to_ascii_lowercase())
        })
        .is_some_and(|last| last.ends_with(".torrent"))
}

fn is_tracker_locator(locator: &str) -> bool {
    let Ok(url) = url::Url::parse(locator) else {
        return false;
    };
    matches!(url.scheme(), "udp" | "http" | "https")
        && url.path().to_ascii_lowercase().contains("announce")
}

fn deduplicate(candidates: Vec<TorrentDiscoveryCandidate>) -> Vec<TorrentDiscoveryCandidate> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for candidate in candidates {
        if seen.insert((candidate.kind, candidate.locator.clone())) {
            deduped.push(candidate);
        }
    }
    deduped
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn strip_trailing_punctuation(value: &str) -> &str {
    value.trim_end_matches(|ch| matches!(ch, '.' | ',' | ';' | ')' | ']'))
}

fn magnet_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"magnet:\?[^\s"'<>]+"#).expect("valid magnet regex"))
}

fn url_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:https?|udp)://[^\s"'<>]+"#).expect("valid URL regex")
    })
}

#[cfg(test)]
mod tests {
    use super::{
        TorrentDiscoveryInputKind, TorrentDiscoveryKind, TorrentDiscoveryOptions,
        discover_torrent_candidates,
    };

    #[test]
    fn discovers_magnets_torrents_and_trackers_from_html() {
        let input = r#"
            <html>
              <a href="/payload.torrent">Payload</a>
              <a href="magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01&tr=udp%3A%2F%2Ftracker.example%2Fannounce">Magnet</a>
            </html>
        "#;

        let candidates = discover_torrent_candidates(
            input,
            &TorrentDiscoveryOptions {
                input_kind: TorrentDiscoveryInputKind::Html,
                base_url: Some("https://example.com/releases/".into()),
                base_path: None,
            },
        )
        .unwrap();

        assert!(candidates.iter().any(|candidate| {
            candidate.kind == TorrentDiscoveryKind::TorrentFile
                && candidate.locator == "https://example.com/payload.torrent"
        }));
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.kind == TorrentDiscoveryKind::Magnet)
        );
        assert!(candidates.iter().any(|candidate| {
            candidate.kind == TorrentDiscoveryKind::Tracker
                && candidate.locator == "udp://tracker.example/announce"
        }));
    }

    #[test]
    fn discovers_links_from_feed_content() {
        let feed = r#"<?xml version="1.0" encoding="utf-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom">
          <title>Releases</title>
          <entry>
            <title>Build</title>
            <link href="https://example.com/build.torrent" />
            <summary>magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01</summary>
          </entry>
        </feed>"#;

        let candidates = discover_torrent_candidates(
            feed,
            &TorrentDiscoveryOptions {
                input_kind: TorrentDiscoveryInputKind::Feed,
                base_url: None,
                base_path: None,
            },
        )
        .unwrap();

        assert!(candidates.iter().any(|candidate| {
            candidate.kind == TorrentDiscoveryKind::TorrentFile
                && candidate.locator == "https://example.com/build.torrent"
        }));
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.kind == TorrentDiscoveryKind::Magnet)
        );
    }

    #[test]
    fn discovers_plain_text_locators() {
        let input =
            "mirror https://example.com/file.torrent tracker udp://tracker.example:1337/announce";
        let candidates =
            discover_torrent_candidates(input, &TorrentDiscoveryOptions::default()).unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].kind, TorrentDiscoveryKind::TorrentFile);
        assert_eq!(candidates[1].kind, TorrentDiscoveryKind::Tracker);
    }

    #[test]
    fn resolves_relative_local_torrent_links_against_base_path() {
        let candidates = discover_torrent_candidates(
            r#"<a href="nested/payload.torrent">Payload</a>"#,
            &TorrentDiscoveryOptions {
                input_kind: TorrentDiscoveryInputKind::Html,
                base_url: None,
                base_path: Some(std::path::PathBuf::from("/tmp/discovery")),
            },
        )
        .unwrap();

        assert!(candidates.iter().any(|candidate| {
            candidate.kind == TorrentDiscoveryKind::TorrentFile
                && candidate.locator == "/tmp/discovery/nested/payload.torrent"
        }));
    }
}
