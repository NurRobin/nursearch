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
use std::path::Path;
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
    let (db, startup_error) = match HistoryDb::open() {
        Ok(db) => (db, None),
        Err(err) => match HistoryDb::open_in_memory() {
            Ok(db) => (
                db,
                Some(format!(
                    "History database is unavailable; using temporary history: {err}"
                )),
            ),
            Err(memory_err) => {
                eprintln!("Failed to open history database: {err}");
                eprintln!("Failed to open temporary history database: {memory_err}");
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
    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    status.add_css_class("error-status");

    root.append(&entry);
    root.append(&status);
    root.append(&list);
    window.set_child(Some(&root));

    install_css();
    if let Some(error) = startup_error {
        show_error(&status, &error);
    }
    refresh_results(&state, &entry, &list);

    {
        let state = Rc::clone(&state);
        let list = list.clone();
        let status = status.clone();
        entry.connect_changed(move |entry| {
            status.set_visible(false);
            refresh_results(&state, entry, &list);
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
        let state = Rc::clone(&state);
        let entry = entry.clone();
        let status = status.clone();
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
                launch_index(&state, entry.text().as_ref(), index, &window, &status);
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

        row.set_child(Some(&result_row(result)));
        list.append(&row);
    }

    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }
}

fn result_row(result: &RankedApp) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
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
    image.set_pixel_size(24);
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
            show_error(status, &format!("Could not update launch history: {err}"));
        }
    }

    match launch::launch(&app) {
        Ok(()) => window.close(),
        Err(err) => show_error(status, &format!("Could not launch {}: {err}", app.name)),
    }
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
            min-height: 46px;
            padding: 4px 8px;
        }

        row:selected {
            background: #3f6f8f;
        }

        .result-row {
            padding: 3px 0;
        }

        .result-name {
            font-size: 15px;
        }

        .result-detail {
            color: #b8b8b8;
            font-size: 12px;
        }

        .error-status {
            color: #ffb4a8;
            font-size: 13px;
            padding: 0 4px;
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
