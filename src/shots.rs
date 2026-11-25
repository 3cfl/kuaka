use crate::{apply, data, setup_theme, App, Snapshot};
use slint::ComponentHandle;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

fn up(name: &str, old: &str, new: &str) -> data::Outdated {
    data::Outdated {
        name: name.into(),
        old: old.into(),
        new: new.into(),
    }
}

fn demo(pending: bool) -> Snapshot {
    let pacman = vec![
        up("linux-cachyos", "6.17.8-1", "6.17.9-1"),
        up("systemd", "258.1-1", "258.2-1"),
        up("mesa", "1:25.2.6-1", "1:25.2.7-1"),
        up("fiordland-base", "0.9.1-1", "1.0.0-1"),
        up("firefox", "145.0-1", "145.0.1-1"),
        up("pipewire", "1:1.4.8-1", "1:1.4.9-1"),
        up("kitty", "0.44.0-1", "0.44.1-1"),
        up("python", "3.13.7-1", "3.13.8-1"),
    ];
    let sizes: HashMap<String, u64> = [
        ("linux-cachyos", 151_000_000),
        ("systemd", 9_800_000),
        ("mesa", 31_000_000),
        ("fiordland-base", 120_000),
        ("firefox", 78_000_000),
        ("pipewire", 2_400_000),
        ("kitty", 7_100_000),
        ("python", 26_000_000),
    ]
    .into_iter()
    .map(|(n, s)| (n.to_string(), s))
    .collect();
    let release = data::parse_news(
        "1.0.0",
        "Fiordland 1\n- Restarting and shutting down no longer hang.\n- Faster start: the boot no longer waits for the network.\n- A firewall is on by default.",
    );
    Snapshot {
        pending: data::Pending {
            release: Some("0.9.1".into()),
            pacman: Some(if pending { pacman } else { vec![] }),
            aur: Some(if pending {
                vec![up("visual-studio-code-bin", "1.106.0-1", "1.106.2-1")]
            } else {
                vec![]
            }),
            flatpak: Some(if pending {
                vec![
                    up("com.valvesoftware.Steam", "", "1.0.0.82"),
                    up("org.gnome.Platform", "", "49"),
                ]
            } else {
                vec![]
            }),
            via_tawaki: true,
        },
        sizes,
        restart: data::Restart::default(),
        alerts: if pending {
            vec![
                data::News {
                    kind: "release",
                    link: "https://fiordlandlinux.org/docs/updates/".into(),
                    ..release.clone()
                },
                data::News {
                    kind: "arch",
                    title: "Manual intervention needed for an example package".into(),
                    points: vec!["The example package moved a config file. Back up your changes before updating, then merge them into the new file.".into()],
                    link: "https://archlinux.org/news/".into(),
                    warn: true,
                },
            ]
        } else {
            vec![]
        },
        news: vec![
            release,
            data::parse_news(
                "0.9.1",
                "Fiordland 0.9.1\n- New stable channel: Arch packages come from a tested snapshot that moves forward every two weeks.\n- The installer cleans up when you press Try again.",
            ),
        ],
        history: vec![
            data::Change {
                number: 42,
                date: "2026-11-20 10:00".into(),
                what: "System update".into(),
            },
            data::Change {
                number: 38,
                date: "2026-11-14 18:22".into(),
                what: "Installed obs-studio".into(),
            },
            data::Change {
                number: 31,
                date: "2026-11-02 09:41".into(),
                what: "Removed libreoffice-fresh".into(),
            },
        ],
        snapshot_note: "A Btrfs snapshot is taken right before the system update, so you can undo it from history if something breaks.".into(),
    }
}

const LOG: &str = ":: Synchronizing package databases...
 core downloading...
 extra downloading...
:: Starting full system upgrade...
resolving dependencies...
looking for conflicting packages...
Packages (8) firefox-145.0.1-1 fiordland-base-1.0.0-1 kitty-0.44.1-1 linux-cachyos-6.17.9-1 mesa-1:25.2.7-1 pipewire-1:1.4.9-1 python-3.13.8-1 systemd-258.2-1
:: Retrieving packages...
 linux-cachyos-6.17.9-1-x86_64 downloading...
 mesa-1:25.2.7-1-x86_64 downloading...
:: Running pre-transaction hooks...
==> snapper pre snapshot 43
:: Processing package changes...
upgrading linux-cachyos...
upgrading systemd...
upgrading mesa...";

type Scene = (&'static str, Box<dyn Fn(&App)>);

fn scenes() -> Vec<Scene> {
    vec![
        (
            "uptodate",
            Box::new(|ui: &App| {
                apply(ui, &demo(false), false);
                ui.set_page(0);
            }),
        ),
        (
            "updates",
            Box::new(|ui: &App| {
                apply(ui, &demo(true), false);
                ui.set_page(0);
            }),
        ),
        (
            "updating",
            Box::new(|ui: &App| {
                apply(ui, &demo(true), false);
                ui.set_stage("updating".into());
                ui.set_headline("updating".into());
                ui.set_subline("Keep your PC on. You can keep working while this runs.".into());
                ui.set_step("Updating mesa".into());
                ui.set_progress(0.62);
                ui.set_log_text(LOG.into());
                ui.set_log_open(true);
            }),
        ),
        (
            "reboot",
            Box::new(|ui: &App| {
                let mut s = demo(false);
                s.restart = data::Restart {
                    needed: true,
                    what: vec!["linux-cachyos".into(), "systemd".into(), "mesa".into()],
                };
                apply(ui, &s, false);
                ui.set_stage("done".into());
                ui.set_headline("all done".into());
                ui.set_subline("Everything that was waiting is installed.".into());
                ui.set_step("Finished".into());
                ui.set_progress(1.0);
                ui.set_log_open(false);
            }),
        ),
        (
            "error",
            Box::new(|ui: &App| {
                apply(ui, &demo(true), false);
                ui.set_stage("error".into());
                ui.set_headline("the update stopped".into());
                ui.set_subline(
                    "The details below say what happened. You can try again once it is fixed."
                        .into(),
                );
                ui.set_step("Stopped".into());
                ui.set_progress(0.31);
                ui.set_error(
                    "The update stopped with an error (code 1). The details below say why.".into(),
                );
                ui.set_log_text(format!("{LOG}\nerror: failed to commit transaction (conflicting files)\nexample: /usr/lib/example.so exists in filesystem").into());
                ui.set_log_open(true);
            }),
        ),
        (
            "news",
            Box::new(|ui: &App| {
                apply(ui, &demo(true), false);
                ui.set_page(1);
            }),
        ),
        (
            "history",
            Box::new(|ui: &App| {
                ui.set_page(2);
                ui.set_confirm(42);
            }),
        ),
        (
            "settings",
            Box::new(|ui: &App| {
                ui.set_confirm(-1);
                ui.set_page(3);
            }),
        ),
        (
            "compact",
            Box::new(|ui: &App| {
                apply(ui, &demo(true), false);
                ui.set_page(0);
                ui.window().set_size(slint::LogicalSize::new(700.0, 620.0));
            }),
        ),
    ]
}

fn save(ui: &App, path: &PathBuf) -> anyhow::Result<()> {
    let shot = ui.window().take_snapshot()?;
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), shot.width(), shot.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header()?;
    w.write_image_data(shot.as_bytes())?;
    Ok(())
}

pub fn run(dir: &str, prefix: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    crate::select_backend();
    let ui = App::new()?;
    setup_theme(&ui);
    ui.set_mode_note("Updates come from tawaki.".into());
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.window().set_size(slint::LogicalSize::new(1000.0, 700.0));
    let scenes = Rc::new(scenes());
    let tick = Rc::new(Cell::new(0usize));
    let failed = Rc::new(Cell::new(false));
    let timer = slint::Timer::default();
    {
        let weak = ui.as_weak();
        let dir = PathBuf::from(dir);
        let prefix = prefix.to_string();
        let failed = failed.clone();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(900),
            move || {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                let k = tick.get();
                tick.set(k + 1);
                let i = k / 2;
                if i >= scenes.len() {
                    let _ = slint::quit_event_loop();
                    return;
                }
                if k.is_multiple_of(2) {
                    (scenes[i].1)(&ui);
                    ui.window().request_redraw();
                } else {
                    let path = dir.join(format!("{prefix}-{}.png", scenes[i].0));
                    match save(&ui, &path) {
                        Ok(()) => println!("{}", path.display()),
                        Err(e) => {
                            eprintln!("{}: {e:#}", path.display());
                            failed.set(true);
                        }
                    }
                }
            },
        );
    }
    let watchdog = slint::Timer::default();
    watchdog.start(
        slint::TimerMode::SingleShot,
        Duration::from_secs(60),
        || {
            eprintln!("screenshot run timed out");
            std::process::exit(3);
        },
    );
    ui.run()?;
    if failed.get() {
        anyhow::bail!("some screenshots failed");
    }
    Ok(())
}
