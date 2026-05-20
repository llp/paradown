use crate::domain::{DownloadSpec, SourceKind, SourceSet};
use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use url::form_urlencoded;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentPeerEndpoint {
    pub host: String,
    pub port: u16,
}

impl TorrentPeerEndpoint {
    pub fn parse(value: &str) -> Result<Self, Error> {
        let value = value.trim();
        let (host, port) = value.rsplit_once(':').ok_or_else(|| {
            Error::Other(format!("torrent peer must be HOST:PORT, got '{value}'"))
        })?;
        if host.trim().is_empty() {
            return Err(Error::Other(format!(
                "torrent peer host cannot be blank in '{value}'"
            )));
        }
        let port = port.parse::<u16>().map_err(|err| {
            Error::Other(format!("invalid torrent peer port in '{value}': {err}"))
        })?;
        Ok(Self {
            host: host.to_string(),
            port,
        })
    }
}

impl fmt::Display for TorrentPeerEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentSwarmHints {
    pub trackers: Vec<String>,
    pub peers: Vec<TorrentPeerEndpoint>,
    pub web_seeds: Vec<String>,
}

impl TorrentSwarmHints {
    pub fn is_empty(&self) -> bool {
        self.trackers.is_empty() && self.peers.is_empty() && self.web_seeds.is_empty()
    }

    pub fn merge(&mut self, other: Self) {
        for tracker in other.trackers {
            self.add_tracker(tracker);
        }
        for peer in other.peers {
            self.add_peer(peer);
        }
        for web_seed in other.web_seeds {
            self.add_web_seed(web_seed);
        }
    }

    pub fn add_tracker(&mut self, tracker: impl Into<String>) {
        push_unique(&mut self.trackers, tracker.into());
    }

    pub fn add_peer(&mut self, peer: TorrentPeerEndpoint) {
        if !self.peers.iter().any(|existing| existing == &peer) {
            self.peers.push(peer);
        }
    }

    pub fn add_web_seed(&mut self, web_seed: impl Into<String>) {
        push_unique(&mut self.web_seeds, web_seed.into());
    }

    pub fn from_spec_and_sources(spec: &DownloadSpec, sources: &SourceSet) -> Result<Self, Error> {
        let mut hints = Self::default();
        if let DownloadSpec::Magnet { uri } = spec {
            hints.merge(MagnetLink::parse(uri)?.swarm_hints());
        }
        for source in &sources.sources {
            match source.kind {
                SourceKind::Tracker => hints.add_tracker(source.locator.clone()),
                SourceKind::Peer => hints.add_peer(TorrentPeerEndpoint::parse(&source.locator)?),
                SourceKind::WebSeed => hints.add_web_seed(source.locator.clone()),
                _ => {}
            }
        }
        Ok(hints)
    }

    pub fn enhance_magnet_uri(&self, uri: &str) -> Result<String, Error> {
        MagnetLink::parse(uri)?.enhance(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MagnetExactTopic {
    Btih(String),
    Btmh(String),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagnetParameter {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagnetLink {
    pub uri: String,
    pub exact_topics: Vec<MagnetExactTopic>,
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
    pub peers: Vec<TorrentPeerEndpoint>,
    pub web_seeds: Vec<String>,
    pub exact_sources: Vec<String>,
    pub extensions: Vec<MagnetParameter>,
    params: Vec<MagnetParameter>,
}

impl MagnetLink {
    pub fn parse(uri: &str) -> Result<Self, Error> {
        let parsed = url::Url::parse(uri)?;
        if parsed.scheme() != "magnet" {
            return Err(Error::UnsupportedProtocol(parsed.scheme().to_string()));
        }

        let mut link = Self {
            uri: uri.to_string(),
            exact_topics: Vec::new(),
            display_name: None,
            trackers: Vec::new(),
            peers: Vec::new(),
            web_seeds: Vec::new(),
            exact_sources: Vec::new(),
            extensions: Vec::new(),
            params: Vec::new(),
        };

        for (key, value) in parsed.query_pairs() {
            let key = key.into_owned();
            let value = value.into_owned();
            let parameter = MagnetParameter {
                key: key.clone(),
                value: value.clone(),
            };
            match key.as_str() {
                "xt" => link.exact_topics.push(parse_exact_topic(&value)),
                "dn" => link.display_name = Some(value),
                "tr" => push_unique(&mut link.trackers, value),
                "x.pe" => link.peers.push(TorrentPeerEndpoint::parse(&value)?),
                "ws" => push_unique(&mut link.web_seeds, value),
                "xs" => push_unique(&mut link.exact_sources, value),
                _ => link.extensions.push(parameter.clone()),
            }
            link.params.push(parameter);
        }

        Ok(link)
    }

    pub fn swarm_hints(&self) -> TorrentSwarmHints {
        TorrentSwarmHints {
            trackers: self.trackers.clone(),
            peers: self.peers.clone(),
            web_seeds: self.web_seeds.clone(),
        }
    }

    pub fn enhance(&self, hints: &TorrentSwarmHints) -> Result<String, Error> {
        let mut pairs = self.params.clone();
        let mut trackers = values_for_key(&pairs, "tr");
        let mut peers = values_for_key(&pairs, "x.pe");
        let mut web_seeds = values_for_key(&pairs, "ws");

        for tracker in &hints.trackers {
            if trackers.insert(tracker.clone()) {
                pairs.push(MagnetParameter {
                    key: "tr".into(),
                    value: tracker.clone(),
                });
            }
        }
        for peer in &hints.peers {
            let peer = peer.to_string();
            if peers.insert(peer.clone()) {
                pairs.push(MagnetParameter {
                    key: "x.pe".into(),
                    value: peer,
                });
            }
        }
        for web_seed in &hints.web_seeds {
            if web_seeds.insert(web_seed.clone()) {
                pairs.push(MagnetParameter {
                    key: "ws".into(),
                    value: web_seed.clone(),
                });
            }
        }

        let mut encoded = form_urlencoded::Serializer::new(String::new());
        for pair in pairs {
            encoded.append_pair(&pair.key, &pair.value);
        }
        Ok(format!("magnet:?{}", encoded.finish()))
    }
}

fn parse_exact_topic(value: &str) -> MagnetExactTopic {
    if let Some(hash) = value.strip_prefix("urn:btih:") {
        MagnetExactTopic::Btih(hash.to_ascii_lowercase())
    } else if let Some(hash) = value.strip_prefix("urn:btmh:") {
        MagnetExactTopic::Btmh(hash.to_ascii_lowercase())
    } else {
        MagnetExactTopic::Other(value.to_string())
    }
}

fn values_for_key(params: &[MagnetParameter], key: &str) -> HashSet<String> {
    params
        .iter()
        .filter(|param| param.key == key)
        .map(|param| param.value.clone())
        .collect()
}

fn push_unique(values: &mut Vec<String>, value: String) {
    let value = value.trim();
    if !value.is_empty() && !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{MagnetExactTopic, MagnetLink, TorrentPeerEndpoint, TorrentSwarmHints};

    #[test]
    fn parses_standard_and_extended_magnet_parameters() {
        let magnet = MagnetLink::parse(
            "magnet:?xt=urn:btih:ABCDEF0123456789ABCDEF0123456789ABCDEF01&dn=video&tr=udp%3A%2F%2Ftracker.example%3A1337%2Fannounce&x.pe=127.0.0.1%3A6881&ws=https%3A%2F%2Fseed.example%2Ffile&x._t-v1=42",
        )
        .unwrap();

        assert_eq!(
            magnet.exact_topics,
            vec![MagnetExactTopic::Btih(
                "abcdef0123456789abcdef0123456789abcdef01".into()
            )]
        );
        assert_eq!(magnet.display_name.as_deref(), Some("video"));
        assert_eq!(magnet.trackers, vec!["udp://tracker.example:1337/announce"]);
        assert_eq!(
            magnet.peers,
            vec![TorrentPeerEndpoint {
                host: "127.0.0.1".into(),
                port: 6881
            }]
        );
        assert_eq!(magnet.web_seeds, vec!["https://seed.example/file"]);
        assert_eq!(magnet.extensions[0].key, "x._t-v1");
    }

    #[test]
    fn enhances_magnet_without_duplicating_existing_hints() {
        let magnet = MagnetLink::parse(
            "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01&tr=udp%3A%2F%2Ftracker.example%2Fannounce",
        )
        .unwrap();
        let enhanced = magnet
            .enhance(&TorrentSwarmHints {
                trackers: vec![
                    "udp://tracker.example/announce".into(),
                    "udp://tracker2.example/announce".into(),
                ],
                peers: vec![TorrentPeerEndpoint {
                    host: "127.0.0.1".into(),
                    port: 6881,
                }],
                web_seeds: vec!["https://seed.example/file".into()],
            })
            .unwrap();

        assert!(enhanced.contains("tr=udp%3A%2F%2Ftracker.example%2Fannounce"));
        assert!(enhanced.contains("tr=udp%3A%2F%2Ftracker2.example%2Fannounce"));
        assert_eq!(enhanced.matches("tracker.example%2Fannounce").count(), 1);
        assert!(enhanced.contains("x.pe=127.0.0.1%3A6881"));
        assert!(enhanced.contains("ws=https%3A%2F%2Fseed.example%2Ffile"));
    }
}
