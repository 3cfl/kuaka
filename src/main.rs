mod data;
mod tray;
mod update;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

slint::include_modules!();

fn pkgs(list: &[data::Outdated]) -> ModelRc<Pkg> {
    ModelRc::new(VecModel::from(
        list.iter()
            .map(|o| Pkg {
                name: o.name.clone().into(),
                old: o.old.clone().into(),
                new: o.new.clone().into(),
            })
            .collect::<Vec<_>>(),
    ))
}

fn groups(p: &data::Pending) -> Vec<Group> {
    let mut out = Vec::new();
    let mut add = |kind: &str, title: &str, note: String, list: &Option<Vec<data::Outdated>>| {
        if let Some(l) = list.as_ref().filter(|l| !l.is_empty()) {
            out.push(Group {
                kind: kind.into(),
                title: format!("{title} · {}", l.len()).into(),
                note: note.into(),
                items: pkgs(l),
            });
        }
    };
    add(
        "pacman",
        "System",
        "Fiordland, Arch and CachyOS packages. A snapshot is taken first, so this can be undone.".into(),
        &p.pacman,
    );
    add(
        "flatpak",
        "Apps",
        "Flatpak apps and runtimes from Flathub.".into(),
        &p.flatpak,
    );
    add(
        "aur",
        "AUR",
        "Built from user-made scripts. Update these in a terminal, where you can review each build script first.".into(),
        &p.aur,
    );
    out
}

fn headline(p: &data::Pending) -> (String, String) {
    if p.failed() {
        return (
            "Couldn't check for updates".into(),
            "Check your internet connection and try again.".into(),
        );
    }
    let n = p.pacman.as_ref().map_or(0, Vec::len) + p.flatpak.as_ref().map_or(0, Vec::len);
    let aur = p.aur.as_ref().map_or(0, Vec::len);
    match (n, aur) {
        (0, 0) => ("Fiordland is up to date".into(), "Nothing to install right now.".into()),
        (0, a) => (
            tray::plural(a),
            "Only AUR packages have updates. Update them in a terminal.".into(),
        ),
        (n, _) => (
            tray::plural(n + aur),
            "Close games and running apps first. Most updates take a few minutes.".into(),
        ),
    }
}

fn check(ui: Weak<App>) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        ui.set_checking(true);
        ui.set_headline("Checking for updates".into());
        ui.set_subline("".into());
    });
    std::thread::spawn(move || {
        let p = data::pending();
        let news = data::news(&p);
        let restart = data::restart_needed();
        let history = data::history();
        let release = p.release.clone().filter(|r| !r.is_empty()).unwrap_or_else(data::release);
        let _ = ui.upgrade_in_event_loop(move |ui| {
            let (h, s) = headline(&p);
            ui.set_headline(h.into());
            ui.set_subline(s.into());
            ui.set_release(release.into());
            ui.set_restart_needed(restart);
            ui.set_can_update(
                p.pacman.as_ref().is_some_and(|l| !l.is_empty())
                    || p.flatpak.as_ref().is_some_and(|l| !l.is_empty()),
            );
            ui.set_groups(ModelRc::new(VecModel::from(groups(&p))));
            ui.set_news(ModelRc::new(VecModel::from(
                news.into_iter()
                    .map(|n| NewsCard {
                        title: n.title.into(),
                        body: n.body.into(),
                        link: n.link.into(),
                        warn: n.warn,
                    })
                    .collect::<Vec<_>>(),
            )));
            set_history(&ui, history);
            ui.set_checking(false);
        });
    });
}

fn set_history(ui: &App, history: Vec<data::Change>) {
    ui.set_history_note(
        if history.is_empty() {
            "No snapshots yet. One is taken before every update and every app you install or remove."
        } else {
            "A snapshot is taken before every change to the system. Undo puts the system back to how it was before that change."
        }
        .into(),
    );
    ui.set_history(ModelRc::new(VecModel::from(
        history
            .into_iter()
            .map(|c| Change {
                number: c.number,
                date: c.date.into(),
                what: c.what.into(),
            })
            .collect::<Vec<_>>(),
    )));
}

fn runner(ui: Weak<App>, job: impl FnOnce(&dyn Fn(update::Event)) -> anyhow::Result<()> + Send + 'static, done: &'static str) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        ui.set_busy(true);
        ui.set_error("".into());
        ui.set_progress(0.0);
        ui.set_log_text("".into());
    });
    std::thread::spawn(move || {
        let lines = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let on = {
            let ui = ui.clone();
            let lines = lines.clone();
            move |e: update::Event| {
                let ui = ui.clone();
                match e {
                    update::Event::Status(s) => {
                        let _ = ui.upgrade_in_event_loop(move |ui| ui.set_step(SharedString::from(s)));
                    }
                    update::Event::Progress(p) => {
                        let _ = ui.upgrade_in_event_loop(move |ui| ui.set_progress(p));
                    }
                    update::Event::Log(l) => {
                        let text = {
                            let mut q = lines.lock().unwrap();
                            q.push_back(l);
                            while q.len() > 12 {
                                q.pop_front();
                            }
                            q.iter().cloned().collect::<Vec<_>>().join("\n")
                        };
                        let _ = ui.upgrade_in_event_loop(move |ui| ui.set_log_text(text.into()));
                    }
                }
            }
        };
        let result = job(&on);
        let ui2 = ui.clone();
        let _ = ui.upgrade_in_event_loop(move |ui| {
            ui.set_busy(false);
            match result {
                Ok(()) => ui.set_step(done.into()),
                Err(e) => {
                    ui.set_step("Stopped".into());
                    ui.set_error(format!("{e:#}").into());
                }
            }
            ui.set_confirm(-1);
            check(ui2);
        });
    });
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--tray") => return tray::run(),
        Some("--check") => {
            let p = data::pending();
            println!("{}", if p.failed() { "couldn't check".into() } else { tray::plural(p.total()) });
            return Ok(());
        }
        Some("--help") | Some("-h") => {
            println!("usage: kuaka [--tray | --check]");
            return Ok(());
        }
        _ => {}
    }

    let _ = slint::set_xdg_app_id("kuaka");
    let ui = App::new()?;
    let weak = ui.as_weak();

    ui.on_check({
        let w = weak.clone();
        move || check(w.clone())
    });
    ui.on_update({
        let w = weak.clone();
        move || {
            let Some(app) = w.upgrade() else { return };
            let groups = app.get_groups();
            let has = |kind: &str| {
                (0..slint::Model::row_count(&groups))
                    .filter_map(|i| slint::Model::row_data(&groups, i))
                    .any(|g| g.kind == kind)
            };
            let (system, apps) = (has("pacman"), has("flatpak"));
            runner(w.clone(), move |on| update::run(system, apps, on), "Updated");
        }
    });
    ui.on_aur(|| {
        let _ = update::terminal(&["tawaki", "update"]);
    });
    ui.on_undo({
        let w = weak.clone();
        move |n| runner(w.clone(), move |on| update::rollback(n, on), "Restored. Restart to use it.")
    });
    ui.on_restart(|| {
        let _ = std::process::Command::new("systemctl").arg("reboot").spawn();
    });
    ui.on_open(|url| {
        let _ = std::process::Command::new("xdg-open").arg(url.as_str()).spawn();
    });

    check(weak);
    ui.run()?;
    Ok(())
}
