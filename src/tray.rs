use crate::data;
use ksni::blocking::TrayMethods;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FIRST: Duration = Duration::from_secs(90);
const EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Default)]
struct State {
    count: usize,
    restart: bool,
}

struct Tray {
    state: Arc<Mutex<State>>,
    wake: std::sync::mpsc::Sender<()>,
}

fn open_window() {
    let _ = std::process::Command::new("kuaka").spawn();
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "kuaka".into()
    }

    fn title(&self) -> String {
        "Kuaka".into()
    }

    fn icon_name(&self) -> String {
        let s = self.state.lock().unwrap();
        if s.count > 0 || s.restart {
            "system-software-update".into()
        } else {
            "update-none".into()
        }
    }

    fn status(&self) -> ksni::Status {
        let s = self.state.lock().unwrap();
        if s.count > 0 || s.restart {
            ksni::Status::Active
        } else {
            ksni::Status::Passive
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let s = self.state.lock().unwrap();
        let text = if s.restart {
            "Restart to finish updating".to_string()
        } else if s.count == 0 {
            "Fiordland is up to date".to_string()
        } else {
            plural(s.count)
        };
        ksni::ToolTip {
            title: "Kuaka".into(),
            description: text,
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_window();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open Kuaka".into(),
                activate: Box::new(|_: &mut Self| open_window()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Check now".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.wake.send(());
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub fn plural(n: usize) -> String {
    format!("{n} update{} ready", if n == 1 { "" } else { "s" })
}

fn notify(count: usize) {
    std::thread::spawn(move || {
        let shown = notify_rust::Notification::new()
            .appname("Kuaka")
            .summary(&plural(count))
            .body("Open Kuaka to see what's new and update.")
            .icon("system-software-update")
            .action("default", "Open")
            .action("open", "Open Kuaka")
            .show();
        if let Ok(h) = shown {
            h.wait_for_action(|a| {
                if a == "default" || a == "open" {
                    open_window();
                }
            });
        }
    });
}

pub fn run() -> anyhow::Result<()> {
    let state = Arc::new(Mutex::new(State::default()));
    let (wake, rx) = std::sync::mpsc::channel();
    let handle = Tray {
        state: state.clone(),
        wake,
    }
    .spawn()
    .map_err(|e| anyhow::anyhow!("no system tray: {e}"))?;
    let mut wait = FIRST;
    let mut told = 0;
    loop {
        let _ = rx.recv_timeout(wait);
        wait = EVERY;
        let p = data::pending();
        let count = p.total();
        {
            let mut s = state.lock().unwrap();
            s.count = count;
            s.restart = data::restart_needed();
        }
        handle.update(|_| {});
        if count > 0 && count != told {
            notify(count);
        }
        told = count;
    }
}
