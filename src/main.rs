mod db;
mod desktop;
mod launch;
mod rank;

use db::HistoryDb;
use desktop::{DesktopEntry, discover_apps};
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use log::{debug, error, info, warn};
use rank::{RankedApp, rank_apps};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

const APP_ID: &str = "dev.nursearch.NurSearch";

struct AppState {
    apps: Vec<DesktopEntry>,
    results: Vec<RankedApp>,
    db: HistoryDb,
}

fn main() -> glib::ExitCode {
    init_logging();
    info!("starting NurSearch");

    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &gtk::Application) {
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
                (
                    db,
                    Some(format!(
                        "History database is unavailable; using temporary history: {err}"
                    )),
                )
            }
            Err(memory_err) => {
                error!("failed to open history database: {err}");
                error!("failed to open temporary history database: {memory_err}");
                return;
            }
        },
    };

    let state = Rc::new(RefCell::new(AppState {
        apps,
        results: Vec::new(),
        db,
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
        .placeholder_text("Apps suchen")
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
        .label("Keine Treffer")
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

    root.append(&entry);
    root.append(&status);
    root.append(&scroller);
    root.append(&empty);
    window.set_child(Some(&root));

    install_css();
    if let Some(error) = startup_error {
        show_error(&status, &error);
    }
    refresh_results(&state, &entry, &list, &empty);

    {
        let state = Rc::clone(&state);
        let list = list.clone();
        let empty = empty.clone();
        let status = status.clone();
        entry.connect_changed(move |entry| {
            status.set_visible(false);
            refresh_results(&state, entry, &list, &empty);
        });
    }

    {
        let state = Rc::clone(&state);
        let window = window.clone();
        let status = status.clone();
        entry.connect_activate(move |entry| {
            launch_index(&state, entry.text().as_ref(), 0, &window, &status)
        });
    }

    {
        let state = Rc::clone(&state);
        let entry = entry.clone();
        let window = window.clone();
        let status = status.clone();
        list.connect_row_activated(move |_, row| {
            launch_index(
                &state,
                entry.text().as_ref(),
                row.index() as usize,
                &window,
                &status,
            );
        });
    }

    let key_controller = gtk::EventControllerKey::new();
    {
        let window = window.clone();
        let list = list.clone();
        let scroller = scroller.clone();
        let state = Rc::clone(&state);
        let entry = entry.clone();
        let status = status.clone();
        key_controller.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape => {
                window.close();
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                move_selection(&list, &scroller, 1);
                glib::Propagation::Stop
            }
            gdk::Key::Up => {
                move_selection(&list, &scroller, -1);
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                let index = list
                    .selected_row()
                    .map(|row| row.index() as usize)
                    .unwrap_or(0);
                launch_index(&state, entry.text().as_ref(), index, &window, &status);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
    }
    window.add_controller(key_controller);

    window.present();
    entry.grab_focus();
    debug!("launcher window presented");
}

fn init_logging() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("nursearch=info"))
        .format_timestamp_secs()
        .init();
}

fn refresh_results(
    state: &Rc<RefCell<AppState>>,
    entry: &gtk::Entry,
    list: &gtk::ListBox,
    empty: &gtk::Label,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let query = entry.text();
    let mut state = state.borrow_mut();
    state.results = rank_apps(&state.apps, query.as_ref(), &state.db);
    debug!(
        "query refreshed: query={:?}, results={}",
        query.as_str(),
        state.results.len()
    );

    for result in &state.results {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(true);
        row.set_selectable(true);
        row.add_css_class("result-list-row");

        row.set_child(Some(&result_row(result)));
        list.append(&row);
    }

    let has_results = !state.results.is_empty();
    empty.set_visible(!has_results);
    list.set_visible(has_results);

    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }
}

fn result_row(result: &RankedApp) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .valign(gtk::Align::Center)
        .build();
    row.add_css_class("result-row");

    let image = icon_image(result.app.icon.as_deref());
    row.append(&image);

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(1)
        .hexpand(true)
        .build();

    let name = gtk::Label::builder()
        .label(&result.app.name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    name.add_css_class("result-name");
    text.append(&name);

    if let Some(detail_text) = row_detail_text(result) {
        let detail = gtk::Label::builder()
            .label(&detail_text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        detail.add_css_class("result-detail");
        text.append(&detail);
    }

    row.append(&text);
    row
}

fn row_detail_text(result: &RankedApp) -> Option<String> {
    result
        .app
        .generic_name
        .as_ref()
        .or(result.app.comment.as_ref())
        .cloned()
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
    let app = {
        let state = state.borrow();
        state.results.get(index).map(|result| result.app.clone())
    };

    let Some(app) = app else {
        return;
    };

    {
        let state = state.borrow();
        if let Err(err) = state.db.record_launch(query, &app.path.to_string_lossy()) {
            warn!("could not update launch history for {}: {err}", app.name);
            show_error(status, &format!("Could not update launch history: {err}"));
        }
    }

    info!("launching app: {}", app.name);
    match launch::launch(&app) {
        Ok(()) => {
            debug!("launch succeeded: {}", app.name);
            window.close();
        }
        Err(err) => {
            error!("could not launch {}: {err}", app.name);
            show_error(status, &format!("Could not launch {}: {err}", app.name));
        }
    }
}

fn move_selection(list: &gtk::ListBox, scroller: &gtk::ScrolledWindow, direction: i32) {
    let current = list.selected_row().map(|row| row.index()).unwrap_or(0);
    let next = (current + direction).max(0);

    if let Some(row) = list.row_at_index(next) {
        list.select_row(Some(&row));
        keep_row_visible(list, scroller, &row);
    }
}

fn keep_row_visible(list: &gtk::ListBox, scroller: &gtk::ScrolledWindow, row: &gtk::ListBoxRow) {
    let Some(bounds) = row.compute_bounds(list) else {
        return;
    };

    let adjustment = scroller.vadjustment();
    let top = bounds.y() as f64;
    let bottom = top + bounds.height() as f64;
    let viewport_top = adjustment.value();
    let viewport_bottom = viewport_top + adjustment.page_size();

    if top < viewport_top {
        adjustment.set_value(top.max(adjustment.lower()));
    } else if bottom > viewport_bottom {
        let target = bottom - adjustment.page_size();
        adjustment.set_value(target.min(adjustment.upper() - adjustment.page_size()));
    }
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "
        window {
            background: transparent;
        }

        .launcher-shell {
            background: #24262a;
            border: 1px solid rgba(255, 255, 255, 0.16);
            border-radius: 16px;
        }

        .search-entry {
            min-height: 54px;
            border-radius: 12px;
            border: 1px solid rgba(255, 255, 255, 0.12);
            background: #303238;
            color: #f7f2ea;
            font-size: 21px;
            padding: 6px 14px;
        }

        .search-entry:focus {
            border-color: #d3a03c;
            box-shadow: 0 0 0 2px rgba(211, 160, 60, 0.24);
        }

        .results-scroller {
            min-height: 356px;
            background: transparent;
            border: none;
        }

        scrollbar slider {
            min-width: 8px;
            min-height: 8px;
            border-radius: 999px;
            background: rgba(255, 255, 255, 0.18);
        }

        .results-list {
            background: transparent;
        }

        .result-list-row {
            min-height: 58px;
            margin: 1px 0;
            padding: 0;
            border-radius: 10px;
            background: transparent;
        }

        .result-list-row:selected {
            background: #375f73;
        }

        .result-row {
            padding: 9px 12px;
        }

        .result-name {
            color: #f7f2ea;
            font-size: 16px;
            font-weight: 600;
        }

        .result-detail {
            color: #aeb4bd;
            font-size: 13px;
        }

        .result-list-row:selected .result-detail {
            color: #d9edf5;
        }

        .empty-state {
            color: #aeb4bd;
            font-size: 14px;
            padding: 2px 4px 0;
        }

        .error-status {
            color: #ffb7a8;
            font-size: 13px;
            padding: 0 6px;
        }
        ",
    );

    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
