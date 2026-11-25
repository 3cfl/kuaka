use crate::config::{Notified, Settings};
use crate::{data, sys};
use ksni::blocking::TrayMethods;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FIRST: Duration = Duration::from_secs(90);
const RETRY: Duration = Duration::from_secs(3600);

#[derive(Default)]
struct State {
    count: usize,
    restart: bool,
    failed: bool,
}

struct Tray {
    state: Arc<Mutex<State>>,
    wake: std::sync::mpsc::Sender<()>,
}

pub fn open_window() {
    let exe = if sys::has(sys::KUAKA) {
        sys::KUAKA.to_string()
    } else {
        std::env::current_exe()
            .ok()
            .filter(|p| p.is_file())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    if !exe.is_empty() {
        sys::spawn_detached(&exe, std::iter::empty::<&str>());
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "kuaka".into()
    }

    fn title(&self) -> String {
        "Kuaka".into()
    }

    fn icon_name(&self) -> String {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.count > 0 || s.restart {
            "system-software-update".into()
        } else {
            "update-none".into()
        }
    }

    fn status(&self) -> ksni::Status {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.count > 0 || s.restart {
            ksni::Status::Active
        } else {
            ksni::Status::Passive
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let text = if s.restart {
            "Restart to finish updating".to_string()
        } else if s.failed {
            "Couldn't check for updates".to_string()
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

static SHOWING: AtomicBool = AtomicBool::new(false);

fn notify(count: usize) {
    if SHOWING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let shown = notify_rust::Notification::new()
            .appname("Kuaka")
            .summary(&plural(count))
            .body("Open Kuaka to see what's new and update.")
            .icon("system-software-update")
            .timeout(notify_rust::Timeout::Milliseconds(15_000))
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
        SHOWING.store(false, Ordering::SeqCst);
    });
}

pub fn paused(settings: &Settings) -> bool {
    (settings.pause_on_battery && sys::on_battery())
        || (settings.pause_on_metered && sys::metered())
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
    loop {
        let forced = match rx.recv_timeout(wait) {
            Ok(()) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let settings = Settings::load();
        let every = Duration::from_secs(u64::from(settings.interval_hours) * 3600);
        if !forced && paused(&settings) {
            wait = RETRY.min(every);
            continue;
        }
        let p = data::pending();
        let restart = data::restart_needed().needed;
        {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            s.count = p.total();
            s.restart = restart;
            s.failed = p.failed();
        }
        handle.update(|_| {});
        if settings.notify && !p.failed() {
            let current = p.fingerprints();
            let mut seen = Notified::load();
            let now = sys::now();
            if seen.should_notify(&current, now) {
                notify(p.total());
                seen.at = now;
                seen.items = current;
                let _ = seen.save();
            }
        }
        wait = if p.failed() { RETRY.min(every) } else { every };
    }
}
