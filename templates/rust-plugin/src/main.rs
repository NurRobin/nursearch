//! A starter NurSearch plugin in Rust using the SDK.
//! See docs/AGENTS-PLUGIN-GUIDE.md and the worked examples under plugins/.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{DetailView, ResultItem, View, ViewEvent};

struct MyPlugin;

impl Plugin for MyPlugin {
    fn query(&mut self, _host: &mut dyn HostApi, text: &str) -> Vec<ResultItem> {
        vec![ResultItem {
            id: "open".to_string(),
            title: format!("Echo: {text}"),
            subtitle: Some("Open a detail view".to_string()),
            icon: None,
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
        (command_id == "open").then(|| {
            Response::Render(View::Detail(DetailView {
                title: Some("Hello".to_string()),
                markdown: Some("This is your plugin's detail view.".to_string()),
                ..Default::default()
            }))
        })
    }

    fn event(&mut self, _host: &mut dyn HostApi, _event: ViewEvent) -> Option<Response> {
        None
    }
}

fn main() {
    run(MyPlugin);
}
