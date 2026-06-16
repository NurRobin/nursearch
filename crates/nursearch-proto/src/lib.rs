//! Wire protocol shared between the NurSearch host and its plugins.
//!
//! Messages are exchanged as newline-delimited compact JSON, one object per
//! line, over the plugin's stdin (host → plugin) and stdout (plugin → host).
//! Every message is a JSON object with a `type` discriminator.
//!
//! Two concerns share the channel:
//! - **Root contributions** (`Query` / `Results`): flat, ranked list items the
//!   plugin offers on the launcher's start screen.
//! - **View sessions** (`Activate` / `Event` / `Render` / `Pop` / `Close`): a
//!   stateful, declarative push-view interaction once a plugin item is chosen.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Protocol revision. Host and plugin exchange this at `Initialize`; a mismatch
/// disables the plugin.
pub const PROTOCOL_VERSION: u32 = 1;

// ===========================================================================
// Framing
// ===========================================================================

/// Serialize a message to a single newline-terminated JSON line.
pub fn encode<T: Serialize>(message: &T) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    Ok(line)
}

/// Parse one JSON line into a message.
pub fn decode<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim())
}

// ===========================================================================
// Host -> plugin
// ===========================================================================

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HostMessage {
    /// Handshake; sent once before anything else.
    #[serde(rename_all = "camelCase")]
    Initialize {
        protocol_version: u32,
        host_version: String,
        plugin_id: String,
        /// User-set preference values keyed by manifest preference id.
        #[serde(default)]
        preferences: serde_json::Value,
    },
    /// Root-screen query for global / active-keyword plugins.
    #[serde(rename_all = "camelCase")]
    Query { generation: u64, text: String },
    /// A plugin-owned root item was chosen; start a view session.
    #[serde(rename_all = "camelCase")]
    Activate {
        generation: u64,
        command_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
    },
    /// Input within an active view session.
    #[serde(rename_all = "camelCase")]
    Event { generation: u64, event: ViewEvent },
    /// Response to a plugin's host-capability call.
    #[serde(rename_all = "camelCase")]
    HostResult { id: u64, outcome: HostOutcome },
    /// Graceful stop request.
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ViewEvent {
    /// The search box text changed while this view is active.
    Input { text: String },
    /// An item was highlighted (not yet activated).
    #[serde(rename_all = "camelCase")]
    Select { item_id: String },
    /// An action was invoked (primary = Enter, others via the action menu).
    #[serde(rename_all = "camelCase")]
    Action {
        action_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
    },
    /// A form was submitted with field values keyed by field id.
    Submit { values: BTreeMap<String, String> },
    /// The user requested to go back (Esc) from this view.
    Pop,
}

/// Result handed back to a plugin after it makes a host-capability call.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct HostOutcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl HostOutcome {
    pub fn ok(value: Option<serde_json::Value>) -> Self {
        Self {
            ok: true,
            value,
            error: None,
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            value: None,
            error: Some(message.into()),
        }
    }
}

// ===========================================================================
// Plugin -> host
// ===========================================================================

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PluginMessage {
    /// Handshake reply.
    #[serde(rename_all = "camelCase")]
    Initialized {
        protocol_version: u32,
        #[serde(default)]
        capabilities: Vec<String>,
    },
    /// Incremental root contributions tagged with the query generation.
    #[serde(rename_all = "camelCase")]
    Results {
        generation: u64,
        #[serde(default)]
        items: Vec<ResultItem>,
        /// True when no further results for this generation will follow.
        #[serde(default)]
        done: bool,
    },
    /// Push (default) or replace the top view of the active session. `generation`
    /// echoes the triggering activate/event so the host can drop stale renders
    /// from an out-of-order async plugin; 0 means unstamped (always accepted).
    #[serde(rename_all = "camelCase")]
    Render {
        #[serde(default)]
        generation: u64,
        #[serde(default)]
        replace: bool,
        view: View,
    },
    /// Pop the top view (back one level). `generation` echoes the triggering
    /// event so stale pops are dropped; 0 means unstamped (always accepted).
    Pop {
        #[serde(default)]
        generation: u64,
    },
    /// End the session. `generation` echoes the triggering event (0 = unstamped).
    #[serde(rename_all = "camelCase")]
    Close {
        #[serde(default)]
        generation: u64,
        #[serde(default)]
        hide_launcher: bool,
    },
    /// Invoke a host capability; correlated by `id` with a `HostResult`.
    #[serde(rename_all = "camelCase")]
    HostCall { id: u64, call: HostCall },
    /// Diagnostic logging surfaced by the host.
    Log { level: String, message: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "name", rename_all = "camelCase")]
pub enum HostCall {
    ClipboardSet {
        text: String,
    },
    Open {
        target: String,
    },
    Run {
        argv: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Toast {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
    StorageGet {
        key: String,
    },
    StorageSet {
        key: String,
        value: String,
    },
    StorageDelete {
        key: String,
    },
    #[serde(rename_all = "camelCase")]
    StorageList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
    },
    CloseLauncher,
}

/// A flat, rankable item contributed to the root screen.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ResultItem {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Relative score hint; the host blends it with usage history.
    #[serde(default)]
    pub score: i64,
    /// Activating this item starts a view session with this command id.
    pub command_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

// ===========================================================================
// Declarative view tree
// ===========================================================================

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum View {
    List(ListView),
    Detail(DetailView),
    Form(FormView),
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DetailView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetaPair>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct FormView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accessories: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MetaPair {
    pub label: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub id: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<FieldOption>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FieldType {
    Text,
    Password,
    Number,
    Select,
    Checkbox,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FieldOption {
    pub value: String,
    pub label: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    #[serde(default)]
    pub kind: ActionKind,
}

/// What an action does. Host-handled kinds run without a round-trip; `Plugin`
/// re-enters the plugin via an `Event::Action`.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(tag = "do", rename_all = "camelCase")]
pub enum ActionKind {
    /// Re-enter the plugin so it can react (default).
    #[default]
    Plugin,
    Copy {
        text: String,
    },
    OpenUrl {
        url: String,
    },
    Run {
        argv: Vec<String>,
    },
    Paste {
        text: String,
    },
    Close,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_message_round_trips() {
        let message = HostMessage::Query {
            generation: 7,
            text: "fire".to_string(),
        };
        let line = encode(&message).unwrap();
        assert!(line.ends_with('\n'));
        assert!(line.contains("\"type\":\"query\""));
        let back: HostMessage = decode(&line).unwrap();
        assert!(matches!(back, HostMessage::Query { generation: 7, .. }));
    }

    #[test]
    fn render_with_list_view_round_trips() {
        let message = PluginMessage::Render {
            generation: 0,
            replace: false,
            view: View::List(ListView {
                title: Some("Windows".to_string()),
                items: vec![Item {
                    id: "1".to_string(),
                    title: "Firefox".to_string(),
                    subtitle: None,
                    icon: None,
                    accessories: vec!["Workspace 2".to_string()],
                    actions: vec![Action {
                        id: "focus".to_string(),
                        title: "Focus".to_string(),
                        icon: None,
                        shortcut: None,
                        kind: ActionKind::Plugin,
                    }],
                }],
                ..Default::default()
            }),
        };
        let line = encode(&message).unwrap();
        let back: PluginMessage = decode(&line).unwrap();
        match back {
            PluginMessage::Render {
                view: View::List(list),
                ..
            } => {
                assert_eq!(list.items.len(), 1);
                assert_eq!(list.items[0].title, "Firefox");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn action_kind_defaults_to_plugin() {
        let json = r#"{"id":"a","title":"Do it"}"#;
        let action: Action = serde_json::from_str(json).unwrap();
        assert!(matches!(action.kind, ActionKind::Plugin));
    }

    #[test]
    fn host_call_tagged_by_name() {
        let call = HostCall::StorageSet {
            key: "last".to_string(),
            value: "x".to_string(),
        };
        let json = serde_json::to_string(&call).unwrap();
        assert!(json.contains("\"name\":\"storageSet\""));
    }
}
