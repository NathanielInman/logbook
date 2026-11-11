//! Watches PipeWire's registry for capture streams and the links that feed
//! them, so the lamp knows who has a microphone open. The graph arrives as
//! registry events on PipeWire's own loop; nothing is polled and no
//! `pw-dump` runs. Capture streams are bound for their info, which names
//! the process behind a pipewire-pulse stream (Chrome, Zoom).

use logbook_core::mic::{Link, Node};
use pipewire as pw;
use pw::types::ObjectType;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::Duration;

/// The parts of the graph that matter, sent whenever they change.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    pub nodes: BTreeMap<u32, Node>,
    pub links: Vec<Link>,
    pub client_pids: HashMap<u32, u32>,
}

#[derive(Default)]
struct Live {
    nodes: BTreeMap<u32, Node>,
    links: BTreeMap<u32, Link>,
    clients: HashMap<u32, u32>,
    /// Bound capture streams and their info listeners, kept alive.
    bound: HashMap<u32, (pw::node::Node, pw::node::NodeListener)>,
}

impl Live {
    fn graph(&self) -> Graph {
        Graph {
            nodes: self.nodes.clone(),
            links: self.links.values().copied().collect(),
            client_pids: self.clients.clone(),
        }
    }
}

fn pairs(d: &pw::spa::utils::dict::DictRef) -> Vec<(String, String)> {
    d.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn as_refs(v: &[(String, String)]) -> impl Iterator<Item = (&str, &str)> {
    v.iter().map(|(k, v)| (k.as_str(), v.as_str()))
}

/// Connects and follows the graph until PipeWire goes away. `send` gets
/// the graph after each change, and `None` when the connection is lost.
fn session(send: &Rc<dyn Fn(Option<Graph>)>) -> Result<(), pw::Error> {
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;
    let live = Rc::new(RefCell::new(Live::default()));

    let publish = {
        let live = live.clone();
        let send = send.clone();
        Rc::new(move || send(Some(live.borrow().graph())))
    };

    let ml = main_loop.downgrade();
    let _core_listener = core
        .add_listener_local()
        .error(move |id, _seq, _res, message| {
            if id == pw::core::PW_ID_CORE {
                eprintln!("logbook: pipewire: {message}");
                if let Some(ml) = ml.upgrade() {
                    ml.quit();
                }
            }
        })
        .register();

    let reg_weak = registry.downgrade();
    let live_add = live.clone();
    let publish_add = publish.clone();
    let live_rm = live.clone();
    let publish_rm = publish.clone();
    let _registry_listener = registry
        .add_listener_local()
        .global(move |obj| {
            let Some(props) = obj.props.as_ref() else {
                return;
            };
            let kv = pairs(props);
            match obj.type_ {
                ObjectType::Node => {
                    let node = Node::from_props(as_refs(&kv));
                    let capture = node.class == "Stream/Input/Audio";
                    live_add.borrow_mut().nodes.insert(obj.id, node);
                    if capture {
                        if let Some(reg) = reg_weak.upgrade() {
                            if let Ok(proxy) = reg.bind::<pw::node::Node, _>(obj) {
                                let id = obj.id;
                                let live = live_add.clone();
                                let publish = publish_add.clone();
                                let listener = proxy
                                    .add_listener_local()
                                    .info(move |info| {
                                        if let Some(p) = info.props() {
                                            let kv = pairs(p);
                                            if let Some(n) = live.borrow_mut().nodes.get_mut(&id) {
                                                n.apply(as_refs(&kv));
                                            }
                                            publish();
                                        }
                                    })
                                    .register();
                                live_add.borrow_mut().bound.insert(id, (proxy, listener));
                            }
                        }
                    }
                    publish_add();
                }
                ObjectType::Link => {
                    if let Some(l) = Link::from_props(as_refs(&kv)) {
                        live_add.borrow_mut().links.insert(obj.id, l);
                        publish_add();
                    }
                }
                ObjectType::Client => {
                    if let Some(pid) = props.get("pipewire.sec.pid").and_then(|p| p.parse().ok()) {
                        live_add.borrow_mut().clients.insert(obj.id, pid);
                    }
                }
                _ => {}
            }
        })
        .global_remove(move |id| {
            let mut l = live_rm.borrow_mut();
            let changed = l.nodes.remove(&id).is_some() | l.links.remove(&id).is_some();
            l.clients.remove(&id);
            l.bound.remove(&id);
            drop(l);
            if changed {
                publish_rm();
            }
        })
        .register();

    main_loop.run();
    Ok(())
}

/// Follows PipeWire forever on the calling thread, reconnecting after it
/// restarts.
pub fn watch(send: impl Fn(Option<Graph>) + 'static) {
    pw::init();
    let send: Rc<dyn Fn(Option<Graph>)> = Rc::new(send);
    let mut wait = Duration::from_secs(2);
    loop {
        match session(&send) {
            Ok(()) => wait = Duration::from_secs(2),
            Err(e) => eprintln!("logbook: pipewire: {e}"),
        }
        send(None);
        std::thread::sleep(wait);
        wait = (wait * 2).min(Duration::from_secs(30));
    }
}
