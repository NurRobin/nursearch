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
use rank::{RankedApp, rank_apps};
use std::cell::RefCell;
use std::rc::Rc;

const APP_ID: &str = "dev.nursearch.NurSearch";

struct AppState {
    apps: Vec<DesktopEntry>,
    results: Vec<RankedApp>,
    db: HistoryDb,
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &gtk::Application) {
    let apps = discover_apps();
    let db = match HistoryDb::open() {
        Ok(db) => db,
        Err(err) => {
            eprintln!("Failed to open history database: {err}");
            return;
        }
    };

    let state = Rc::new(RefCell::new(AppState {
        apps,
        results: Vec::new(),
        db,
    }));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("NurSearch")
        .default_width(640)
        .default_height(420)
        .decorated(false)
        .resizable(false)
        .build();

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let entry = gtk::Entry::builder()
        .placeholder_text("Search apps")
        .hexpand(true)
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .activate_on_single_click(false)
        .build();

    root.append(&entry);
    root.append(&list);
    window.set_child(Some(&root));

    install_css();
    refresh_results(&state, &entry, &list);

    {
        let state = Rc::clone(&state);
        let list = list.clone();
        entry.connect_changed(move |entry| refresh_results(&state, entry, &list));
    }

    {
        let state = Rc::clone(&state);
        let window = window.clone();
        entry
            .connect_activate(move |entry| launch_index(&state, entry.text().as_ref(), 0, &window));
    }

    {
        let state = Rc::clone(&state);
        let entry = entry.clone();
        let window = window.clone();
        list.connect_row_activated(move |_, row| {
            launch_index(&state, entry.text().as_ref(), row.index() as usize, &window);
        });
    }

    let key_controller = gtk::EventControllerKey::new();
    {
        let window = window.clone();
        let list = list.clone();
        let state = Rc::clone(&state);
        let entry = entry.clone();
        key_controller.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape => {
                window.close();
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                move_selection(&list, 1);
                glib::Propagation::Stop
            }
            gdk::Key::Up => {
                move_selection(&list, -1);
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                let index = list
                    .selected_row()
                    .map(|row| row.index() as usize)
                    .unwrap_or(0);
                launch_index(&state, entry.text().as_ref(), index, &window);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
    }
    window.add_controller(key_controller);

    window.present();
    entry.grab_focus();
}

fn refresh_results(state: &Rc<RefCell<AppState>>, entry: &gtk::Entry, list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let query = entry.text();
    let mut state = state.borrow_mut();
    state.results = rank_apps(&state.apps, query.as_ref(), &state.db);

    for result in &state.results {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(true);
        row.set_selectable(true);

        let label = gtk::Label::builder()
            .label(row_text(result))
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        row.set_child(Some(&label));
        list.append(&row);
    }

    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }
}

fn row_text(result: &RankedApp) -> String {
    match &result.app.icon {
        Some(icon) if !icon.is_empty() => format!("{}   {}", result.app.name, icon),
        _ => result.app.name.clone(),
    }
}

fn launch_index(
    state: &Rc<RefCell<AppState>>,
    query: &str,
    index: usize,
    window: &gtk::ApplicationWindow,
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
            eprintln!("Failed to record launch history: {err}");
        }
    }

    if let Err(err) = launch::launch(&app) {
        eprintln!("Failed to launch {}: {err}", app.name);
    }

    window.close();
}

fn move_selection(list: &gtk::ListBox, direction: i32) {
    let current = list.selected_row().map(|row| row.index()).unwrap_or(0);
    let next = (current + direction).max(0);

    if let Some(row) = list.row_at_index(next) {
        list.select_row(Some(&row));
    }
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "
        window {
            background: #242424;
            border: 1px solid #5d5d5d;
        }

        entry {
            min-height: 42px;
            font-size: 20px;
            padding: 6px 10px;
        }

        list {
            background: transparent;
        }

        row {
            min-height: 34px;
            padding: 5px 8px;
        }

        row:selected {
            background: #3f6f8f;
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
