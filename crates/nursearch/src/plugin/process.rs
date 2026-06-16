//! A single running plugin process and its newline-delimited JSON-RPC channel.
//!
//! Reading is fully asynchronous on the GLib main context (`read_line_future`
//! in a `spawn_future_local` loop). Writes are small single-line messages to a
//! pipe and are issued synchronously; a prompt plugin drains them immediately.

use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use nursearch_proto::{HostMessage, PluginMessage, decode, encode};
use std::cell::Cell;
use std::ffi::OsStr;
use std::path::Path;
use std::rc::Rc;

/// Callback invoked for every message a plugin emits, plus an end-of-life
/// signal. The string is the plugin id.
pub type OnMessage = Rc<dyn Fn(&str, PluginMessage)>;
pub type OnExit = Rc<dyn Fn(&str)>;

pub struct PluginProcess {
    id: String,
    subprocess: gio::Subprocess,
    stdin: gio::OutputStream,
    alive: Rc<Cell<bool>>,
}

impl PluginProcess {
    /// Spawn the plugin and start its read loop. `on_message` fires for each
    /// decoded message; `on_exit` fires once when the process ends or errors.
    pub fn spawn(
        id: &str,
        argv: &[String],
        cwd: &Path,
        on_message: OnMessage,
        on_exit: OnExit,
        on_error: OnExit,
    ) -> Result<Self, glib::Error> {
        let os_argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
        let launcher = gio::SubprocessLauncher::new(
            gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE,
        );
        launcher.set_cwd(cwd);
        let subprocess = launcher.spawn(&os_argv)?;

        let stdin = subprocess.stdin_pipe().ok_or_else(|| {
            glib::Error::new(gio::IOErrorEnum::Failed, "plugin has no stdin pipe")
        })?;
        let stdout = subprocess.stdout_pipe().ok_or_else(|| {
            glib::Error::new(gio::IOErrorEnum::Failed, "plugin has no stdout pipe")
        })?;

        let alive = Rc::new(Cell::new(true));
        spawn_read_loop(
            id.to_string(),
            stdout,
            on_message,
            on_exit,
            on_error,
            Rc::clone(&alive),
        );

        Ok(Self {
            id: id.to_string(),
            subprocess,
            stdin,
            alive,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn is_alive(&self) -> bool {
        self.alive.get()
    }

    /// Send a message to the plugin. Returns an error if encoding or the write
    /// fails (e.g. the plugin has gone away).
    pub fn send(&self, message: &HostMessage) -> Result<(), String> {
        if !self.alive.get() {
            return Err("plugin process is not running".to_string());
        }
        let line = encode(message).map_err(|err| format!("encode failed: {err}"))?;
        let bytes = glib::Bytes::from_owned(line.into_bytes());
        self.stdin
            .write_bytes(&bytes, gio::Cancellable::NONE)
            .map_err(|err| format!("write failed: {err}"))?;
        Ok(())
    }

    /// Force-terminate the process.
    pub fn kill(&self) {
        self.alive.set(false);
        self.subprocess.force_exit();
    }
}

fn spawn_read_loop(
    id: String,
    stdout: gio::InputStream,
    on_message: OnMessage,
    on_exit: OnExit,
    on_error: OnExit,
    alive: Rc<Cell<bool>>,
) {
    let reader = gio::DataInputStream::new(&stdout);
    glib::spawn_future_local(async move {
        loop {
            match reader.read_line_future(glib::Priority::DEFAULT).await {
                Ok(Some(bytes)) => {
                    if bytes.is_empty() {
                        continue; // blank line
                    }
                    let line = String::from_utf8_lossy(&bytes);
                    match decode::<PluginMessage>(&line) {
                        Ok(message) => on_message(&id, message),
                        Err(err) => {
                            // Malformed output is a protocol violation: stop
                            // reading and let the host disable the plugin.
                            log::warn!("plugin '{id}' sent invalid message: {err}; line={line:?}");
                            alive.set(false);
                            on_error(&id);
                            return;
                        }
                    }
                }
                Ok(None) => break, // EOF
                Err(err) => {
                    log::warn!("plugin '{id}' read error: {err}");
                    break;
                }
            }
        }
        alive.set(false);
        on_exit(&id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nursearch_proto::{HostMessage, PROTOCOL_VERSION};
    use std::cell::RefCell;

    /// A minimal protocol-speaking plugin: replies to initialize and echoes
    /// queries back as a single result item.
    const PLUGIN_PY: &str = r#"
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    m = json.loads(line)
    t = m.get("type")
    if t == "initialize":
        send({"type": "initialized", "protocolVersion": 1, "capabilities": []})
    elif t == "query":
        send({"type": "results", "generation": m["generation"], "done": True,
              "items": [{"id": "1", "title": "Echo: " + m["text"],
                         "commandId": "echo", "score": 9000}]})
    elif t == "shutdown":
        break
"#;

    #[test]
    fn talks_to_a_real_plugin_process() {
        let dir = std::env::temp_dir().join(format!("nursearch-proc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.py"), PLUGIN_PY).unwrap();

        let messages = Rc::new(RefCell::new(Vec::new()));
        let ctx = glib::MainContext::new();
        let messages_for_run = Rc::clone(&messages);
        ctx.with_thread_default(|| {
            let main_loop = glib::MainLoop::new(Some(&ctx), false);

            let on_message = {
                let messages = Rc::clone(&messages_for_run);
                let main_loop = main_loop.clone();
                Rc::new(move |_id: &str, message: PluginMessage| {
                    let is_results = matches!(message, PluginMessage::Results { .. });
                    messages.borrow_mut().push(message);
                    if is_results {
                        main_loop.quit();
                    }
                })
            };
            let on_exit = Rc::new(|_id: &str| {});
            let on_error = Rc::new(|_id: &str| {});

            let argv = vec!["python3".to_string(), "main.py".to_string()];
            let process =
                PluginProcess::spawn("test", &argv, &dir, on_message, on_exit, on_error)
                    .expect("spawn plugin");
            process
                .send(&HostMessage::Initialize {
                    protocol_version: PROTOCOL_VERSION,
                    host_version: "test".to_string(),
                    plugin_id: "test".to_string(),
                    preferences: serde_json::Value::Null,
                })
                .unwrap();
            process
                .send(&HostMessage::Query {
                    generation: 1,
                    text: "hi".to_string(),
                })
                .unwrap();

            // Safety net so a misbehaving plugin can't hang the test.
            let bail = main_loop.clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || bail.quit());
            main_loop.run();
        })
        .unwrap();

        let msgs = messages.borrow();
        assert!(
            msgs.iter()
                .any(|m| matches!(m, PluginMessage::Initialized { .. })),
            "expected an Initialized message, got {msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| matches!(
                m,
                PluginMessage::Results { items, .. }
                    if items.iter().any(|item| item.title.contains("hi"))
            )),
            "expected an echoed Results message, got {msgs:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drives_a_session_with_the_demo_plugin_binary() {
        // Built by `cargo test --workspace`; skip if only this crate was built.
        let binary = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/nursearch-demo");
        if !binary.exists() {
            eprintln!(
                "skipping: demo plugin binary not built ({})",
                binary.display()
            );
            return;
        }

        let messages = Rc::new(RefCell::new(Vec::new()));
        let ctx = glib::MainContext::new();
        let messages_for_run = Rc::clone(&messages);
        ctx.with_thread_default(|| {
            let main_loop = glib::MainLoop::new(Some(&ctx), false);
            let on_message = {
                let messages = Rc::clone(&messages_for_run);
                let main_loop = main_loop.clone();
                Rc::new(move |_id: &str, message: PluginMessage| {
                    let is_render = matches!(message, PluginMessage::Render { .. });
                    messages.borrow_mut().push(message);
                    if is_render {
                        main_loop.quit();
                    }
                })
            };
            let on_exit = Rc::new(|_id: &str| {});
            let on_error = Rc::new(|_id: &str| {});

            let argv = vec![binary.to_string_lossy().into_owned()];
            let process =
                PluginProcess::spawn("demo", &argv, &std::env::temp_dir(), on_message, on_exit, on_error)
                    .expect("spawn demo plugin");
            for message in [
                HostMessage::Initialize {
                    protocol_version: PROTOCOL_VERSION,
                    host_version: "test".to_string(),
                    plugin_id: "demo".to_string(),
                    preferences: serde_json::Value::Null,
                },
                HostMessage::Query {
                    generation: 1,
                    text: "x".to_string(),
                },
                HostMessage::Activate {
                    generation: 1,
                    command_id: "open".to_string(),
                    item_id: None,
                },
            ] {
                process.send(&message).unwrap();
            }

            let bail = main_loop.clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || bail.quit());
            main_loop.run();
        })
        .unwrap();

        let msgs = messages.borrow();
        assert!(
            msgs.iter()
                .any(|m| matches!(m, PluginMessage::Initialized { .. })),
            "no Initialized: {msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|m| matches!(m, PluginMessage::Results { .. })),
            "no Results: {msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| matches!(
                m,
                PluginMessage::Render { view: nursearch_proto::View::List(list), .. }
                    if list.title.as_deref() == Some("Demo items")
            )),
            "no List render: {msgs:?}"
        );
    }
}
