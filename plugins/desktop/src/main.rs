//! Desktop Toggles plugin. Keyword: `d`
//!
//! Provides quick toggles for KDE Plasma 6 desktop settings:
//! - Night Color: toggle via `org.kde.KWin.NightLight` inhibit/uninhibit D-Bus calls
//! - Do Not Disturb: toggle via `org.freedesktop.Notifications` Inhibit/UnInhibit
//!
//! Both toggles are inhibitions held on this plugin's own D-Bus connection, the
//! same mechanism the Plasma applets use: they last while NurSearch runs and
//! end when it quits or the toggle is flipped back. The shown state is read
//! from the system, so an inhibition by another app shows up too.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{Item, ListView, ResultItem, View, ViewEvent};
use std::collections::HashMap;
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

#[derive(Default)]
struct Desktop {
    backend: Backend,
}

impl Plugin for Desktop {
    fn query(&mut self, _host: &mut dyn HostApi, _text: &str) -> Vec<ResultItem> {
        vec![ResultItem {
            id: "open".to_string(),
            title: "Desktop Toggles".to_string(),
            subtitle: Some(status_summary(&mut self.backend)),
            icon: Some("preferences-desktop".to_string()),
            score: 5_200,
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
        if command_id == "open" {
            return Some(Response::Render(toggles_view(&mut self.backend)));
        }
        None
    }

    fn event(&mut self, _host: &mut dyn HostApi, event: ViewEvent) -> Option<Response> {
        match event {
            ViewEvent::Action { action_id, item_id } => {
                let target = item_id.as_deref().unwrap_or(&action_id);
                match target {
                    "toggle-nightcolor" => self.backend.toggle(Toggle::NightColor),
                    "toggle-dnd" => self.backend.toggle(Toggle::DoNotDisturb),
                    _ => {}
                }
                Some(Response::Replace(toggles_view(&mut self.backend)))
            }
            _ => None,
        }
    }
}

fn status_summary(backend: &mut Backend) -> String {
    let nc = if backend.is_inhibited(&NIGHT_COLOR) {
        "Night Color: Off"
    } else {
        "Night Color: On"
    };
    let dnd = if backend.is_inhibited(&DO_NOT_DISTURB) {
        "DND: On"
    } else {
        "DND: Off"
    };
    format!("{nc}  •  {dnd}")
}

fn toggles_view(backend: &mut Backend) -> View {
    let nc_inhibited = backend.is_inhibited(&NIGHT_COLOR);
    let dnd_on = backend.is_inhibited(&DO_NOT_DISTURB);

    View::List(ListView {
        title: Some("Desktop Toggles".to_string()),
        items: vec![
            Item {
                id: "toggle-nightcolor".to_string(),
                title: if nc_inhibited {
                    "Night Color: Off — click to re-enable"
                } else {
                    "Night Color: On — click to disable"
                }
                .to_string(),
                subtitle: Some("org.kde.KWin.NightLight  (KWin night light filter)".to_string()),
                icon: Some(
                    if nc_inhibited {
                        "weather-clear"
                    } else {
                        "night-light"
                    }
                    .to_string(),
                ),
                accessories: Vec::new(),
                actions: Vec::new(),
            },
            Item {
                id: "toggle-dnd".to_string(),
                title: if dnd_on {
                    "Do Not Disturb: On — click to disable"
                } else {
                    "Do Not Disturb: Off — click to enable"
                }
                .to_string(),
                subtitle: Some(
                    "org.freedesktop.Notifications  (suppress system notifications)".to_string(),
                ),
                icon: Some(
                    if dnd_on {
                        "notifications-disabled"
                    } else {
                        "preferences-desktop-notification"
                    }
                    .to_string(),
                ),
                accessories: Vec::new(),
                actions: Vec::new(),
            },
        ],
        empty_text: None,
        placeholder: None,
        actions: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// D-Bus backend
// ---------------------------------------------------------------------------

/// A toggle implemented as a D-Bus inhibition held by this process.
struct Inhibitor {
    service: &'static str,
    path: &'static str,
    interface: &'static str,
    inhibit: &'static str,
    uninhibit: &'static str,
    /// Boolean property telling whether *anyone* currently inhibits.
    property: &'static str,
}

const NIGHT_COLOR: Inhibitor = Inhibitor {
    service: "org.kde.KWin",
    path: "/org/kde/KWin/NightLight",
    interface: "org.kde.KWin.NightLight",
    inhibit: "inhibit",
    uninhibit: "uninhibit",
    property: "inhibited",
};

const DO_NOT_DISTURB: Inhibitor = Inhibitor {
    service: "org.freedesktop.Notifications",
    path: "/org/freedesktop/Notifications",
    interface: "org.freedesktop.Notifications",
    inhibit: "Inhibit",
    uninhibit: "UnInhibit",
    property: "Inhibited",
};

/// Holds the session-bus connection and our inhibition cookies. KWin and the
/// Plasma notification server drop an inhibition as soon as the connection
/// that requested it goes away (a one-shot `busctl` call is undone the moment
/// it exits), so the connection must live as long as the toggle should.
#[derive(Default)]
struct Backend {
    connection: Option<Connection>,
    night_color_cookie: Option<u32>,
    dnd_cookie: Option<u32>,
}

impl Backend {
    fn connection(&mut self) -> Option<&Connection> {
        if self.connection.is_none() {
            self.connection = Connection::session().ok();
        }
        self.connection.as_ref()
    }

    /// Whether the inhibition is active, by anyone (not only by us).
    fn is_inhibited(&mut self, target: &Inhibitor) -> bool {
        let Some(connection) = self.connection() else {
            return false;
        };
        connection
            .call_method(
                Some(target.service),
                target.path,
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(target.interface, target.property),
            )
            .ok()
            .and_then(|reply| reply.body().deserialize::<OwnedValue>().ok())
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(false)
    }

    fn toggle(&mut self, which: Toggle) {
        let (target, cookie) = match which {
            Toggle::NightColor => (&NIGHT_COLOR, self.night_color_cookie),
            Toggle::DoNotDisturb => (&DO_NOT_DISTURB, self.dnd_cookie),
        };
        let new_cookie = match cookie {
            Some(cookie) => {
                self.release(target, cookie);
                None
            }
            None => self.acquire(target),
        };
        match which {
            Toggle::NightColor => self.night_color_cookie = new_cookie,
            Toggle::DoNotDisturb => self.dnd_cookie = new_cookie,
        }
    }

    fn acquire(&mut self, target: &Inhibitor) -> Option<u32> {
        let connection = self.connection()?;
        let reply = if target.inhibit == "Inhibit" {
            connection.call_method(
                Some(target.service),
                target.path,
                Some(target.interface),
                target.inhibit,
                &("nursearch", "Do Not Disturb", HashMap::<&str, Value>::new()),
            )
        } else {
            connection.call_method(
                Some(target.service),
                target.path,
                Some(target.interface),
                target.inhibit,
                &(),
            )
        };
        reply.ok()?.body().deserialize::<u32>().ok()
    }

    fn release(&mut self, target: &Inhibitor, cookie: u32) {
        if let Some(connection) = self.connection() {
            let _ = connection.call_method(
                Some(target.service),
                target.path,
                Some(target.interface),
                target.uninhibit,
                &(cookie,),
            );
        }
    }
}

#[derive(Clone, Copy)]
enum Toggle {
    NightColor,
    DoNotDisturb,
}

fn main() {
    run(Desktop::default());
}
