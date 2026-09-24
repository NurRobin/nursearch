mod calc;
mod config;
mod db;
mod desktop;
mod direct;
mod i18n;
mod kcm;
mod launch;
mod plugin;
mod rank;
mod search;
mod shortcut;
mod system;
mod view;

use db::{HistoryDb, StatsSnapshot};
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
const WINDOW_HEIGHT: i32 = 520;

struct AppState {
    apps: Vec<DesktopEntry>,
    /// KDE System Settings pages; empty outside Plasma.
    settings_pages: Vec<kcm::SettingsPage>,
    /// `config.toml`, re-read each time the launcher opens.
    config: config::Settings,
    /// Home directory for `~` paths.
    home: Option<std::path::PathBuf>,
    /// Pill inside the search field naming the active keyword mode.
    mode_pill: Option<gtk::Label>,
    /// The merged, ranked list currently shown on the root screen.
    results: Vec<SearchResult>,
    db: HistoryDb,
    /// Plugin host; set after the UI sink exists.
    host: Option<PluginHost>,
    /// Monotonic query counter used to discard stale async plugin results.
    generation: u64,
    /// Latest core results for the current generation (instant, in-process).
    core: Vec<SearchResult>,
    /// Usage snapshot for the current root generation. Shared so async plugin
    /// contributions get the same launch-history boost the core results do.
    snapshot: Rc<StatsSnapshot>,
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
    ui: Ui,
}

impl Launcher {
    /// Reset and bring the launcher back to the foreground (daemon re-activation).
    /// Every open starts at an empty search root: a plugin view left open when
    /// the launcher lost focus, and any old error message, are cleared.
    fn show(&self) {
        let ui = &self.ui;
        if ui.state.borrow().session.is_some() {
            exit_session(ui);
        }
        ui.status.set_visible(false);
        reload_settings(ui);
        ui.entry.set_text("");
        ui.window.set_visible(true);
        ui.window.present();
        ui.entry.grab_focus();
    }
}

/// Anchor the window to the top edge of the screen. A regular Wayland window
/// cannot choose its position, so this makes it a layer-shell overlay, the
/// way KRunner-style launchers are placed. Falls back to the centred window
/// when the compositor lacks the protocol.
fn place_at_top(window: &gtk::ApplicationWindow, margin: i32) {
    use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
    if !gtk4_layer_shell::is_supported() {
        warn!("layer-shell is not supported here; keeping the window centred");
        return;
    }
    window.init_layer_shell();
    window.set_namespace(Some("nursearch"));
    window.set_layer(Layer::Overlay);
    // On-demand focus: the launcher gets the keyboard when shown, and clicking
    // elsewhere takes it away, which hides the launcher like a normal window.
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    window.set_anchor(Edge::Top, true);
    window.set_margin(Edge::Top, margin);
}

/// Re-read `config.toml` so edits apply on the next open. A broken file keeps
/// the previous settings and says why.
fn reload_settings(ui: &Ui) {
    let (settings, error) = config::load_settings(&config::settings_path());
    match error {
        Some(error) => show_error(&ui.status, &i18n::error_settings(&error)),
        None => {
            ui.window
                .set_default_size(settings.window.width, WINDOW_HEIGHT);
            ui.state.borrow_mut().config = settings;
        }
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
            let snapshot = Rc::clone(&st.snapshot);
            let converted = items
                .into_iter()
                .map(|item| result_from_plugin(plugin_id, &name, item, &snapshot))
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

    // Default to GTK's software (cairo) renderer. NurSearch only ever draws a
    // small, mostly static launcher window, so GPU acceleration buys nothing —
    // but the Vulkan/GL renderer makes GTK map the full GPU driver stack
    // (on multi-GPU machines, several at once), costing ~150 MiB of resident
    // memory at idle. Cairo cuts idle RSS by roughly two thirds. Users who want
    // GPU rendering can still override this by exporting GSK_RENDERER themselves.
    if std::env::var_os("GSK_RENDERER").is_none() {
        // Safety: called at the very top of `main`, before any threads are
        // spawned and before GTK initializes its renderer.
        unsafe {
            std::env::set_var("GSK_RENDERER", "cairo");
        }
    }

    let background = match parse_cli(std::env::args().skip(1)) {
        Ok(Cli::Launch { background }) => background,
        Ok(Cli::ClearHistory) => return clear_history(),
        Ok(Cli::SetupShortcut) => {
            return match shortcut::setup(true) {
                Ok(message) => {
                    println!("{message}");
                    glib::ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("{err}");
                    glib::ExitCode::FAILURE
                }
            };
        }
        Ok(Cli::Help) => {
            println!("{USAGE}");
            return glib::ExitCode::SUCCESS;
        }
        Err(unknown) => {
            eprintln!("unknown option: {unknown}\n\n{USAGE}");
            return glib::ExitCode::FAILURE;
        }
    };
    let app = gtk::Application::builder().application_id(APP_ID).build();

    if background {
        if let Err(err) = app.register(gio::Cancellable::NONE) {
            error!("could not register the application: {err}");
            return glib::ExitCode::FAILURE;
        }
        // A background start must never pop up an already-running launcher.
        if app.is_remote() {
            info!("NurSearch is already running; nothing to do");
            return glib::ExitCode::SUCCESS;
        }
    }

    // The process stays resident as a daemon: the first invocation builds the
    // window, and every later `nursearch` call re-activates the running instance
    // and simply re-presents it for an instant open. `--background` (used by
    // the login autostart) builds it hidden, so even the first open after login
    // skips GTK's ~300 ms cold start.
    let launcher: Rc<RefCell<Option<Launcher>>> = Rc::new(RefCell::new(None));
    app.connect_activate(move |app| {
        let mut slot = launcher.borrow_mut();
        match slot.as_ref() {
            Some(existing) => existing.show(),
            None => *slot = build_ui(app, !background),
        }
    });
    // Our own flags are handled above; GApplication would reject them.
    app.run_with_args(&std::env::args().take(1).collect::<Vec<_>>())
}

const USAGE: &str = "Usage: nursearch [OPTION]
Open the launcher (or start it, then keep it resident).

  --background       start hidden (used by the login autostart)
  --clear-history    forget all launch history and exit
  --setup-shortcut   bind the Meta key to NurSearch on KDE Plasma and exit
  --help             show this help";

#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Launch { background: bool },
    ClearHistory,
    SetupShortcut,
    Help,
}

/// Parse our own flags; returns the offending argument if one is unknown.
fn parse_cli(args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut cli = Cli::Launch { background: false };
    for arg in args {
        cli = match arg.as_str() {
            "--background" => Cli::Launch { background: true },
            "--clear-history" => Cli::ClearHistory,
            "--setup-shortcut" => Cli::SetupShortcut,
            "-h" | "--help" => Cli::Help,
            _ => return Err(arg),
        };
    }
    Ok(cli)
}

fn clear_history() -> glib::ExitCode {
    match HistoryDb::open().and_then(|db| db.clear_history()) {
        Ok(()) => {
            println!("Launch history cleared.");
            glib::ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("could not clear the launch history: {err}");
            glib::ExitCode::FAILURE
        }
    }
}

fn build_ui(app: &gtk::Application, present: bool) -> Option<Launcher> {
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

    match db.prune() {
        Ok(0) => {}
        Ok(removed) => info!("pruned {removed} stale history entries"),
        Err(err) => warn!("could not prune history: {err}"),
    }
    shortcut::ensure_on_first_run();

    let settings_pages = kcm::discover();
    info!("discovered {} settings pages", settings_pages.len());
    let (settings, settings_error) = config::load_settings(&config::settings_path());

    let state = Rc::new(RefCell::new(AppState {
        apps,
        settings_pages,
        config: settings.clone(),
        home: std::env::var_os("HOME").map(std::path::PathBuf::from),
        mode_pill: None,
        results: Vec::new(),
        db,
        host: None,
        generation: 0,
        core: Vec::new(),
        snapshot: Rc::new(StatsSnapshot::default()),
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
        .default_width(settings.window.width)
        .default_height(WINDOW_HEIGHT)
        .decorated(false)
        .resizable(false)
        .build();

    if settings.window.position == config::Position::Top {
        place_at_top(&window, settings.window.top_margin);
    }

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
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

    // The pill shows which plugin or quicklink a keyword switched to, so
    // typing "f " is visibly a file search rather than a search for "f".
    let mode_pill = gtk::Label::builder()
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .margin_end(14)
        .visible(false)
        .can_target(false)
        .build();
    mode_pill.add_css_class("mode-pill");
    let entry_overlay = gtk::Overlay::builder().child(&entry).build();
    entry_overlay.add_overlay(&mode_pill);
    state.borrow_mut().mode_pill = Some(mode_pill);

    root.append(&entry_overlay);
    root.append(&status);
    root.append(&content_root);
    root.append(&content_session);
    root.append(&hint_bar());
    window.set_child(Some(&root));

    if let Some(error) = startup_error {
        show_error(&status, &error);
    }
    if let Some(error) = settings_error {
        show_error(&status, &i18n::error_settings(&error));
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

    // Dismiss the launcher when it loses focus: clicking another window or
    // tabbing away hides it, matching standard launcher behaviour. The window
    // becomes inactive the moment focus leaves it, so re-showing (which calls
    // `present` + `grab_focus`) re-activates it without re-triggering this.
    window.connect_is_active_notify(move |window| {
        if !window.is_active() {
            window.set_visible(false);
        }
    });

    state.borrow_mut()._dir_monitors = watch_app_dirs(&state, &entry, &list, &empty);

    if present {
        window.present();
        entry.grab_focus();
        debug!("launcher window presented");
    } else {
        info!("launcher ready in the background");
    }

    debug_autodrive(&ui);

    Some(Launcher { ui })
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
                // Package upgrades and Steam touch many files in bursts; wait
                // for the burst, then parse on a worker thread so a rescan of
                // thousands of entries never stalls typing.
                glib::timeout_add_local_once(Duration::from_millis(700), move || {
                    pending.set(false);
                    debug!("reloading apps after .desktop change");
                    glib::spawn_future_local(async move {
                        let Ok(apps) = gio::spawn_blocking(discover_apps).await else {
                            warn!("rescanning applications failed");
                            return;
                        };
                        let in_session = {
                            let mut st = state.borrow_mut();
                            st.apps = apps;
                            st.session.is_some()
                        };
                        // Don't disturb an active plugin session; the refreshed apps
                        // are picked up the next time the root screen is shown.
                        if !in_session {
                            dispatch_query(&state, &entry, &list, &empty);
                        }
                    });
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
    update_mode_pill(
        state,
        entry,
        host.as_ref(),
        keyword.as_ref().map(|(id, _)| id.as_str()),
        &query,
    );

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
        // One snapshot per generation, shared by the core and every plugin
        // contribution so both are ranked with the same launch history.
        let snapshot = Rc::new(st.db.snapshot(&query));
        // A keyword takes over the root, so the core contributes nothing.
        st.core = if keyword.is_some() {
            Vec::new()
        } else {
            let sources = search::Sources {
                apps: &st.apps,
                settings: &st.settings_pages,
                quicklinks: &st.config.quicklinks,
                home: st.home.as_deref(),
            };
            core_results(&sources, &query, &snapshot)
        };
        st.snapshot = snapshot;
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

/// Name the active keyword mode (a plugin keyword or a quicklink) in the pill.
fn update_mode_pill(
    state: &Rc<RefCell<AppState>>,
    entry: &gtk::Entry,
    host: Option<&PluginHost>,
    plugin_id: Option<&str>,
    query: &str,
) {
    let st = state.borrow();
    let Some(pill) = st.mode_pill.as_ref() else {
        return;
    };
    let plugin_name = plugin_id
        .and_then(|id| host.and_then(|host| host.manifest(id)))
        .map(|manifest| manifest.name);
    let quicklink_name = || {
        let (keyword, _) = query.trim_start().split_once(char::is_whitespace)?;
        let keyword = keyword.to_lowercase();
        st.config
            .quicklinks
            .iter()
            .find(|link| link.keyword == keyword)
            .map(|link| link.name.clone())
    };
    match plugin_name.or_else(quicklink_name) {
        Some(name) => {
            pill.set_text(&name);
            pill.set_visible(true);
            entry.add_css_class("with-mode");
        }
        None => {
            pill.set_visible(false);
            entry.remove_css_class("with-mode");
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
        finalize(all, st.config.search.max_results)
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
        Action::AppShortcut { app, action } => launch::launch_action(app, action),
        Action::Run(command) => launch::run_command(command),
        Action::Detached { command, app_id } => launch::spawn_detached(command, app_id),
        Action::Open(target) => launch::open_uri(target),
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
            Ok(()) => match launch::open_uri(&target) {
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
        ActionKind::Copy { text } => {
            if !allowed("clipboard") {
                return deny(ui, "clipboard");
            }
            ui.window.clipboard().set_text(&text);
            finish_interaction(ui);
        }
        ActionKind::Paste { text } => {
            if !allowed("clipboard") {
                return deny(ui, "clipboard");
            }
            ui.window.clipboard().set_text(&text);
            // Hide the launcher first so focus returns to the previous window,
            // then synthesize the paste keystroke into it after a short delay
            // (the compositor needs a moment to restore focus). The text is on
            // the clipboard either way, so a missing paste tool is non-fatal.
            finish_interaction(ui);
            glib::timeout_add_local_once(Duration::from_millis(120), || {
                if let Err(err) = launch::paste_into_focused() {
                    warn!("could not synthesize paste: {err}");
                }
            });
        }
        ActionKind::OpenUrl { url } => {
            if !allowed("open") {
                return deny(ui, "open");
            }
            match launch::open_uri(&url) {
                Ok(()) => finish_interaction(ui),
                Err(err) => show_error(&ui.status, &i18n::error_action(&err.to_string())),
            }
        }
        ActionKind::Run { argv } => {
            if !allowed("run") {
                return deny(ui, "run");
            }
            match launch::run_command(&argv) {
                Ok(()) => finish_interaction(ui),
                Err(err) => show_error(&ui.status, &i18n::error_action(&err.to_string())),
            }
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
    // A popover stays attached to its parent until unparented; without this
    // every Alt+Enter would leak one for the daemon's lifetime.
    popover.connect_closed(|popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
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

#[cfg(test)]
mod tests {
    use super::{Cli, parse_cli};

    fn parse(list: &[&str]) -> Result<Cli, String> {
        parse_cli(list.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn parses_command_line_flags() {
        assert_eq!(parse(&[]), Ok(Cli::Launch { background: false }));
        assert_eq!(
            parse(&["--background"]),
            Ok(Cli::Launch { background: true })
        );
        assert_eq!(parse(&["--clear-history"]), Ok(Cli::ClearHistory));
        assert_eq!(parse(&["--setup-shortcut"]), Ok(Cli::SetupShortcut));
        assert_eq!(parse(&["--other"]), Err("--other".to_string()));
    }
}
