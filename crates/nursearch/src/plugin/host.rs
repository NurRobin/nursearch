//! The plugin host: discovers plugins, manages their process lifecycle, runs
//! the initialize handshake, and routes messages to a [`HostSink`].

use super::manifest::{Activation, ActivationMode, Manifest, Plugin, discover};
use super::process::PluginProcess;
use nursearch_proto::{
    HostCall, HostMessage, HostOutcome, PROTOCOL_VERSION, PluginMessage, ResultItem, View,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

/// The launcher's side of the plugin channel: where contributions, view
/// renders, and host-capability calls are delivered. Implemented by the UI
/// layer (rendering) and the capability layer (clipboard/storage/…).
pub trait HostSink {
    /// Root-screen contributions for a query generation.
    fn results(&self, plugin_id: &str, generation: u64, items: Vec<ResultItem>, done: bool);
    /// Push (or replace) a view in the active session. `generation` echoes the
    /// triggering activate/event so stale renders can be dropped.
    fn render(&self, plugin_id: &str, generation: u64, replace: bool, view: View);
    /// Pop one view in the active session.
    fn pop(&self, plugin_id: &str, generation: u64);
    /// End the active session.
    fn close(&self, plugin_id: &str, generation: u64, hide_launcher: bool);
    /// Execute a host-capability call and return its outcome.
    fn host_call(&self, plugin_id: &str, call: HostCall) -> HostOutcome;
}

struct Running {
    process: PluginProcess,
    manifest: Manifest,
    ready: bool,
}

struct HostInner {
    plugins: Vec<Plugin>,
    running: HashMap<String, Running>,
    /// Plugins disabled for this session (protocol mismatch or malformed output).
    disabled: HashSet<String>,
    sink: Rc<dyn HostSink>,
}

/// Manages every plugin process. Cheap to clone (shares one inner cell).
#[derive(Clone)]
pub struct PluginHost {
    inner: Rc<RefCell<HostInner>>,
}

impl PluginHost {
    /// Discover plugins and build a host that routes to `sink`.
    pub fn new(sink: Rc<dyn HostSink>) -> Self {
        let plugins = discover();
        log::info!("discovered {} plugin(s)", plugins.len());
        Self {
            inner: Rc::new(RefCell::new(HostInner {
                plugins,
                running: HashMap::new(),
                disabled: HashSet::new(),
                sink,
            })),
        }
    }

    /// Plugins whose contributions should be requested for `query`: every
    /// global plugin, plus the keyword plugin whose prefix matches.
    pub fn contributors_for(&self, query: &str) -> Vec<String> {
        let trimmed = query.trim();
        let inner = self.inner.borrow();
        inner
            .plugins
            .iter()
            .filter(|plugin| !inner.disabled.contains(&plugin.manifest.id))
            .filter(|plugin| activation_matches(&plugin.manifest.activation, trimmed))
            .map(|plugin| plugin.manifest.id.clone())
            .collect()
    }

    /// If `query` begins with a plugin keyword (`w …`), return that plugin id
    /// and the remaining text after the keyword.
    pub fn keyword_match(&self, query: &str) -> Option<(String, String)> {
        let trimmed = query.trim_start();
        let inner = self.inner.borrow();
        inner.plugins.iter().find_map(|plugin| {
            if inner.disabled.contains(&plugin.manifest.id) {
                return None;
            }
            let activation = &plugin.manifest.activation;
            if activation.mode != ActivationMode::Keyword {
                return None;
            }
            let keyword = activation.keyword.as_deref()?;
            let rest = trimmed.strip_prefix(keyword)?;
            if rest.is_empty() || rest.starts_with(' ') {
                Some((plugin.manifest.id.clone(), rest.trim_start().to_string()))
            } else {
                None
            }
        })
    }

    /// Look up a discovered plugin's manifest.
    pub fn manifest(&self, id: &str) -> Option<Manifest> {
        self.inner
            .borrow()
            .plugins
            .iter()
            .find(|plugin| plugin.manifest.id == id)
            .map(|plugin| plugin.manifest.clone())
    }

    /// Ensure a plugin process is running; spawn and handshake if not. Returns
    /// false if the plugin is unknown or could not be started.
    pub fn ensure_started(&self, id: &str) -> bool {
        {
            let inner = self.inner.borrow();
            if inner.running.contains_key(id) {
                return true;
            }
            if inner.disabled.contains(id) {
                return false; // disabled for this session; do not restart
            }
        }
        let plugin = match self
            .inner
            .borrow()
            .plugins
            .iter()
            .find(|plugin| plugin.manifest.id == id)
            .cloned()
        {
            Some(plugin) => plugin,
            None => {
                log::warn!("requested unknown plugin '{id}'");
                return false;
            }
        };

        let weak = Rc::downgrade(&self.inner);
        let on_message = {
            let weak = weak.clone();
            Rc::new(move |pid: &str, message: PluginMessage| {
                handle_message(&weak, pid, message);
            })
        };
        let on_exit = {
            let weak = weak.clone();
            Rc::new(move |pid: &str| {
                if let Some(inner) = weak.upgrade() {
                    inner.borrow_mut().running.remove(pid);
                    log::info!("plugin '{pid}' exited");
                }
            })
        };
        // Malformed plugin output is fatal: terminate and disable the plugin.
        let on_error = Rc::new(move |pid: &str| {
            if let Some(inner) = weak.upgrade() {
                let mut inner = inner.borrow_mut();
                if let Some(running) = inner.running.remove(pid) {
                    running.process.kill();
                }
                inner.disabled.insert(pid.to_string());
                log::warn!("disabled plugin '{pid}' after a protocol error");
            }
        });

        let id_owned = id.to_string();
        let argv = plugin.launch_argv();
        match PluginProcess::spawn(&id_owned, &argv, &plugin.dir, on_message, on_exit, on_error) {
            Ok(process) => {
                let init = HostMessage::Initialize {
                    protocol_version: PROTOCOL_VERSION,
                    host_version: env!("CARGO_PKG_VERSION").to_string(),
                    plugin_id: id_owned.clone(),
                    preferences: serde_json::Value::Null,
                };
                if let Err(err) = process.send(&init) {
                    log::warn!("could not initialize plugin '{id_owned}': {err}");
                    return false;
                }
                self.inner.borrow_mut().running.insert(
                    id_owned,
                    Running {
                        process,
                        manifest: plugin.manifest,
                        ready: false,
                    },
                );
                true
            }
            Err(err) => {
                log::warn!("could not start plugin '{id_owned}': {err}");
                false
            }
        }
    }

    /// Send a message to a running plugin, starting it first if needed.
    pub fn send(&self, id: &str, message: &HostMessage) -> bool {
        if !self.ensure_started(id) {
            return false;
        }
        let inner = self.inner.borrow();
        match inner.running.get(id) {
            Some(running) => match running.process.send(message) {
                Ok(()) => true,
                Err(err) => {
                    log::warn!("send to plugin '{id}' failed: {err}");
                    false
                }
            },
            None => false,
        }
    }

    /// Gracefully ask every running plugin to shut down.
    pub fn shutdown_all(&self) {
        let inner = self.inner.borrow();
        for running in inner.running.values() {
            let _ = running.process.send(&HostMessage::Shutdown);
        }
    }
}

/// Route a decoded plugin message: handle the handshake and host calls here,
/// forward everything else to the sink.
fn handle_message(weak: &Weak<RefCell<HostInner>>, plugin_id: &str, message: PluginMessage) {
    let Some(inner_rc) = weak.upgrade() else {
        return;
    };

    // Everything except the handshake and logging requires a completed,
    // version-compatible handshake first.
    let needs_handshake = !matches!(
        message,
        PluginMessage::Initialized { .. } | PluginMessage::Log { .. }
    );
    if needs_handshake {
        let ready = inner_rc
            .borrow()
            .running
            .get(plugin_id)
            .map(|running| running.ready)
            .unwrap_or(false);
        if !ready {
            log::warn!("ignoring message from '{plugin_id}' before its handshake completed");
            return;
        }
    }

    match message {
        PluginMessage::Initialized {
            protocol_version, ..
        } => {
            let mut inner = inner_rc.borrow_mut();
            if protocol_version != PROTOCOL_VERSION {
                log::warn!("plugin '{plugin_id}' reported protocol {protocol_version}, disabling");
                if let Some(running) = inner.running.remove(plugin_id) {
                    running.process.kill();
                }
                inner.disabled.insert(plugin_id.to_string());
                return;
            }
            if let Some(running) = inner.running.get_mut(plugin_id) {
                running.ready = true;
                log::debug!("plugin '{plugin_id}' ready");
            }
        }
        PluginMessage::Results {
            generation,
            items,
            done,
        } => {
            let sink = inner_rc.borrow().sink.clone();
            sink.results(plugin_id, generation, items, done);
        }
        PluginMessage::Render {
            generation,
            replace,
            view,
        } => {
            let sink = inner_rc.borrow().sink.clone();
            sink.render(plugin_id, generation, replace, view);
        }
        PluginMessage::Pop { generation } => {
            let sink = inner_rc.borrow().sink.clone();
            sink.pop(plugin_id, generation);
        }
        PluginMessage::Close {
            generation,
            hide_launcher,
        } => {
            let sink = inner_rc.borrow().sink.clone();
            sink.close(plugin_id, generation, hide_launcher);
        }
        PluginMessage::HostCall { id, call } => {
            let sink = inner_rc.borrow().sink.clone();
            let outcome = sink.host_call(plugin_id, call);
            let reply = HostMessage::HostResult { id, outcome };
            if let Some(running) = inner_rc.borrow().running.get(plugin_id)
                && let Err(err) = running.process.send(&reply) {
                    log::warn!("could not reply to host call from '{plugin_id}': {err}");
                }
        }
        PluginMessage::Log { level, message } => {
            log::info!("[plugin {plugin_id}] {level}: {message}");
        }
    }
}

/// A plugin contributes to the root for `query` when it is global, or its
/// keyword prefixes the query.
fn activation_matches(activation: &Activation, query: &str) -> bool {
    match activation.mode {
        ActivationMode::Global => true,
        ActivationMode::Keyword => activation
            .keyword
            .as_deref()
            .map(|keyword| {
                query
                    .strip_prefix(keyword)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
            })
            .unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::manifest::parse_manifest;

    fn activation(toml: &str) -> Activation {
        parse_manifest(toml).unwrap().activation
    }

    #[test]
    fn global_plugin_always_contributes() {
        let act = activation(
            "id='x'\nname='X'\nprotocol_version=1\nentry=['x']\n[activation]\nmode='global'",
        );
        assert!(activation_matches(&act, "anything"));
    }

    #[test]
    fn keyword_plugin_only_on_prefix() {
        let act = activation(
            "id='x'\nname='X'\nprotocol_version=1\nentry=['x']\n[activation]\nmode='keyword'\nkeyword='w'",
        );
        assert!(activation_matches(&act, "w"));
        assert!(activation_matches(&act, "w firefox"));
        assert!(!activation_matches(&act, "world")); // "w" not a standalone keyword
        assert!(!activation_matches(&act, "firefox"));
    }
}
