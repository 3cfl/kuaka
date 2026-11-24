use anyhow::{bail, Result};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

pub const HELPER: &str = "/usr/lib/kuaka/kuaka-helper";

pub enum Event {
    Status(String),
    Progress(f32),
    Log(String),
}

fn stream(
    mut cmd: Command,
    on: &dyn Fn(Event),
    parse: &dyn Fn(&str) -> Option<(String, f32)>,
) -> Result<()> {
    let mut child = cmd
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let line = line.trim_end().to_string();
            if line.is_empty() {
                continue;
            }
            if let Some((s, p)) = parse(&line) {
                on(Event::Status(s));
                on(Event::Progress(p));
            }
            on(Event::Log(line));
        }
    }
    let status = child.wait()?;
    match status.code() {
        Some(0) => Ok(()),
        Some(126) => bail!("Cancelled: the password prompt was closed"),
        Some(127) => bail!("Not allowed: your account needs administrator rights"),
        Some(c) => bail!("Stopped with an error (code {c})"),
        None => bail!("Stopped unexpectedly"),
    }
}

fn pacman_line(line: &str) -> Option<(String, f32)> {
    let rest = line.strip_prefix('(')?;
    let (count, tail) = rest.split_once(')')?;
    let (a, b) = count.split_once('/')?;
    let (a, b): (f32, f32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    let mut words = tail.split_whitespace();
    let verb = words.next()?;
    let name = words.next().unwrap_or("");
    let (label, base, span) = match verb {
        "upgrading" => ("Updating", 0.25, 0.6),
        "installing" => ("Installing", 0.25, 0.6),
        "reinstalling" => ("Reinstalling", 0.25, 0.6),
        "removing" => ("Removing", 0.25, 0.6),
        "checking" | "loading" => return None,
        _ => ("Preparing", 0.05, 0.2),
    };
    let label = if name.is_empty() {
        label.to_string()
    } else {
        format!("{label} {name}")
    };
    Some((label, base + span * a / b.max(1.0)))
}

pub fn run(system: bool, flatpak: bool, on: &dyn Fn(Event)) -> Result<()> {
    if system {
        on(Event::Status("Waiting for your password".into()));
        on(Event::Progress(0.02));
        let mut c = Command::new("pkexec");
        c.args([HELPER, "upgrade"]);
        stream(c, on, &pacman_line)?;
    }
    if flatpak {
        on(Event::Status("Updating apps".into()));
        on(Event::Progress(0.88));
        let mut c = Command::new("flatpak");
        c.args(["update", "-y", "--noninteractive"]);
        stream(c, on, &|_| None)?;
    }
    on(Event::Progress(1.0));
    Ok(())
}

pub fn rollback(number: i32, on: &dyn Fn(Event)) -> Result<()> {
    on(Event::Status("Waiting for your password".into()));
    let mut c = Command::new("pkexec");
    c.args([HELPER, "rollback", &number.to_string()]);
    stream(c, on, &|_| None)
}

pub fn terminal(cmd: &[&str]) -> Result<()> {
    let mut tries: Vec<(String, Vec<String>)> = Vec::new();
    if let Ok(t) = std::env::var("TERMINAL") {
        tries.push((t, vec!["-e".into()]));
    }
    for (t, flag) in [
        ("xdg-terminal-exec", ""),
        ("konsole", "-e"),
        ("gnome-terminal", "--"),
        ("kgx", "--"),
        ("ptyxis", "--"),
        ("kitty", ""),
        ("alacritty", "-e"),
        ("foot", ""),
        ("xfce4-terminal", "-x"),
        ("xterm", "-e"),
    ] {
        tries.push((
            t.into(),
            if flag.is_empty() {
                vec![]
            } else {
                vec![flag.into()]
            },
        ));
    }
    for (t, flag) in tries {
        let ok = Command::new(&t)
            .args(&flag)
            .args(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok();
        if ok {
            return Ok(());
        }
    }
    bail!("No terminal found")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress() {
        let (s, p) = pacman_line("( 3/10) upgrading linux-firmware").unwrap();
        assert_eq!(s, "Updating linux-firmware");
        assert!((p - 0.43).abs() < 0.01);
        assert!(pacman_line(":: Synchronizing package databases...").is_none());
    }
}
