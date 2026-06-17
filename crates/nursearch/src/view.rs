//! Builds GTK widgets from the declarative plugin view tree. These builders are
//! pure (no behavior wiring); `main` attaches navigation and actions.

use crate::icon_image;
use gtk::prelude::*;
use gtk4 as gtk;
use nursearch_proto::{DetailView, Field, FieldType, FormView, Item, ListView};

/// A built form field paired with a way to read its current value.
pub struct FormField {
    pub id: String,
    widget: FieldWidget,
}

enum FieldWidget {
    Text(gtk::Entry),
    Password(gtk::PasswordEntry),
    Number(gtk::SpinButton),
    Select(gtk::DropDown, Vec<String>),
    Checkbox(gtk::CheckButton),
}

impl FormField {
    pub fn value(&self) -> String {
        match &self.widget {
            FieldWidget::Text(entry) => entry.text().to_string(),
            FieldWidget::Password(entry) => entry.text().to_string(),
            FieldWidget::Number(spin) => {
                let value = spin.value();
                if value.fract() == 0.0 {
                    format!("{}", value as i64)
                } else {
                    format!("{value}")
                }
            }
            FieldWidget::Select(dropdown, values) => values
                .get(dropdown.selected() as usize)
                .cloned()
                .unwrap_or_default(),
            FieldWidget::Checkbox(check) => check.is_active().to_string(),
        }
    }
}

/// Build a scrollable list view, returning its container and the inner list box
/// so the caller can drive selection.
pub fn build_list(view: &ListView) -> (gtk::Widget, gtk::ListBox) {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .activate_on_single_click(false)
        .build();
    list.add_css_class("results-list");

    for item in &view.items {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(true);
        row.add_css_class("result-list-row");
        row.set_child(Some(&item_row(item)));
        list.append(&row);
    }
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&list)
        .build();
    scroller.add_css_class("results-scroller");

    if view.items.is_empty() {
        let empty = gtk::Label::builder()
            .label(view.empty_text.as_deref().unwrap_or(""))
            .xalign(0.0)
            .build();
        empty.add_css_class("empty-state");
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.append(&empty);
        return (container.upcast(), list);
    }

    (scroller.upcast(), list)
}

fn item_row(item: &Item) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .valign(gtk::Align::Center)
        .build();
    row.add_css_class("result-row");

    row.append(&icon_image(item.icon.as_deref()));

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(1)
        .hexpand(true)
        .build();
    let title = gtk::Label::builder()
        .label(&item.title)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("result-name");
    text.append(&title);
    if let Some(subtitle) = item.subtitle.as_deref() {
        let detail = gtk::Label::builder()
            .label(subtitle)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        detail.add_css_class("result-detail");
        text.append(&detail);
    }
    row.append(&text);

    for accessory in &item.accessories {
        let tag = gtk::Label::builder()
            .label(accessory)
            .valign(gtk::Align::Center)
            .build();
        tag.add_css_class("result-badge");
        row.append(&tag);
    }
    row
}

/// Build a detail view (markdown shown as wrapped text plus a metadata table).
pub fn build_detail(view: &DetailView) -> gtk::Widget {
    let outer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    body.add_css_class("detail-body");

    if let Some(markdown) = view.markdown.as_deref() {
        let text = gtk::Label::builder()
            .label(markdown)
            .xalign(0.0)
            .wrap(true)
            .build();
        text.add_css_class("detail-text");
        body.append(&text);
    }

    for pair in &view.metadata {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let label = gtk::Label::builder().label(&pair.label).xalign(0.0).build();
        label.add_css_class("result-detail");
        let value = gtk::Label::builder()
            .label(&pair.value)
            .xalign(1.0)
            .hexpand(true)
            .wrap(true)
            .build();
        value.add_css_class("result-name");
        line.append(&label);
        line.append(&value);
        body.append(&line);
    }

    scroller.set_child(Some(&body));
    outer.append(&scroller);
    outer.upcast()
}

/// Build a form view, returning the container and the fields for value reads.
pub fn build_form(view: &FormView) -> (gtk::Widget, Vec<FormField>) {
    let outer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .vexpand(true)
        .build();
    outer.add_css_class("form-body");

    let mut fields = Vec::new();
    for field in &view.fields {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let label = gtk::Label::builder()
            .label(&field.label)
            .xalign(0.0)
            .build();
        label.add_css_class("result-detail");
        row.append(&label);

        let widget = build_field(field, &row);
        fields.push(FormField {
            id: field.id.clone(),
            widget,
        });
        outer.append(&row);
    }
    (outer.upcast(), fields)
}

fn build_field(field: &Field, row: &gtk::Box) -> FieldWidget {
    match field.field_type {
        FieldType::Text => {
            let entry = gtk::Entry::new();
            entry.add_css_class("search-entry");
            if let Some(value) = field.value.as_deref() {
                entry.set_text(value);
            }
            if let Some(placeholder) = field.placeholder.as_deref() {
                entry.set_placeholder_text(Some(placeholder));
            }
            row.append(&entry);
            FieldWidget::Text(entry)
        }
        FieldType::Password => {
            let entry = gtk::PasswordEntry::new();
            entry.set_show_peek_icon(true);
            row.append(&entry);
            FieldWidget::Password(entry)
        }
        FieldType::Number => {
            let spin = gtk::SpinButton::with_range(f64::MIN, f64::MAX, 1.0);
            if let Some(value) = field.value.as_deref().and_then(|v| v.parse::<f64>().ok()) {
                spin.set_value(value);
            }
            row.append(&spin);
            FieldWidget::Number(spin)
        }
        FieldType::Select => {
            let values: Vec<String> = field.options.iter().map(|o| o.value.clone()).collect();
            let labels: Vec<&str> = field.options.iter().map(|o| o.label.as_str()).collect();
            let dropdown = gtk::DropDown::from_strings(&labels);
            if let Some(value) = field.value.as_deref()
                && let Some(index) = values.iter().position(|v| v == value)
            {
                dropdown.set_selected(index as u32);
            }
            row.append(&dropdown);
            FieldWidget::Select(dropdown, values)
        }
        FieldType::Checkbox => {
            let check = gtk::CheckButton::new();
            check.set_active(field.value.as_deref() == Some("true"));
            row.append(&check);
            FieldWidget::Checkbox(check)
        }
    }
}
