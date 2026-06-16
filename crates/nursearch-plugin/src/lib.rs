//! SDK for writing NurSearch plugins in Rust.
//!
//! Implement [`Plugin`] and call [`run`]; the SDK drives the newline-delimited
//! JSON-RPC loop, the initialize handshake, and host-capability calls so you
//! work with plain Rust values instead of the wire protocol. Plugin methods
//! receive a [`HostApi`] for clipboard, open/run, toast, and persistent storage.

use nursearch_proto::{
    HostCall, HostMessage, HostOutcome, PROTOCOL_VERSION, PluginMessage, ResultItem, View,
    ViewEvent, decode, encode,
};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write, stdin, stdout};

/// What a plugin does in response to an activation or event.
pub enum Response {
    /// Push a new view onto the stack.
    Render(View),
    /// Replace the current top view.
    Replace(View),
    /// Pop the current view.
    Pop,
    /// End the session, optionally hiding the launcher.
    Close { hide: bool },
}

/// Host capabilities a plugin may call. Calls block until the host replies.
pub trait HostApi {
    fn clipboard_set(&mut self, text: &str);
    fn open(&mut self, target: &str);
    fn run_command(&mut self, argv: Vec<String>);
    fn toast(&mut self, text: &str);
    fn storage_get(&mut self, key: &str) -> Option<String>;
    fn storage_set(&mut self, key: &str, value: &str);
    fn storage_delete(&mut self, key: &str);
    fn storage_list(&mut self, prefix: Option<&str>) -> Vec<(String, String)>;
}

/// A plugin's behavior.
pub trait Plugin {
    /// Root-screen contributions for a query (only called for global / active
    /// keyword plugins). Default: no contributions.
    fn query(&mut self, _host: &mut dyn HostApi, _text: &str) -> Vec<ResultItem> {
        Vec::new()
    }

    /// A root item was activated; return the first view of the session.
    fn activate(
        &mut self,
        host: &mut dyn HostApi,
        command_id: &str,
        item_id: Option<String>,
    ) -> Option<Response>;

    /// An event arrived within the session.
    fn event(&mut self, host: &mut dyn HostApi, event: ViewEvent) -> Option<Response>;
}

/// The plugin's I/O channel to the host.
struct Io<R: BufRead, W: Write> {
    reader: R,
    writer: W,
    next_id: u64,
    /// Messages read from the host while waiting for a host-call result. They
    /// are replayed by `read_message` so the main loop never loses them.
    pending: VecDeque<HostMessage>,
}

impl<R: BufRead, W: Write> Io<R, W> {
    fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            next_id: 0,
            pending: VecDeque::new(),
        }
    }

    /// Next message for the main loop: buffered ones first, then the stream.
    fn read_message(&mut self) -> Option<HostMessage> {
        if let Some(message) = self.pending.pop_front() {
            return Some(message);
        }
        self.read_raw()
    }

    /// Read one message directly from the host stream.
    fn read_raw(&mut self) -> Option<HostMessage> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None,
                Ok(_) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match decode::<HostMessage>(&line) {
                        Ok(message) => return Some(message),
                        Err(err) => {
                            eprintln!("plugin: ignoring undecodable host message: {err}");
                            continue;
                        }
                    }
                }
                Err(_) => return None,
            }
        }
    }

    fn send(&mut self, message: &PluginMessage) {
        if let Ok(line) = encode(message) {
            let _ = self.writer.write_all(line.as_bytes());
            let _ = self.writer.flush();
        }
    }

    fn call(&mut self, call: HostCall) -> HostOutcome {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&PluginMessage::HostCall { id, call });
        loop {
            match self.read_raw() {
                Some(HostMessage::HostResult { id: rid, outcome }) if rid == id => return outcome,
                // Any other message the host already sent must not be dropped;
                // buffer it for the main loop to dispatch after this call.
                Some(other) => self.pending.push_back(other),
                None => return HostOutcome::error("host channel closed"),
            }
        }
    }
}

impl<R: BufRead, W: Write> HostApi for Io<R, W> {
    fn clipboard_set(&mut self, text: &str) {
        self.call(HostCall::ClipboardSet {
            text: text.to_string(),
        });
    }
    fn open(&mut self, target: &str) {
        self.call(HostCall::Open {
            target: target.to_string(),
        });
    }
    fn run_command(&mut self, argv: Vec<String>) {
        self.call(HostCall::Run { argv });
    }
    fn toast(&mut self, text: &str) {
        self.call(HostCall::Toast {
            text: text.to_string(),
            kind: None,
        });
    }
    fn storage_get(&mut self, key: &str) -> Option<String> {
        self.call(HostCall::StorageGet {
            key: key.to_string(),
        })
        .value
        .and_then(|value| value.as_str().map(str::to_string))
    }
    fn storage_set(&mut self, key: &str, value: &str) {
        self.call(HostCall::StorageSet {
            key: key.to_string(),
            value: value.to_string(),
        });
    }
    fn storage_delete(&mut self, key: &str) {
        self.call(HostCall::StorageDelete {
            key: key.to_string(),
        });
    }
    fn storage_list(&mut self, prefix: Option<&str>) -> Vec<(String, String)> {
        let outcome = self.call(HostCall::StorageList {
            prefix: prefix.map(str::to_string),
        });
        outcome
            .value
            .and_then(|value| value.as_array().cloned())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|entry| {
                        Some((
                            entry.get("key")?.as_str()?.to_string(),
                            entry.get("value")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Run a plugin against stdin/stdout (the normal entry point).
pub fn run<P: Plugin>(plugin: P) {
    run_with(plugin, BufReader::new(stdin()), stdout());
}

/// Run a plugin against arbitrary streams, returning the writer (for testing).
pub fn run_with<P: Plugin, R: BufRead, W: Write>(mut plugin: P, reader: R, writer: W) -> W {
    let mut io = Io::new(reader, writer);
    while let Some(message) = io.read_message() {
        match message {
            HostMessage::Initialize { .. } => io.send(&PluginMessage::Initialized {
                protocol_version: PROTOCOL_VERSION,
                capabilities: Vec::new(),
            }),
            HostMessage::Query { generation, text } => {
                let items = plugin.query(&mut io, &text);
                io.send(&PluginMessage::Results {
                    generation,
                    items,
                    done: true,
                });
            }
            HostMessage::Activate {
                generation,
                command_id,
                item_id,
            } => {
                if let Some(response) = plugin.activate(&mut io, &command_id, item_id) {
                    emit(&mut io, response, generation);
                }
            }
            HostMessage::Event { generation, event } => {
                if let Some(response) = plugin.event(&mut io, event) {
                    emit(&mut io, response, generation);
                }
            }
            HostMessage::HostResult { .. } => {}
            HostMessage::Shutdown => break,
        }
    }
    io.writer
}

fn emit<R: BufRead, W: Write>(io: &mut Io<R, W>, response: Response, generation: u64) {
    let message = match response {
        Response::Render(view) => PluginMessage::Render {
            generation,
            replace: false,
            view,
        },
        Response::Replace(view) => PluginMessage::Render {
            generation,
            replace: true,
            view,
        },
        Response::Pop => PluginMessage::Pop { generation },
        Response::Close { hide } => PluginMessage::Close {
            generation,
            hide_launcher: hide,
        },
    };
    io.send(&message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nursearch_proto::ListView;
    use std::io::Cursor;

    struct Demo;
    impl Plugin for Demo {
        fn query(&mut self, _host: &mut dyn HostApi, text: &str) -> Vec<ResultItem> {
            vec![ResultItem {
                id: "1".to_string(),
                title: format!("Echo {text}"),
                subtitle: None,
                icon: None,
                score: 100,
                command_id: "open".to_string(),
                actions: Vec::new(),
            }]
        }
        fn activate(
            &mut self,
            _host: &mut dyn HostApi,
            _command_id: &str,
            _item_id: Option<String>,
        ) -> Option<Response> {
            Some(Response::Render(View::List(ListView {
                title: Some("Opened".to_string()),
                ..Default::default()
            })))
        }
        fn event(&mut self, _host: &mut dyn HostApi, _event: ViewEvent) -> Option<Response> {
            None
        }
    }

    #[test]
    fn handshake_query_and_activate() {
        let input = [
            encode(&HostMessage::Initialize {
                protocol_version: PROTOCOL_VERSION,
                host_version: "test".to_string(),
                plugin_id: "demo".to_string(),
                preferences: serde_json::Value::Null,
            })
            .unwrap(),
            encode(&HostMessage::Query {
                generation: 3,
                text: "hi".to_string(),
            })
            .unwrap(),
            encode(&HostMessage::Activate {
                generation: 3,
                command_id: "open".to_string(),
                item_id: Some("1".to_string()),
            })
            .unwrap(),
        ]
        .concat();

        let output = run_with(Demo, Cursor::new(input.into_bytes()), Vec::<u8>::new());
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("\"type\":\"initialized\""));
        assert!(text.contains("\"type\":\"results\""));
        assert!(text.contains("Echo hi"));
        assert!(text.contains("\"type\":\"render\""));
        assert!(text.contains("Opened"));
    }

    /// A plugin that makes a host call inside `query`, to exercise message
    /// buffering while waiting for the host result.
    struct Calling;
    impl Plugin for Calling {
        fn query(&mut self, host: &mut dyn HostApi, text: &str) -> Vec<ResultItem> {
            let _ = host.storage_get("x"); // sends a host call mid-query
            vec![ResultItem {
                id: text.to_string(),
                title: format!("R:{text}"),
                subtitle: None,
                icon: None,
                score: 1,
                command_id: "c".to_string(),
                actions: Vec::new(),
            }]
        }
        fn activate(
            &mut self,
            _host: &mut dyn HostApi,
            _command_id: &str,
            _item_id: Option<String>,
        ) -> Option<Response> {
            None
        }
        fn event(&mut self, _host: &mut dyn HostApi, _event: ViewEvent) -> Option<Response> {
            None
        }
    }

    #[test]
    fn host_call_does_not_drop_queued_messages() {
        // The second query arrives in the pipe before the first query's host
        // result, so the call loop must buffer it rather than discard it.
        let input = [
            encode(&HostMessage::Query {
                generation: 1,
                text: "first".to_string(),
            })
            .unwrap(),
            encode(&HostMessage::Query {
                generation: 2,
                text: "second".to_string(),
            })
            .unwrap(),
            encode(&HostMessage::HostResult {
                id: 0,
                outcome: HostOutcome::ok(None),
            })
            .unwrap(),
            encode(&HostMessage::HostResult {
                id: 1,
                outcome: HostOutcome::ok(None),
            })
            .unwrap(),
        ]
        .concat();

        let output = run_with(Calling, Cursor::new(input.into_bytes()), Vec::<u8>::new());
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("R:first"), "first query lost: {text}");
        assert!(text.contains("R:second"), "second query lost: {text}");
    }
}
