//! Who has a microphone open, from PipeWire's graph: every capture stream
//! (`Stream/Input/Audio`) linked to a source (`Audio/Source...`). A stream
//! that records what the speakers play links to a sink's monitor ports
//! instead, and peak meters set `stream.monitor`; neither counts.

use crate::model::MicUser;
use std::collections::{BTreeMap, HashMap};

/// A node's properties, as the daemon collects them from the registry and
/// the node's info.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Node {
    pub class: String,
    pub name: String,
    pub description: String,
    pub app: String,
    pub binary: String,
    pub pid: Option<u32>,
    pub client: Option<u32>,
    pub monitor: bool,
}

impl Node {
    /// Fills the fields from a property list.
    pub fn from_props<'a>(props: impl IntoIterator<Item = (&'a str, &'a str)>) -> Node {
        let mut n = Node::default();
        n.apply(props);
        n
    }

    /// Updates the fields from a property list; keys it lacks keep their
    /// value.
    pub fn apply<'a>(&mut self, props: impl IntoIterator<Item = (&'a str, &'a str)>) {
        for (k, v) in props {
            match k {
                "media.class" => self.class = v.into(),
                "node.name" => self.name = v.into(),
                "node.description" => self.description = v.into(),
                "node.nick" if self.description.is_empty() => self.description = v.into(),
                "application.name" => self.app = v.into(),
                "application.process.binary" => self.binary = v.into(),
                "application.process.id" => self.pid = v.parse().ok().or(self.pid),
                "client.id" => self.client = v.parse().ok(),
                "stream.monitor" => self.monitor = v == "true" || v == "1",
                _ => {}
            }
        }
    }

    pub fn is_capture_stream(&self) -> bool {
        self.class == "Stream/Input/Audio" && !self.monitor
    }

    pub fn is_mic(&self) -> bool {
        self.class.starts_with("Audio/Source") || self.class == "Audio/Duplex"
    }
}

/// A link between two nodes' ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub output: u32,
    pub input: u32,
}

impl Link {
    pub fn from_props<'a>(props: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<Link> {
        let mut out = None;
        let mut inp = None;
        for (k, v) in props {
            match k {
                "link.output.node" => out = v.parse().ok(),
                "link.input.node" => inp = v.parse().ok(),
                _ => {}
            }
        }
        Some(Link {
            output: out?,
            input: inp?,
        })
    }
}

/// What a short name for an app looks like: `chrome`, not "Google Chrome
/// input".
fn short(n: &Node) -> String {
    if !n.binary.is_empty() {
        return n.binary.clone();
    }
    let app = n.app.trim();
    let app = app
        .strip_suffix(" input")
        .or_else(|| app.strip_suffix(" Input"))
        .unwrap_or(app);
    if !app.is_empty() {
        app.to_string()
    } else {
        n.name.clone()
    }
}

/// The apps holding a microphone, one entry per app.
///
/// `client_pids` maps a PipeWire client to its process (`pipewire.sec.pid`),
/// for native clients that do not name their own pid. `ours` are the pids of
/// Logbook's recorders.
pub fn users(
    nodes: &BTreeMap<u32, Node>,
    links: &[Link],
    client_pids: &HashMap<u32, u32>,
    ours: &[u32],
    ignored: impl Fn(&str, &str) -> bool,
) -> Vec<MicUser> {
    let mut out: Vec<MicUser> = vec![];
    for (id, n) in nodes {
        if !n.is_capture_stream() {
            continue;
        }
        let Some(source) = links
            .iter()
            .filter(|l| l.input == *id)
            .filter_map(|l| nodes.get(&l.output))
            .find(|s| s.is_mic())
        else {
            continue;
        };
        let pid = n
            .pid
            .or_else(|| n.client.and_then(|c| client_pids.get(&c).copied()));
        let name = short(n);
        if ignored(&n.binary, &n.app) || ignored(&name, &name) {
            continue;
        }
        let ours = pid.is_some_and(|p| ours.contains(&p));
        let lower = format!("{} {} {}", n.binary, n.app, n.name).to_lowercase();
        let user = MicUser {
            name,
            app: n.app.clone(),
            binary: n.binary.clone(),
            pid,
            source: if source.description.is_empty() {
                source.name.clone()
            } else {
                source.description.clone()
            },
            ours,
            dictation: !ours && lower.contains("voxtype"),
        };
        if !out
            .iter()
            .any(|u| u.name == user.name && u.ours == user.ours)
        {
            out.push(user);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> (BTreeMap<u32, Node>, Vec<Link>) {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            50,
            Node::from_props([
                ("media.class", "Audio/Source"),
                ("node.name", "alsa_input.usb-Shure_MV7"),
                ("node.description", "Shure MV7"),
            ]),
        );
        nodes.insert(
            94,
            Node::from_props([
                ("media.class", "Audio/Sink"),
                ("node.name", "alsa_output.usb-SteelSeries"),
                ("node.description", "Arctis Nova Pro Wireless"),
            ]),
        );
        // Chrome through pipewire-pulse: two streams on the mic.
        for id in [83, 143] {
            nodes.insert(
                id,
                Node::from_props([
                    ("media.class", "Stream/Input/Audio"),
                    ("application.name", "Google Chrome input"),
                    ("node.name", "Google Chrome input"),
                    ("application.process.binary", "chrome"),
                    ("application.process.id", "27598"),
                    ("client.id", "115"),
                ]),
            );
        }
        // Our pw-record, a native client that names no pid on the node.
        nodes.insert(
            160,
            Node::from_props([
                ("media.class", "Stream/Input/Audio"),
                ("node.name", "pw-record"),
                ("application.name", "pw-record"),
                ("client.id", "161"),
            ]),
        );
        // wf-recorder on the speakers' monitor.
        nodes.insert(
            170,
            Node::from_props([
                ("media.class", "Stream/Input/Audio"),
                ("application.process.binary", "wf-recorder"),
            ]),
        );
        // pavucontrol's peak meter.
        nodes.insert(
            180,
            Node::from_props([
                ("media.class", "Stream/Input/Audio"),
                ("application.process.binary", "pavucontrol"),
                ("stream.monitor", "true"),
            ]),
        );
        // A stream that is open but not linked.
        nodes.insert(
            190,
            Node::from_props([
                ("media.class", "Stream/Input/Audio"),
                ("application.process.binary", "zoom"),
            ]),
        );
        let links = vec![
            Link {
                output: 50,
                input: 83,
            },
            Link {
                output: 50,
                input: 143,
            },
            Link {
                output: 50,
                input: 160,
            },
            Link {
                output: 94,
                input: 170,
            },
            Link {
                output: 50,
                input: 180,
            },
        ];
        (nodes, links)
    }

    #[test]
    fn finds_who_has_the_mic() {
        let (nodes, links) = graph();
        let pids = HashMap::from([(161, 4242)]);
        let u = users(&nodes, &links, &pids, &[4242], |_, _| false);
        assert_eq!(u.len(), 2, "{u:#?}");
        let chrome = u.iter().find(|x| x.name == "chrome").unwrap();
        assert_eq!(chrome.pid, Some(27598));
        assert_eq!(chrome.source, "Shure MV7");
        assert!(!chrome.ours);
        let rec = u.iter().find(|x| x.name == "pw-record").unwrap();
        assert!(rec.ours);
        assert_eq!(rec.pid, Some(4242));
    }

    #[test]
    fn ignore_list_and_dictation() {
        let (mut nodes, mut links) = graph();
        nodes.insert(
            200,
            Node::from_props([
                ("media.class", "Stream/Input/Audio"),
                ("application.process.binary", "voxtype"),
            ]),
        );
        links.push(Link {
            output: 50,
            input: 200,
        });
        let u = users(&nodes, &links, &HashMap::new(), &[], |b, _| b == "chrome");
        assert!(u.iter().all(|x| x.name != "chrome"));
        assert!(u.iter().find(|x| x.name == "voxtype").unwrap().dictation);
        // Without its pid, pw-record is someone else's.
        assert!(!u.iter().find(|x| x.name == "pw-record").unwrap().ours);
    }

    #[test]
    fn link_props() {
        let l = Link::from_props([
            ("link.output.port", "70"),
            ("link.output.node", "78"),
            ("link.input.node", "83"),
        ]);
        assert_eq!(
            l,
            Some(Link {
                output: 78,
                input: 83
            })
        );
        assert_eq!(Link::from_props([("link.output.node", "1")]), None);
    }
}
