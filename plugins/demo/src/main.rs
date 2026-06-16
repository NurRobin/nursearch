//! Reference plugin demonstrating the protocol: root contributions, a List
//! view with per-item actions, drill-in to a Detail view, and a host-handled
//! copy action. Activated with the `demo` keyword.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{
    Action, ActionKind, DetailView, Item, ListView, MetaPair, ResultItem, View, ViewEvent,
};

struct Demo {
    items: Vec<(&'static str, &'static str)>,
}

impl Plugin for Demo {
    fn query(&mut self, _host: &mut dyn HostApi, text: &str) -> Vec<ResultItem> {
        vec![ResultItem {
            id: "open".to_string(),
            title: "Demo: browse items".to_string(),
            subtitle: Some(if text.is_empty() {
                "Open the demo list".to_string()
            } else {
                format!("Open the demo list ({text})")
            }),
            icon: Some("applications-other".to_string()),
            score: 5_000,
            command_id: "open".to_string(),
            actions: Vec::new(),
        }]
    }

    fn activate(
        &mut self,
        _host: &mut dyn HostApi,
        command_id: &str,
        _item_id: Option<String>,
    ) -> Option<Response> {
        (command_id == "open").then(|| Response::Render(self.list_view("")))
    }

    fn event(&mut self, _host: &mut dyn HostApi, event: ViewEvent) -> Option<Response> {
        match event {
            ViewEvent::Input { text } => Some(Response::Replace(self.list_view(&text))),
            ViewEvent::Action {
                action_id,
                item_id: Some(item_id),
            } if action_id == "detail" => Some(Response::Render(self.detail_view(&item_id))),
            _ => None,
        }
    }
}

impl Demo {
    fn list_view(&self, filter: &str) -> View {
        let needle = filter.to_lowercase();
        let items = self
            .items
            .iter()
            .filter(|(title, _)| title.to_lowercase().contains(&needle))
            .map(|(title, body)| Item {
                id: title.to_string(),
                title: title.to_string(),
                subtitle: Some(body.to_string()),
                icon: None,
                accessories: Vec::new(),
                actions: vec![
                    Action {
                        id: "detail".to_string(),
                        title: "Show detail".to_string(),
                        icon: None,
                        shortcut: None,
                        kind: ActionKind::Plugin,
                    },
                    Action {
                        id: "copy".to_string(),
                        title: "Copy name".to_string(),
                        icon: None,
                        shortcut: None,
                        kind: ActionKind::Copy {
                            text: title.to_string(),
                        },
                    },
                ],
            })
            .collect();
        View::List(ListView {
            title: Some("Demo items".to_string()),
            placeholder: Some("Filter demo items".to_string()),
            items,
            empty_text: Some("No matching items".to_string()),
            actions: Vec::new(),
        })
    }

    fn detail_view(&self, id: &str) -> View {
        let body = self
            .items
            .iter()
            .find(|(title, _)| *title == id)
            .map(|(_, body)| body.to_string())
            .unwrap_or_default();
        View::Detail(DetailView {
            title: Some(id.to_string()),
            placeholder: None,
            markdown: Some(body),
            metadata: vec![MetaPair {
                label: "Item".to_string(),
                value: id.to_string(),
            }],
            actions: vec![Action {
                id: "copy".to_string(),
                title: "Copy name".to_string(),
                icon: None,
                shortcut: None,
                kind: ActionKind::Copy {
                    text: id.to_string(),
                },
            }],
        })
    }
}

fn main() {
    run(Demo {
        items: vec![
            ("Alpha", "The first demo item."),
            ("Beta", "The second demo item."),
            ("Gamma", "The third demo item."),
        ],
    });
}
