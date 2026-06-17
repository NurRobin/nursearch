mod calc;
mod config;
mod db;
mod desktop;
mod i18n;
mod launch;
mod plugin;
mod rank;
mod search;
mod system;
mod view;

use db::HistoryDb;
use desktop::{DesktopEntry, discover_apps};
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use log::{debug, error, info, warn};
use plugin::PluginHost;
use search::{Action, SearchResult, core_results, finalize, result_from_plugin};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

const APP_ID: &str = "dev.nursearch.NurSearch";

struct AppState {
    apps: Vec<DesktopEntry>,
    /// The merged, ranked list currently shown on the root screen.
    results: Vec<SearchResult>,
    db: HistoryDb,
    /// Plugin host; set after the UI sink exists.
    host: Option<PluginHost>,
    /// Monotonic query counter used to discard stale async plugin results.
    generation: u64,
    /// Latest core results for the current generation (instant, in-process).
    core: Vec<SearchResult>,
    /// Latest plugin contributions for the current generation, keyed by plugin id.
    plugin_results: HashMap<String, Vec<SearchResult>>,
    /// Plugin ids actually asked to contribute to the current root generation.
    /// Results from any other plugin are rejected.
    expected_contributors: std::collections::HashSet<String>,
    /// The active plugin view session, if the user has drilled into a plugin.
    session: Option<Session>,
    /// The plugin the user activated for the current/pending session. Session
    /// messages (render/pop/close) from any other plugin are rejected.
    session_owner: Option<String>,
    /// Monotonic counter for session input, separate from the root `generation`
    /// so a root refresh never makes an in-flight session render look stale.
    session_generation: u64,
    /// The list box that keyboard navigation drives (root list, or a session
    /// List view's box).
    active_list: Option<gtk::ListBox>,
    /// Fields of the active Form view, for reading values on submit.
    active_form: Vec<view::FormField>,
    /// Set while programmatically changing the entry text, to suppress the
    /// resulting `changed` signal.
    suppress_input: Cell<bool>,
    /// Kept alive so the CSS hot-reload monitor keeps firing.
    _css_monitor: Option<gio::FileMonitor>,
    /// Kept alive so the .desktop directory watchers keep firing.
    _dir_monitors: Vec<gio::FileMonitor>,
}

/// An active plugin view session. The host keeps the view stack so it can
/// navigate back robustly even if the plugin is slow.
struct Session {
    plugin_id: String,
    stack: Vec<nursearch_proto::View>,
}

/// Handles needed to re-show an already-built launcher window. Cloning is cheap
/// (GTK objects are reference counted).
#[derive(Clone)]
struct Launcher {
    window: gtk::ApplicationWindow,
    entry: gtk::Entry,
}

impl Launcher {
    /// Reset and bring the launcher back to the foreground (daemon re-activation).
    fn show(&self) {
        self.entry.set_text("");
        self.window.set_visible(true);
        self.window.present();
        self.entry.grab_focus();
    }
}

/// The launcher's side of the plugin channel. Holds the shared state and the
/// widgets that plugin messages update. Cloning is cheap (Rc + GTK objects).
#[derive(Clone)]
struct Ui {
    state: Rc<RefCell<AppState>>,
    window: gtk::ApplicationWindow,
    entry: gtk::Entry,
    list: gtk::ListBox,
    empty: gtk::Label,
    status: gtk::Label,
    /// Container holding the root search results (shown when no session).
    content_root: gtk::Box,
    /// Container holding the active plugin view (shown during a session).
    content_session: gtk::Box,
}

impl plugin::HostSink for Ui {
    fn results(
        &self,
        plugin_id: &str,
        generation: u64,
        items: Vec<nursearch_proto::ResultItem>,
        _done: bool,
    ) {
        {
            let mut st = self.state.borrow_mut();
            if generation != st.generation {
                return; // stale results for an older query
            }
            if !st.expected_contributors.contains(plugin_id) {
                // This plugin was not asked to contribute to this query (e.g. a
                // keyword takeover, or an unsolicited injection). Ignore it.
                return;
            }
            let name = st
                .host
                .as_ref()
                .and_then(|host| host.manifest(plugin_id))
                .map(|manifest| manifest.name)
                .unwrap_or_else(|| plugin_id.to_string());
            let converted = items
                .into_iter()
                .map(|item| result_from_plugin(plugin_id, &name, item))
                .collect();
            st.plugin_results.insert(plugin_id.to_string(), converted);
        }
        render_root(&self.state, &self.list, &self.empty);
    }

    fn render(&self, plugin_id: &str, generation: u64, replace: bool, view: nursearch_proto::View) {
        session_render(self, plugin_id, generation, replace, view);
    }

    fn pop(&self, plugin_id: &str, generation: u64) {
        session_pop(self, plugin_id, generation);
    }

    fn close(&self, plugin_id: &str, generation: u64, hide_launcher: bool) {
        session_close(self, plugin_id, generation, hide_launcher);
    }

    fn host_call(
        &self,
        plugin_id: &str,
        call: nursearch_proto::HostCall,
    ) -> nursearch_proto::HostOutcome {
        host_capability(self, plugin_id, call)
    }
}

fn main() -> glib::ExitCode {
    init_logging();
    info!("starting NurSearch");

    let app = gtk::Application::builder().application_id(APP_ID).build();

    // The process stays resident as a daemon: the first invocation builds the
    // window, and every later `nursearch` call re-activates the running instance
    // and simply re-presents it for an instant open.
    let launcher: Rc<RefCell<Option<Launcher>>> = Rc::new(RefCell::new(None));
    app.connect_activate(move |app| {
        let mut slot = launcher.borrow_mut();
        match slot.as_ref() {
            Some(existing) => existing.show(),
            None => *slot = build_ui(app),
        }
    });
    app.run()
}

fn build_ui(app: &gtk::Application) -> Option<Launcher> {
    let apps = discover_apps();
    info!("discovered {} desktop applications", apps.len());

    let (db, startup_error) = match HistoryDb::open() {
        Ok(db) => {
            debug!("opened persistent history database");
            (db, None)
        }
        Err(err) => match HistoryDb::open_in_memory() {
            Ok(db) => {
                warn!("history database is unavailable; using temporary history: {err}");
                (db, Some(i18n::warn_history_memory(&err.to_string())))
            }
            Err(memory_err) => {
                error!("failed to open history database: {err}");
                error!("failed to open temporary history database: {memory_err}");
                return None;
            }
        },
    };

    let state = Rc::new(RefCell::new(AppState {
        apps,
        results: Vec::new(),
        db,
        host: None,
        generation: 0,
        core: Vec::new(),
        plugin_results: HashMap::new(),
        expected_contributors: std::collections::HashSet::new(),
        session: None,
        session_owner: None,
        session_generation: 0,
        active_list: None,
        active_form: Vec::new(),
        suppress_input: Cell::new(false),
        _css_monitor: config::install_css(),
        _dir_monitors: Vec::new(),
    }));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("NurSearch")
        .default_width(720)
        .default_height(520)
        .decorated(false)
        .resizable(false)
        .build();

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    root.add_css_class("launcher-shell");

    let entry = gtk::Entry::builder()
        .placeholder_text(i18n::search_placeholder())
        .hexpand(true)
        .build();
    entry.set_primary_icon_name(Some("system-search-symbolic"));
    entry.add_css_class("search-entry");

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .activate_on_single_click(false)
        .build();
    list.add_css_class("results-list");

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .child(&list)
        .build();
    scroller.add_css_class("results-scroller");

    let empty = gtk::Label::builder()
        .label(i18n::no_results())
        .xalign(0.0)
        .visible(false)
        .build();
    empty.add_css_class("empty-state");

    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    status.add_css_class("error-status");

    let content_root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .build();
    content_root.append(&scroller);
    content_root.append(&empty);

    // Where a plugin's pushed views are rendered; hidden until a session starts.
    let content_session = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .visible(false)
        .build();

    root.append(&entry);
    root.append(&status);
    root.append(&content_root);
    root.append(&content_session);
    root.append(&hint_bar());
    window.set_child(Some(&root));

    if let Some(error) = startup_error {
        show_error(&status, &error);
    }

    // Build the plugin host with the UI as its message sink, then make it
    // available to the rest of the app.
    let ui = Ui {
        state: Rc::clone(&state),
        window: window.clone(),
        entry: entry.clone(),
        list: list.clone(),
        empty: empty.clone(),
        status: status.clone(),
        content_root: content_root.clone(),
        content_session: content_session.clone(),
    };
    let host = PluginHost::new(Rc::new(ui.clone()) as Rc<dyn plugin::HostSink>);
    {
        let mut st = state.borrow_mut();
        st.host = Some(host);
        st.active_list = Some(list.clone());
    }

    dispatch_query(&state, &entry, &list, &empty);

    {
        let ui = ui.clone();
        entry.connect_changed(move |entry| {
            if ui.state.borrow().suppress_input.get() {
                return;
            }
            ui.status.set_visible(false);
            if ui.state.borrow().session.is_some() {
                send_event(
                    &ui,
                    nursearch_proto::ViewEvent::Input {
                        text: entry.text().to_string(),
                    },
                );
            } else {
                dispatch_query(&ui.state, entry, &ui.list, &ui.empty);
            }
        });
    }

    {
        // Mouse activation of a root row.
        let ui = ui.clone();
        list.connect_row_activated(move |_, row| {
            launch_index(
                &ui.state,
                ui.entry.text().as_ref(),
                row.index() as usize,
                &ui.window,
                &ui.status,
            );
        });
    }

    let entry_key_controller = gtk::EventControllerKey::new();
    // Capture phase: the Entry's internal GtkText consumes Return during the
    // target/bubble pass, so a default (bubble) controller never sees it while
    // the text field has focus. Capturing runs ancestor-first, before GtkText.
    entry_key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let ui = ui.clone();
        entry_key_controller.connect_key_pressed(move |_, key, _, modifiers| match key {
            gdk::Key::Return | gdk::Key::KP_Enter => {
                activate_primary(&ui, modifiers);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
    }
    entry.add_controller(entry_key_controller);

    let key_controller = gtk::EventControllerKey::new();
    {
        let ui = ui.clone();
        key_controller.connect_key_pressed(move |_, key, _, modifiers| {
            let in_session = ui.state.borrow().session.is_some();
            match key {
                gdk::Key::Escape => {
                    if in_session {
                        session_back(&ui);
                    } else {
                        ui.window.set_visible(false);
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Down => {
                    navigate(&ui, 1);
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    navigate(&ui, -1);
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    activate_primary(&ui, modifiers);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
    }
    window.add_controller(key_controller);

    state.borrow_mut()._dir_monitors = watch_app_dirs(&state, &entry, &list, &empty);

    window.present();
    entry.grab_focus();
    debug!("launcher window presented");

    debug_autodrive(&ui);

    Some(Launcher { window, entry })
}

/// Test/verification hook: with `NURSEARCH_DEBUG_QUERY` set, pre-fill the search
/// box (so plugin results render without keystroke injection); with
/// `NURSEARCH_DEBUG_ACTIVATE` also set, activate the first result shortly after
/// so a plugin view session can be observed. No effect unless those vars are set.
fn debug_autodrive(ui: &Ui) {
    let Ok(query) = std::env::var("NURSEARCH_DEBUG_QUERY") else {
        return;
    };
    ui.entry.set_text(&query);
    ui.entry.set_position(-1);
    if std::env::var("NURSEARCH_DEBUG_ACTIVATE").is_ok() {
        let ui = ui.clone();
        glib::timeout_add_local_once(Duration::from_millis(800), move || activate_current(&ui));
    }
}

/// Watch every `.desktop` source directory and rebuild the in-memory app list
/// when files appear, change, or disappear. Reloads are coalesced so a burst of
/// filesystem events triggers a single rescan.
fn watch_app_dirs(
    state: &Rc<RefCell<AppState>>,
    entry: &gtk::Entry,
    list: &gtk::ListBox,
    empty: &gtk::Label,
) -> Vec<gio::FileMonitor> {
    let pending = Rc::new(Cell::new(false));

    desktop::application_dirs()
        .into_iter()
        .filter_map(|dir| {
            let file = gio::File::for_path(&dir);
            let monitor = file
                .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
                .ok()?;

            let pending = Rc::clone(&pending);
            let state = Rc::clone(state);
            let entry = entry.clone();
            let list = list.clone();
            let empty = empty.clone();
            monitor.connect_changed(move |_, _, _, _| {
                if pending.replace(true) {
                    return;
                }
                let pending = Rc::clone(&pending);
                let state = Rc::clone(&state);
                let entry = entry.clone();
                let list = list.clone();
                let empty = empty.clone();
                glib::timeout_add_local_once(Duration::from_millis(300), move || {
                    pending.set(false);
                    debug!("reloading apps after .desktop change");
                    let in_session = {
                        let mut st = state.borrow_mut();
                        st.apps = discover_apps();
                        st.session.is_some()
                    };
                    // Don't disturb an active plugin session; the refreshed apps
                    // are picked up the next time the root screen is shown.
                    if !in_session {
                        dispatch_query(&state, &entry, &list, &empty);
                    }
                });
            });
            Some(monitor)
        })
        .collect()
}

fn init_logging() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("nursearch=info"))
        .format_timestamp_secs()
        .init();
}

/// Run a new root query. With an active keyword (`f …`) the matching plugin
/// takes over the root list; otherwise the built-in core renders instantly and
/// global plugins are asked for async contributions.
fn dispatch_query(
    state: &Rc<RefCell<AppState>>,
    entry: &gtk::Entry,
    list: &gtk::ListBox,
    empty: &gtk::Label,
) {
    let query = entry.text().to_string();
    let host = state.borrow().host.clone();
    let keyword = host.as_ref().and_then(|host| host.keyword_match(&query));
    let normalized = db::normalize_query(&query);

    // Decide exactly which plugins are asked to contribute, with the text each
    // gets. A keyword takes over (only that plugin); otherwise every global
    // contributor sees the full query. Empty query asks no one.
    let contributors: Vec<(String, String)> = if normalized.is_empty() {
        Vec::new()
    } else if let Some((id, rest)) = &keyword {
        vec![(id.clone(), rest.clone())]
    } else if let Some(host) = &host {
        host.contributors_for(&query)
            .into_iter()
            .map(|id| (id, normalized.clone()))
            .collect()
    } else {
        Vec::new()
    };

    let generation = {
        let mut st = state.borrow_mut();
        st.generation += 1;
        // A keyword takes over the root, so the core contributes nothing.
        st.core = if keyword.is_some() {
            Vec::new()
        } else {
            let snapshot = st.db.snapshot(&query);
            core_results(&st.apps, &query, &snapshot)
        };
        st.plugin_results.clear();
        // Results are only accepted from plugins in this set for this generation.
        st.expected_contributors = contributors.iter().map(|(id, _)| id.clone()).collect();
        st.generation
    };

    render_root(state, list, empty);

    if let Some(host) = host {
        for (id, text) in contributors {
            host.send(
                &id,
                &nursearch_proto::HostMessage::Query { generation, text },
            );
        }
    }
}

/// Merge core + plugin results, rank them, and rebuild the result list.
fn render_root(state: &Rc<RefCell<AppState>>, list: &gtk::ListBox, empty: &gtk::Label) {
    let merged = {
        let st = state.borrow();
        let mut all = st.core.clone();
        for contributions in st.plugin_results.values() {
            all.extend(contributions.iter().cloned());
        }
        finalize(all)
    };
    state.borrow_mut().results = merged;
    rebuild_list(state, list, empty);
}

/// Replace the list rows from the current `state.results`.
fn rebuild_list(state: &Rc<RefCell<AppState>>, list: &gtk::ListBox, empty: &gtk::Label) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let st = state.borrow();
    for result in &st.results {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(true);
        row.set_selectable(true);
        row.add_css_class("result-list-row");
        row.set_child(Some(&result_row(result)));
        list.append(&row);
    }

    let has_results = !st.results.is_empty();
    empty.set_visible(!has_results);
    list.set_visible(has_results);

    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }
}

fn result_row(result: &SearchResult) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .valign(gtk::Align::Center)
        .build();
    row.add_css_class("result-row");

    let image = icon_image(result.icon.as_deref());
    row.append(&image);

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(1)
        .hexpand(true)
        .build();

    let name = gtk::Label::builder()
        .label(&result.title)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    name.add_css_class("result-name");
    text.append(&name);

    if let Some(detail_text) = result.subtitle.as_deref() {
        let detail = gtk::Label::builder()
            .label(detail_text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        detail.add_css_class("result-detail");
        text.append(&detail);
    }

    row.append(&text);

    if let Some(badge_text) = result.kind.badge() {
        let badge = gtk::Label::builder()
            .label(badge_text)
            .valign(gtk::Align::Center)
            .build();
        badge.add_css_class("result-badge");
        row.append(&badge);
    }

    row
}

fn icon_image(icon: Option<&str>) -> gtk::Image {
    let image = match icon.filter(|icon| !icon.is_empty()) {
        Some(icon) if Path::new(icon).is_absolute() => gtk::Image::from_file(icon),
        Some(icon) => gtk::Image::from_icon_name(icon),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(32);
    image
}

fn show_error(label: &gtk::Label, message: &str) {
    label.set_text(message);
    label.set_visible(true);
}

fn launch_index(
    state: &Rc<RefCell<AppState>>,
    query: &str,
    index: usize,
    window: &gtk::ApplicationWindow,
    status: &gtk::Label,
) {
    let result = {
        let state = state.borrow();
        state.results.get(index).cloned()
    };

    let Some(result) = result else {
        return;
    };

    // Plugin items open a view session instead of running a one-shot action.
    if let Action::OpenPlugin {
        plugin_id,
        command_id,
        item_id,
    } = &result.action
    {
        let (host, generation) = {
            let mut st = state.borrow_mut();
            // Record who owns the session so renders from other plugins are ignored.
            st.session_owner = Some(plugin_id.clone());
            st.session_generation += 1;
            (st.host.clone(), st.session_generation)
        };
        if let Some(host) = host {
            info!("activating plugin command: {plugin_id}:{command_id}");
            host.send(
                plugin_id,
                &nursearch_proto::HostMessage::Activate {
                    generation,
                    command_id: command_id.clone(),
                    item_id: Some(item_id.clone()),
                },
            );
        }
        record_history(state, query, &result, status);
        return; // keep the window open; the plugin will render its view
    }

    info!("activating result: {}", result.title);
    match perform_action(&result, window) {
        Ok(()) => {
            debug!("action succeeded: {}", result.title);
            record_history(state, query, &result, status);
            window.set_visible(false);
        }
        Err(err) => {
            error!("could not activate {}: {err}", result.title);
            show_error(status, &i18n::error_run(&result.title, &err.to_string()));
        }
    }
}

/// Carry out a result's action. Returns once the action has been accepted; it
/// does not wait for spawned processes to finish.
fn perform_action(result: &SearchResult, window: &gtk::ApplicationWindow) -> std::io::Result<()> {
    match &result.action {
        Action::Launch(app) => launch::launch(app),
        Action::Run(command) => launch::run_command(command),
        Action::Copy(text) => {
            window.clipboard().set_text(text);
            Ok(())
        }
        // Handled earlier in launch_index; never reached here.
        Action::OpenPlugin { .. } => Ok(()),
    }
}

/// Record a successful activation in the usage history, if the result opts in.
fn record_history(
    state: &Rc<RefCell<AppState>>,
    query: &str,
    result: &SearchResult,
    status: &gtk::Label,
) {
    let Some(key) = result.history_key.as_deref() else {
        return;
    };
    let state = state.borrow();
    if let Err(err) = state.db.record_launch(query, key) {
        warn!(
            "could not update launch history for {}: {err}",
            result.title
        );
        show_error(status, &i18n::error_history(&err.to_string()));
    }
}

// --- Plugin view session ---

use nursearch_proto::{View, ViewEvent};

/// A plugin pushed (or replaced) a view: start/continue the session and render.
fn session_render(ui: &Ui, plugin_id: &str, generation: u64, replace: bool, view: View) {
    if !owns_session(ui, plugin_id) {
        warn!("ignoring render from '{plugin_id}', which does not own the session");
        return;
    }
    if is_stale_session_message(ui, generation) {
        debug!("dropping stale render from '{plugin_id}' (gen {generation})");
        return;
    }
    {
        let mut st = ui.state.borrow_mut();
        let starting = st
            .session
            .as_ref()
            .map(|session| session.plugin_id != plugin_id)
            .unwrap_or(true);
        if starting {
            st.session = Some(Session {
                plugin_id: plugin_id.to_string(),
                stack: Vec::new(),
            });
        }
        let session = st.session.as_mut().expect("session just set");
        if replace && !session.stack.is_empty() {
            *session.stack.last_mut().unwrap() = view;
        } else {
            session.stack.push(view);
        }
    }
    enter_session_mode(ui);
    render_current_view(ui);
}

/// A plugin popped its own view.
fn session_pop(ui: &Ui, plugin_id: &str, generation: u64) {
    if !owns_session(ui, plugin_id) || is_stale_session_message(ui, generation) {
        return;
    }
    let emptied = {
        let mut st = ui.state.borrow_mut();
        match st.session.as_mut() {
            Some(session) => {
                session.stack.pop();
                session.stack.is_empty()
            }
            None => true,
        }
    };
    if emptied {
        exit_session(ui);
    } else {
        render_current_view(ui);
    }
}

/// A plugin ended its session.
fn session_close(ui: &Ui, plugin_id: &str, generation: u64, hide_launcher: bool) {
    if !owns_session(ui, plugin_id) || is_stale_session_message(ui, generation) {
        return;
    }
    exit_session(ui);
    if hide_launcher {
        ui.window.set_visible(false);
    }
}

/// Whether `plugin_id` owns the current (or pending) session.
fn owns_session(ui: &Ui, plugin_id: &str) -> bool {
    ui.state.borrow().session_owner.as_deref() == Some(plugin_id)
}

/// Whether a stamped session message belongs to an older input than the latest.
/// 0 means the plugin did not stamp a generation, which is always accepted.
fn is_stale_session_message(ui: &Ui, generation: u64) -> bool {
    generation != 0 && generation < ui.state.borrow().session_generation
}

/// Execute a host-capability call from a plugin, enforcing declared
/// capabilities. Returns the outcome the host sends back to the plugin.
fn host_capability(
    ui: &Ui,
    plugin_id: &str,
    call: nursearch_proto::HostCall,
) -> nursearch_proto::HostOutcome {
    use nursearch_proto::{HostCall, HostOutcome};

    let require = |cap: &str| -> Result<(), HostOutcome> {
        if capability_allowed(ui, plugin_id, cap) {
            Ok(())
        } else {
            Err(HostOutcome::error(format!(
                "plugin '{plugin_id}' did not declare the '{cap}' capability"
            )))
        }
    };

    match call {
        HostCall::ClipboardSet { text } => match require("clipboard") {
            Ok(()) => {
                ui.window.clipboard().set_text(&text);
                HostOutcome::ok(None)
            }
            Err(outcome) => outcome,
        },
        HostCall::Open { target } => match require("open") {
            Ok(()) => match launch::run_command(&["xdg-open".to_string(), target]) {
                Ok(()) => HostOutcome::ok(None),
                Err(err) => HostOutcome::error(err.to_string()),
            },
            Err(outcome) => outcome,
        },
        HostCall::Run { argv } => match require("run") {
            Ok(()) => match launch::run_command(&argv) {
                Ok(()) => HostOutcome::ok(None),
                Err(err) => HostOutcome::error(err.to_string()),
            },
            Err(outcome) => outcome,
        },
        HostCall::Toast { text, .. } => match require("toast") {
            Ok(()) => {
                show_error(&ui.status, &text);
                HostOutcome::ok(None)
            }
            Err(outcome) => outcome,
        },
        HostCall::StorageGet { key } => match require("storage") {
            Ok(()) => storage_outcome(ui.state.borrow().db.storage_get(plugin_id, &key), |value| {
                value
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null)
            }),
            Err(outcome) => outcome,
        },
        HostCall::StorageSet { key, value } => match require("storage") {
            Ok(()) => storage_outcome(
                ui.state.borrow().db.storage_set(plugin_id, &key, &value),
                |()| serde_json::Value::Null,
            ),
            Err(outcome) => outcome,
        },
        HostCall::StorageDelete { key } => match require("storage") {
            Ok(()) => storage_outcome(ui.state.borrow().db.storage_delete(plugin_id, &key), |()| {
                serde_json::Value::Null
            }),
            Err(outcome) => outcome,
        },
        HostCall::StorageList { prefix } => match require("storage") {
            Ok(()) => storage_outcome(
                ui.state
                    .borrow()
                    .db
                    .storage_list(plugin_id, prefix.as_deref()),
                |pairs| {
                    serde_json::Value::Array(
                        pairs
                            .into_iter()
                            .map(|(key, value)| serde_json::json!({ "key": key, "value": value }))
                            .collect(),
                    )
                },
            ),
            Err(outcome) => outcome,
        },
        HostCall::CloseLauncher => {
            // Only the plugin that owns the active session may hide the launcher,
            // so a background/global plugin cannot grief the user.
            if owns_session(ui, plugin_id) {
                ui.window.set_visible(false);
                HostOutcome::ok(None)
            } else {
                HostOutcome::error("closeLauncher is only allowed for the active session")
            }
        }
    }
}

/// Map a database `Result` into a `HostOutcome`, converting the success value.
fn storage_outcome<T>(
    result: rusqlite::Result<T>,
    to_value: impl FnOnce(T) -> serde_json::Value,
) -> nursearch_proto::HostOutcome {
    match result {
        Ok(value) => nursearch_proto::HostOutcome::ok(Some(to_value(value))),
        Err(err) => nursearch_proto::HostOutcome::error(err.to_string()),
    }
}

/// Whether a plugin declared a capability in its manifest.
fn capability_allowed(ui: &Ui, plugin_id: &str, capability: &str) -> bool {
    ui.state
        .borrow()
        .host
        .as_ref()
        .and_then(|host| host.manifest(plugin_id))
        .map(|manifest| {
            manifest
                .capabilities
                .iter()
                .any(|declared| declared == capability)
        })
        .unwrap_or(false)
}

/// Send a view event to the plugin owning the active session. Each event bumps
/// the generation so only the newest input's render is accepted.
fn send_event(ui: &Ui, event: ViewEvent) {
    let (host, plugin_id, generation) = {
        let mut st = ui.state.borrow_mut();
        let Some(plugin_id) = st.session.as_ref().map(|session| session.plugin_id.clone()) else {
            return;
        };
        st.session_generation += 1;
        (st.host.clone(), plugin_id, st.session_generation)
    };
    if let Some(host) = host {
        host.send(
            &plugin_id,
            &nursearch_proto::HostMessage::Event { generation, event },
        );
    }
}

fn enter_session_mode(ui: &Ui) {
    ui.content_root.set_visible(false);
    ui.content_session.set_visible(true);
    set_entry_text_silently(ui, "");
}

/// Tear down the session and return to the root search screen.
fn exit_session(ui: &Ui) {
    {
        let mut st = ui.state.borrow_mut();
        st.session = None;
        st.session_owner = None;
        st.active_form.clear();
        st.active_list = Some(ui.list.clone());
    }
    while let Some(child) = ui.content_session.first_child() {
        ui.content_session.remove(&child);
    }
    ui.content_session.set_visible(false);
    ui.content_root.set_visible(true);
    set_entry_text_silently(ui, "");
    ui.entry
        .set_placeholder_text(Some(i18n::search_placeholder()));
    ui.entry.grab_focus();
    dispatch_query(&ui.state, &ui.entry, &ui.list, &ui.empty);
}

/// User pressed Esc within a session. Back navigation is host-owned: the host
/// pops its own view stack and does not send the plugin a Pop event, so a
/// plugin that answers Pop with a Pop message cannot trigger a second pop.
/// Bumping the generation also invalidates any in-flight render for the popped
/// view.
fn session_back(ui: &Ui) {
    let emptied = {
        let mut st = ui.state.borrow_mut();
        st.session_generation += 1;
        match st.session.as_mut() {
            Some(session) => {
                session.stack.pop();
                session.stack.is_empty()
            }
            None => true,
        }
    };
    if emptied {
        exit_session(ui);
    } else {
        render_current_view(ui);
    }
}

/// Rebuild the session content area from the top of the view stack.
fn render_current_view(ui: &Ui) {
    while let Some(child) = ui.content_session.first_child() {
        ui.content_session.remove(&child);
    }
    let view = {
        let st = ui.state.borrow();
        st.session
            .as_ref()
            .and_then(|session| session.stack.last().cloned())
    };
    let Some(view) = view else {
        return;
    };

    let placeholder;
    let (widget, active_list, active_form) = match &view {
        View::List(list) => {
            placeholder = list.placeholder.clone();
            let (widget, list_box) = view::build_list(list);
            (widget, Some(list_box), Vec::new())
        }
        View::Detail(detail) => {
            placeholder = detail.placeholder.clone();
            (view::build_detail(detail), None, Vec::new())
        }
        View::Form(form) => {
            placeholder = form.placeholder.clone();
            let (widget, fields) = view::build_form(form);
            (widget, None, fields)
        }
    };

    ui.content_session.append(&widget);
    {
        let mut st = ui.state.borrow_mut();
        st.active_list = active_list;
        st.active_form = active_form;
    }
    ui.entry
        .set_placeholder_text(Some(placeholder.as_deref().unwrap_or("")));
    ui.entry.grab_focus();
}

/// Move selection in the active list (root or session) and scroll to it.
fn navigate(ui: &Ui, direction: i32) {
    let list = ui.state.borrow().active_list.clone();
    let Some(list) = list else {
        return;
    };
    let current = list.selected_row().map(|row| row.index()).unwrap_or(0);
    let next = (current + direction).max(0);
    if let Some(row) = list.row_at_index(next) {
        list.select_row(Some(&row));
        row.grab_focus();
    }
}

/// Enter / primary action on the current screen.
fn activate_current(ui: &Ui) {
    if ui.state.borrow().session.is_some() {
        activate_session(ui);
    } else {
        let index = ui
            .list
            .selected_row()
            .map(|row| row.index() as usize)
            .unwrap_or(0);
        launch_index(
            &ui.state,
            ui.entry.text().as_ref(),
            index,
            &ui.window,
            &ui.status,
        );
    }
}

fn activate_primary(ui: &Ui, modifiers: gdk::ModifierType) {
    if modifiers.contains(gdk::ModifierType::ALT_MASK) {
        open_action_menu(ui);
    } else {
        activate_current(ui);
    }
}

/// Run the primary action of the active session view.
fn activate_session(ui: &Ui) {
    let view = {
        let st = ui.state.borrow();
        st.session
            .as_ref()
            .and_then(|session| session.stack.last().cloned())
    };
    let Some(view) = view else {
        return;
    };
    match view {
        View::List(list) => {
            let index = ui
                .state
                .borrow()
                .active_list
                .as_ref()
                .and_then(|list_box| list_box.selected_row())
                .map(|row| row.index() as usize);
            let Some(item) = index.and_then(|index| list.items.get(index)) else {
                return;
            };
            match item.actions.first() {
                Some(action) => run_action(ui, action.clone(), Some(item.id.clone())),
                None => send_event(
                    ui,
                    ViewEvent::Action {
                        action_id: "default".to_string(),
                        item_id: Some(item.id.clone()),
                    },
                ),
            }
        }
        View::Detail(detail) => {
            if let Some(action) = detail.actions.first() {
                run_action(ui, action.clone(), None);
            }
        }
        View::Form(_) => {
            let values = {
                let st = ui.state.borrow();
                st.active_form
                    .iter()
                    .map(|field| (field.id.clone(), field.value()))
                    .collect()
            };
            send_event(ui, ViewEvent::Submit { values });
        }
    }
}

/// Carry out an action: host-handled kinds finish the interaction; the plugin
/// kind re-enters the plugin.
fn run_action(ui: &Ui, action: nursearch_proto::Action, item_id: Option<String>) {
    use nursearch_proto::ActionKind;

    // Host-handled actions are subject to the same capability gate as host
    // calls, so a plugin cannot run a command or open a URL it never declared.
    let owner = ui
        .state
        .borrow()
        .session
        .as_ref()
        .map(|session| session.plugin_id.clone());
    let allowed = |cap: &str| -> bool {
        match &owner {
            Some(plugin_id) => capability_allowed(ui, plugin_id, cap),
            None => false,
        }
    };
    let deny = |ui: &Ui, cap: &str| {
        warn!("blocked action requiring undeclared '{cap}' capability");
        show_error(&ui.status, &i18n::error_capability(cap));
    };

    match action.kind {
        ActionKind::Plugin => send_event(
            ui,
            ViewEvent::Action {
                action_id: action.id,
                item_id,
            },
        ),
        ActionKind::Copy { text } | ActionKind::Paste { text } => {
            if !allowed("clipboard") {
                return deny(ui, "clipboard");
            }
            ui.window.clipboard().set_text(&text);
            finish_interaction(ui);
        }
        ActionKind::OpenUrl { url } => {
            if !allowed("open") {
                return deny(ui, "open");
            }
            let _ = launch::run_command(&["xdg-open".to_string(), url]);
            finish_interaction(ui);
        }
        ActionKind::Run { argv } => {
            if !allowed("run") {
                return deny(ui, "run");
            }
            let _ = launch::run_command(&argv);
            finish_interaction(ui);
        }
        ActionKind::Close => finish_interaction(ui),
    }
}

/// A host-handled action completed: leave the session and hide the launcher.
fn finish_interaction(ui: &Ui) {
    if ui.state.borrow().session.is_some() {
        exit_session(ui);
    }
    ui.window.set_visible(false);
}

/// Alt+Enter: show the available actions for the current selection in a popover.
fn open_action_menu(ui: &Ui) {
    let actions = current_actions(ui);
    if actions.is_empty() {
        return;
    }

    let popover = gtk::Popover::new();
    popover.set_parent(&ui.entry);
    let list = gtk::ListBox::new();
    list.add_css_class("results-list");

    for (action, item_id) in actions {
        let row = gtk::ListBoxRow::new();
        let label = gtk::Label::builder()
            .label(&action.title)
            .xalign(0.0)
            .build();
        label.add_css_class("result-name");
        row.set_child(Some(&label));
        list.append(&row);

        let ui = ui.clone();
        let popover = popover.clone();
        row.connect_activate(move |_| {
            popover.popdown();
            run_action(&ui, action.clone(), item_id.clone());
        });
    }

    popover.set_child(Some(&list));
    popover.popup();
}

/// The actions offered for the current selection (item actions + view actions).
fn current_actions(ui: &Ui) -> Vec<(nursearch_proto::Action, Option<String>)> {
    let st = ui.state.borrow();
    let Some(session) = st.session.as_ref() else {
        return Vec::new();
    };
    let Some(view) = session.stack.last() else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    match view {
        View::List(list) => {
            if let Some(item) = st
                .active_list
                .as_ref()
                .and_then(|list_box| list_box.selected_row())
                .map(|row| row.index() as usize)
                .and_then(|index| list.items.get(index))
            {
                for action in &item.actions {
                    actions.push((action.clone(), Some(item.id.clone())));
                }
            }
            for action in &list.actions {
                actions.push((action.clone(), None));
            }
        }
        View::Detail(detail) => {
            for action in &detail.actions {
                actions.push((action.clone(), None));
            }
        }
        View::Form(form) => {
            for action in &form.actions {
                actions.push((action.clone(), None));
            }
        }
    }
    actions
}

fn set_entry_text_silently(ui: &Ui, text: &str) {
    ui.state.borrow().suppress_input.set(true);
    ui.entry.set_text(text);
    ui.state.borrow().suppress_input.set(false);
}

/// Build the footer row of keyboard hints shown beneath the results.
fn hint_bar() -> gtk::Box {
    let bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(16)
        .halign(gtk::Align::Start)
        .build();
    bar.add_css_class("hint-bar");

    let hints = [
        ("↵", i18n::hint_open()),
        ("↑ ↓", i18n::hint_navigate()),
        ("Esc", i18n::hint_close()),
    ];
    for (key, label) in hints {
        let item = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(5)
            .build();
        item.add_css_class("hint-item");

        let key_label = gtk::Label::new(Some(key));
        key_label.add_css_class("hint-key");
        let text_label = gtk::Label::new(Some(label));
        text_label.add_css_class("hint-item");

        item.append(&key_label);
        item.append(&text_label);
        bar.append(&item);
    }

    bar
}
