mod config;
mod data;
#[cfg(feature = "shots")]
mod shots;
mod sys;
mod tray;
mod update;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

slint::include_modules!();

const DONATE: &str = "https://donate.stripe.com/8x25kFfe96fn0WL6X12wU00";
const DISCORD: &str = "https://discord.gg/gy4ANH2Q9V";
const LOG_LINES: usize = 400;

pub struct Snapshot {
    pub pending: data::Pending,
    pub sizes: std::collections::HashMap<String, u64>,
    pub restart: data::Restart,
    pub alerts: Vec<data::News>,
    pub news: Vec<data::News>,
    pub history: Vec<data::Change>,
    pub snapshot_note: String,
}

fn gather() -> Snapshot {
    let pending = data::pending();
    let (sizes, alerts, restart, news, history, snapshot_note) = std::thread::scope(|s| {
        let sizes = s.spawn(|| data::sizes(pending.pacman.as_deref().unwrap_or(&[])));
        let alerts = s.spawn(|| data::alerts(&pending));
        let restart = data::restart_needed();
        let news = data::local_news();
        let history = data::history();
        let note = data::snapshot_note();
        (
            sizes.join().unwrap_or_default(),
            alerts.join().unwrap_or_default(),
            restart,
            news,
            history,
            note,
        )
    });
    Snapshot {
        pending,
        sizes,
        restart,
        alerts,
        news,
        history,
        snapshot_note,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn pkgs(list: &[data::Outdated], sizes: &std::collections::HashMap<String, u64>) -> ModelRc<Pkg> {
    ModelRc::new(VecModel::from(
        list.iter()
            .map(|o| Pkg {
                name: o.name.as_str().into(),
                old: o.old.as_str().into(),
                new: o.new.as_str().into(),
                size: sizes
                    .get(&o.name)
                    .map(|b| data::human_size(*b))
                    .unwrap_or_default()
                    .into(),
                core: data::is_core(&o.name),
            })
            .collect::<Vec<_>>(),
    ))
}

pub fn groups(s: &Snapshot) -> Vec<Group> {
    let p = &s.pending;
    let mut out = Vec::new();
    if let Some(l) = p.pacman.as_ref().filter(|l| !l.is_empty()) {
        let total: u64 = l.iter().filter_map(|o| s.sizes.get(&o.name)).sum();
        let mut summary = plural(l.len(), "update", "updates");
        if total > 0 {
            summary.push_str(&format!("  ·  {} to download", data::human_size(total)));
        }
        out.push(Group {
            kind: "pacman".into(),
            title: "system".into(),
            icon: "deployed_code".into(),
            summary: summary.into(),
            note: "Fiordland, Arch and CachyOS packages, checked and signed by their repositories."
                .into(),
            restart: l.iter().any(|o| data::is_core(&o.name)),
            items: pkgs(l, &s.sizes),
        });
    }
    if let Some(l) = p.flatpak.as_ref().filter(|l| !l.is_empty()) {
        out.push(Group {
            kind: "flatpak".into(),
            title: "apps".into(),
            icon: "apps".into(),
            summary: plural(l.len(), "update", "updates").into(),
            note: "Flatpak apps and runtimes from Flathub.".into(),
            restart: false,
            items: pkgs(l, &s.sizes),
        });
    }
    if let Some(l) = p.aur.as_ref().filter(|l| !l.is_empty()) {
        out.push(Group {
            kind: "aur".into(),
            title: "aur".into(),
            icon: "terminal".into(),
            summary: plural(l.len(), "update", "updates").into(),
            note: "Built on this PC from scripts other users wrote. They update in a terminal, where you can read each build script first.".into(),
            restart: false,
            items: pkgs(l, &s.sizes),
        });
    }
    out
}

pub fn headline(s: &Snapshot) -> (&'static str, String, String) {
    let p = &s.pending;
    if p.failed() {
        return (
            "offline",
            "couldn't check for updates".into(),
            "Check your internet connection and try again.".into(),
        );
    }
    let n = p.installable();
    let aur = p.aur.as_ref().map_or(0, Vec::len);
    let total: u64 = p
        .pacman
        .iter()
        .flatten()
        .filter_map(|o| s.sizes.get(&o.name))
        .sum();
    match (n, aur) {
        (0, 0) => (
            "uptodate",
            "you're up to date".into(),
            "Nothing to install right now.".into(),
        ),
        (0, a) => (
            "updates",
            format!("{} ready", plural(a, "aur update", "aur updates")),
            "Only AUR packages have updates. They update in a terminal.".into(),
        ),
        (n, _) => (
            "updates",
            format!("{} ready", plural(n + aur, "update", "updates")),
            if total > 0 {
                format!(
                    "{} to download. Close games and big apps first; most updates take a few minutes.",
                    data::human_size(total)
                )
            } else {
                "Close games and big apps first. Most updates take a few minutes.".into()
            },
        ),
    }
}

fn news_cards(list: &[data::News]) -> ModelRc<NewsCard> {
    ModelRc::new(VecModel::from(
        list.iter()
            .map(|n| NewsCard {
                kind: n.kind.into(),
                title: n.title.as_str().into(),
                points: ModelRc::new(VecModel::from(
                    n.points
                        .iter()
                        .map(|p| SharedString::from(p.as_str()))
                        .collect::<Vec<_>>(),
                )),
                link: n.link.as_str().into(),
                warn: n.warn,
            })
            .collect::<Vec<_>>(),
    ))
}

pub fn apply(ui: &App, s: &Snapshot, keep_stage: bool) {
    let (stage, h, sub) = headline(s);
    if !keep_stage {
        ui.set_stage(stage.into());
        ui.set_headline(h.into());
        ui.set_subline(sub.into());
    }
    let release = s
        .pending
        .release
        .clone()
        .filter(|r| !r.is_empty())
        .unwrap_or_else(data::release);
    ui.set_release(release.into());
    ui.set_restart_needed(s.restart.needed);
    ui.set_restart_reason(s.restart.reason().into());
    ui.set_snapshot_note(s.snapshot_note.as_str().into());
    ui.set_can_update(s.pending.installable() > 0);
    ui.set_has_aur(s.pending.aur.as_ref().is_some_and(|l| !l.is_empty()) && sys::has(sys::TAWAKI));
    ui.set_pending_count(s.pending.total().min(999) as i32);
    ui.set_groups(ModelRc::new(VecModel::from(groups(s))));
    ui.set_alerts(news_cards(&s.alerts));
    ui.set_news(news_cards(&s.news));
    ui.set_source_note(
        if s.pending.via_tawaki {
            ""
        } else {
            "tawaki isn't installed, so kuaka checks pacman and Flatpak directly and can't see AUR updates."
        }
        .into(),
    );
    ui.set_history_note(
        if s.history.is_empty() {
            "No snapshots yet. With snapper and snap-pac, one is taken before every update and every app you install or remove."
        } else {
            "A snapshot is taken before every change to the system. Undo puts the system back to how it was before that change."
        }
        .into(),
    );
    ui.set_history(ModelRc::new(VecModel::from(
        s.history
            .iter()
            .map(|c| Change {
                number: c.number,
                date: c.date.as_str().into(),
                what: c.what.as_str().into(),
            })
            .collect::<Vec<_>>(),
    )));
}

fn check(ui: Weak<App>, shared: Arc<Mutex<Option<data::Pending>>>, keep_stage: bool) {
    if !keep_stage {
        let _ = ui.upgrade_in_event_loop(|ui| {
            ui.set_stage("checking".into());
            ui.set_headline("checking for updates".into());
            ui.set_subline("This takes a few seconds.".into());
        });
    }
    std::thread::spawn(move || {
        let snap = gather();
        *shared.lock().unwrap_or_else(|e| e.into_inner()) = Some(snap.pending.clone());
        let _ = ui.upgrade_in_event_loop(move |ui| apply(&ui, &snap, keep_stage));
    });
}

struct LogState {
    lines: VecDeque<String>,
    last: Instant,
}

fn log_text(lines: &VecDeque<String>) -> String {
    lines.iter().cloned().collect::<Vec<_>>().join("\n")
}

fn runner(
    ui: Weak<App>,
    shared: Arc<Mutex<Option<data::Pending>>>,
    job: impl FnOnce(&dyn Fn(update::Event)) -> anyhow::Result<()> + Send + 'static,
    done: (&'static str, &'static str),
) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        ui.set_stage("updating".into());
        ui.set_headline("updating".into());
        ui.set_subline("Keep your PC on. You can keep working while this runs.".into());
        ui.set_error("".into());
        ui.set_progress(0.0);
        ui.set_step("Starting".into());
        ui.set_log_text("".into());
    });
    std::thread::spawn(move || {
        let log = Arc::new(Mutex::new(LogState {
            lines: VecDeque::new(),
            last: Instant::now() - Duration::from_secs(1),
        }));
        let on = {
            let ui = ui.clone();
            let log = log.clone();
            move |e: update::Event| match e {
                update::Event::Status(s) => {
                    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_step(SharedString::from(s)));
                }
                update::Event::Progress(p) => {
                    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_progress(p));
                }
                update::Event::Log(l) => {
                    let text = {
                        let mut st = log.lock().unwrap_or_else(|e| e.into_inner());
                        st.lines.push_back(l);
                        while st.lines.len() > LOG_LINES {
                            st.lines.pop_front();
                        }
                        if st.last.elapsed() < Duration::from_millis(120) {
                            None
                        } else {
                            st.last = Instant::now();
                            Some(log_text(&st.lines))
                        }
                    };
                    if let Some(text) = text {
                        let _ = ui.upgrade_in_event_loop(move |ui| ui.set_log_text(text.into()));
                    }
                }
            }
        };
        let result = job(&on);
        let text = log_text(&log.lock().unwrap_or_else(|e| e.into_inner()).lines);
        let ui2 = ui.clone();
        let _ = ui.upgrade_in_event_loop(move |ui| {
            ui.set_log_text(text.into());
            match result {
                Ok(()) => {
                    ui.set_stage("done".into());
                    ui.set_headline(done.0.into());
                    ui.set_subline(done.1.into());
                    ui.set_step("Finished".into());
                    ui.set_progress(1.0);
                }
                Err(e) => {
                    ui.set_stage("error".into());
                    ui.set_headline("the update stopped".into());
                    ui.set_subline(
                        "The details below say what happened. You can try again once it is fixed."
                            .into(),
                    );
                    ui.set_step("Stopped".into());
                    ui.set_error(format!("{e:#}").into());
                    ui.set_log_open(true);
                }
            }
            ui.set_confirm(-1);
            check(ui2, shared, true);
        });
    });
}

fn mode_note(low: bool, reduced: bool, symbols: bool) -> String {
    let mut parts = vec![if sys::has(sys::TAWAKI) {
        "Updates come from tawaki."
    } else {
        "tawaki isn't installed, so pacman and Flatpak are used directly."
    }
    .to_string()];
    if low {
        parts.push("Lightweight mode is on: no translucency, shadows or motion.".into());
    } else if reduced {
        parts.push("Reduced motion is on.".into());
    }
    if !symbols {
        parts.push("Install ttf-material-symbols-variable for the full icon set.".into());
    }
    parts.join(" ")
}

pub fn setup_theme(ui: &App) -> (bool, bool, bool) {
    let low = sys::low_end();
    let reduced = sys::reduce_motion(low);
    let symbols = sys::symbols_font();
    let theme = ui.global::<Theme>();
    theme.set_mode(sys::theme_mode());
    theme.set_low_end(low);
    theme.set_reduce_motion(reduced);
    theme.set_symbols(symbols);
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_mode_note(mode_note(low, reduced, symbols).into());
    (low, reduced, symbols)
}

pub fn select_backend() {
    if std::env::var_os("SLINT_BACKEND").is_some() {
        return;
    }
    if sys::low_end() {
        let _ = slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select();
    }
}

fn load_settings(ui: &App) {
    let s = config::Settings::load();
    ui.set_interval_hours(s.interval_hours as i32);
    ui.set_notify(s.notify);
    ui.set_pause_battery(s.pause_on_battery);
    ui.set_pause_metered(s.pause_on_metered);
}

fn save_settings(ui: &App) {
    let s = config::Settings {
        interval_hours: ui.get_interval_hours().clamp(1, 168) as u32,
        notify: ui.get_notify(),
        pause_on_battery: ui.get_pause_battery(),
        pause_on_metered: ui.get_pause_metered(),
    };
    let _ = s.save();
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--tray") => return tray::run(),
        Some("--check") => {
            let p = data::pending();
            println!(
                "{}",
                if p.failed() {
                    "couldn't check".into()
                } else {
                    tray::plural(p.total())
                }
            );
            return Ok(());
        }
        Some("--version") | Some("-V") => {
            println!("kuaka {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        #[cfg(feature = "shots")]
        Some("--screenshot") => {
            let dir = args.get(1).cloned().unwrap_or_else(|| "shots".into());
            let prefix = args.get(2).cloned().unwrap_or_else(|| "shot".into());
            return shots::run(&dir, &prefix);
        }
        Some("--help") | Some("-h") => {
            println!("usage: kuaka [--tray | --check | --version]");
            return Ok(());
        }
        Some(other) => {
            eprintln!("kuaka: unknown option {other}");
            std::process::exit(2);
        }
        None => {}
    }

    select_backend();
    let ui = App::new()?;
    let _ = slint::set_xdg_app_id("kuaka");
    setup_theme(&ui);
    load_settings(&ui);
    let weak = ui.as_weak();
    let shared: Arc<Mutex<Option<data::Pending>>> = Arc::new(Mutex::new(None));

    ui.on_check({
        let (w, sh) = (weak.clone(), shared.clone());
        move || check(w.clone(), sh.clone(), false)
    });
    ui.on_update({
        let (w, sh) = (weak.clone(), shared.clone());
        move || {
            let Some(p) = sh.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
                return;
            };
            let system = p.pacman.as_ref().is_some_and(|l| !l.is_empty());
            let apps = p.flatpak.as_ref().is_some_and(|l| !l.is_empty());
            let expected = p.pacman.as_ref().map_or(0, Vec::len);
            if !system && !apps {
                return;
            }
            runner(
                w.clone(),
                sh.clone(),
                move |on| update::run(system, apps, expected, on),
                ("all done", "Everything that was waiting is installed."),
            );
        }
    });
    ui.on_aur({
        let w = weak.clone();
        move || {
            if let Err(e) = update::terminal_update() {
                if let Some(ui) = w.upgrade() {
                    ui.set_error(format!("{e:#}").into());
                }
            }
        }
    });
    ui.on_undo({
        let (w, sh) = (weak.clone(), shared.clone());
        move |n| {
            if let Some(ui) = w.upgrade() {
                ui.set_page(0);
            }
            runner(
                w.clone(),
                sh.clone(),
                move |on| update::rollback(n, on),
                ("restored", "Restart to use the restored system."),
            )
        }
    });
    ui.on_restart(|| {
        update::reboot();
    });
    ui.on_open(|url| {
        update::open_url(url.as_str());
    });
    ui.on_donate(|| {
        update::open_url(DONATE);
    });
    ui.on_discord(|| {
        update::open_url(DISCORD);
    });
    ui.on_settings_changed({
        let w = weak.clone();
        move || {
            if let Some(ui) = w.upgrade() {
                save_settings(&ui);
            }
        }
    });

    check(weak, shared, false);
    ui.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(pacman: Option<Vec<data::Outdated>>, aur: Option<Vec<data::Outdated>>) -> Snapshot {
        Snapshot {
            pending: data::Pending {
                release: Some("1.0.0".into()),
                pacman,
                aur,
                flatpak: Some(vec![]),
                via_tawaki: true,
            },
            sizes: Default::default(),
            restart: Default::default(),
            alerts: vec![],
            news: vec![],
            history: vec![],
            snapshot_note: String::new(),
        }
    }

    fn up(n: &str) -> data::Outdated {
        data::Outdated {
            name: n.into(),
            old: "1".into(),
            new: "2".into(),
        }
    }

    #[test]
    fn headlines() {
        assert_eq!(headline(&snap(None, None)).0, "offline");
        assert_eq!(headline(&snap(Some(vec![]), Some(vec![]))).0, "uptodate");
        let (stage, h, _) = headline(&snap(Some(vec![up("a"), up("b")]), Some(vec![up("c")])));
        assert_eq!(stage, "updates");
        assert_eq!(h, "3 updates ready");
        let (_, h, _) = headline(&snap(Some(vec![]), Some(vec![up("c")])));
        assert_eq!(h, "1 aur update ready");
    }

    #[test]
    fn groups_flag_restart() {
        let s = snap(Some(vec![up("linux"), up("firefox")]), None);
        let g = groups(&s);
        assert_eq!(g.len(), 1);
        assert!(g[0].restart);
        assert_eq!(g[0].summary.as_str(), "2 updates");
    }
}
