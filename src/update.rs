use crate::sys;
use anyhow::{bail, Result};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;

pub enum Event {
    Status(String),
    Progress(f32),
    Log(String),
}

#[derive(Default)]
pub struct PacmanProgress {
    pub total: usize,
    downloaded: usize,
    applied: usize,
}

impl PacmanProgress {
    pub fn new(total: usize) -> Self {
        PacmanProgress {
            total: total.max(1),
            ..Default::default()
        }
    }

    pub fn feed(&mut self, line: &str) -> Option<(String, f32)> {
        let t = line.trim();
        if t.starts_with(":: Synchronizing") {
            return Some(("Refreshing package lists".into(), 0.04));
        }
        if t.starts_with(":: Retrieving packages") {
            return Some(("Downloading".into(), 0.08));
        }
        if t.starts_with(":: Running pre-transaction hooks") {
            return Some(("Taking a snapshot".into(), 0.45));
        }
        if t.starts_with(":: Processing package changes") {
            return Some(("Installing".into(), 0.5));
        }
        if t.starts_with(":: Running post-transaction hooks") {
            return Some(("Finishing up".into(), 0.92));
        }
        if let Some(rest) = t.strip_prefix('(') {
            let (count, tail) = rest.split_once(')')?;
            let (a, b) = count.split_once('/')?;
            let (a, b): (f32, f32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            let mut words = tail.split_whitespace();
            let verb = words.next()?;
            let name = words.next().unwrap_or("").trim_end_matches("...");
            let label = match verb {
                "upgrading" => "Updating",
                "installing" => "Installing",
                "reinstalling" => "Reinstalling",
                "downgrading" => "Downgrading",
                "removing" => "Removing",
                _ => return None,
            };
            return Some((join(label, name), 0.5 + 0.42 * a / b.max(1.0)));
        }
        let mut words = t.split_whitespace();
        let first = words.next()?;
        let second = words.next().unwrap_or("");
        if second == "downloading..." {
            self.downloaded += 1;
            let f = (self.downloaded as f32 / self.total as f32).min(1.0);
            return Some((join("Downloading", first), 0.08 + 0.34 * f));
        }
        let label = match first {
            "upgrading" => "Updating",
            "installing" => "Installing",
            "reinstalling" => "Reinstalling",
            "downgrading" => "Downgrading",
            "removing" => "Removing",
            _ => return None,
        };
        if !second.ends_with("...") {
            return None;
        }
        self.applied += 1;
        let f = (self.applied as f32 / self.total as f32).min(1.0);
        Some((join(label, second.trim_end_matches("...")), 0.5 + 0.42 * f))
    }
}

fn join(label: &str, name: &str) -> String {
    if name.is_empty() {
        label.to_string()
    } else {
        format!("{label} {name}")
    }
}

fn stream(
    mut cmd: Command,
    on: &dyn Fn(Event),
    mut parse: impl FnMut(&str) -> Option<(String, f32)>,
) -> Result<i32> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || pump(out, tx)));
    }
    if let Some(err) = child.stderr.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || pump(err, tx)));
    }
    drop(tx);
    for line in rx {
        let line = sys::clean_line(&line);
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some((s, p)) = parse(line) {
            on(Event::Status(s));
            on(Event::Progress(p));
        }
        on(Event::Log(line.to_string()));
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait()?;
    Ok(status.code().unwrap_or(-1))
}

fn pump(r: impl Read, tx: mpsc::Sender<String>) {
    let reader = BufReader::new(r.take(64 << 20));
    for line in reader.split(b'\n').map_while(Result::ok) {
        for part in line.split(|b| *b == b'\r') {
            let s = String::from_utf8_lossy(part);
            if tx.send(s.chars().take(2000).collect()).is_err() {
                return;
            }
        }
    }
}

fn pkexec_result(code: i32) -> Result<()> {
    match code {
        0 => Ok(()),
        126 => bail!("Cancelled: the password prompt was closed."),
        127 => bail!("Not allowed: your account needs administrator rights."),
        2 => bail!("The update helper refused the request."),
        -1 => bail!("The update stopped unexpectedly."),
        c => bail!("The update stopped with an error (code {c}). The details below say why."),
    }
}

fn helper(args: &[&str]) -> Result<Command> {
    let allowed = matches!(args, ["upgrade"])
        || matches!(args, ["rollback", n] if n.len() <= 9 && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if !allowed {
        bail!("refusing to run the helper with these arguments");
    }
    if !sys::has(sys::PKEXEC) {
        bail!("polkit (pkexec) isn't installed, so kuaka can't ask for your password.");
    }
    if !sys::has(sys::HELPER) {
        bail!("The kuaka helper is missing. Reinstall kuaka.");
    }
    let mut c = sys::command(sys::PKEXEC);
    c.arg("--disable-internal-agent")
        .arg(sys::HELPER)
        .args(args);
    Ok(c)
}

pub fn run(system: bool, flatpak: bool, expected: usize, on: &dyn Fn(Event)) -> Result<()> {
    if system {
        on(Event::Status("Waiting for your password".into()));
        on(Event::Progress(0.02));
        let mut progress = PacmanProgress::new(expected);
        let code = stream(helper(&["upgrade"])?, on, |l| progress.feed(l))?;
        pkexec_result(code)?;
    }
    if flatpak && sys::has(sys::FLATPAK) {
        on(Event::Status("Updating apps".into()));
        on(Event::Progress(if system { 0.95 } else { 0.1 }));
        let mut c = sys::command(sys::FLATPAK);
        c.args(["update", "-y", "--noninteractive"]);
        match stream(c, on, |_| None)? {
            0 => {}
            c => bail!("Flatpak stopped with an error (code {c}). The details below say why."),
        }
    }
    on(Event::Progress(1.0));
    Ok(())
}

pub fn rollback(number: i32, on: &dyn Fn(Event)) -> Result<()> {
    if number <= 0 {
        bail!("not a snapshot number");
    }
    on(Event::Status("Waiting for your password".into()));
    on(Event::Progress(0.05));
    let n = number.to_string();
    let code = stream(helper(&["rollback", &n])?, on, |_| None)?;
    pkexec_result(code)?;
    on(Event::Progress(1.0));
    Ok(())
}

const TERMINALS: [(&str, &str); 10] = [
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
];

pub fn terminal_update() -> Result<()> {
    if !sys::has(sys::TAWAKI) {
        bail!("tawaki isn't installed");
    }
    let cmd = [sys::TAWAKI, "update"];
    let user = std::env::var("TERMINAL")
        .ok()
        .filter(|t| !t.is_empty() && !t.contains(char::is_whitespace) && !t.starts_with('-'));
    let mut tries: Vec<(String, &str)> = Vec::new();
    if let Some(t) = user {
        tries.push((t, "-e"));
    }
    for (t, flag) in TERMINALS {
        let path = format!("/usr/bin/{t}");
        if sys::has(&path) {
            tries.push((path, flag));
        }
    }
    for (t, flag) in tries {
        let mut args: Vec<&str> = Vec::new();
        if !flag.is_empty() {
            args.push(flag);
        }
        args.extend(cmd);
        if sys::spawn_detached(&t, &args) {
            return Ok(());
        }
    }
    bail!("No terminal found")
}

pub fn reboot() -> bool {
    sys::spawn_detached(sys::SYSTEMCTL, ["reboot"])
}

pub fn open_url(url: &str) -> bool {
    sys::https_url(url) && sys::spawn_detached(sys::XDG_OPEN, [url])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counted_lines() {
        let mut p = PacmanProgress::new(10);
        let (s, v) = p.feed("( 3/10) upgrading linux-firmware").unwrap();
        assert_eq!(s, "Updating linux-firmware");
        assert!((v - (0.5 + 0.42 * 0.3)).abs() < 0.01);
        assert!(p.feed("random text").is_none());
    }

    #[test]
    fn plain_lines() {
        let mut p = PacmanProgress::new(4);
        assert_eq!(
            p.feed(":: Synchronizing package databases...").unwrap().0,
            "Refreshing package lists"
        );
        let (s, _) = p.feed(" linux-6.17.2-1-x86_64 downloading...").unwrap();
        assert_eq!(s, "Downloading linux-6.17.2-1-x86_64");
        let (s, v) = p.feed("upgrading linux...").unwrap();
        assert_eq!(s, "Updating linux");
        assert!((v - (0.5 + 0.42 * 0.25)).abs() < 0.01);
        assert!(p.feed("upgrading is fun").is_none());
    }

    #[test]
    fn helper_allowlist() {
        assert!(matches!(pkexec_result(126), Err(_)));
        assert!(pkexec_result(0).is_ok());
        if sys::has(sys::PKEXEC) && sys::has(sys::HELPER) {
            assert!(helper(&["upgrade"]).is_ok());
            assert!(helper(&["rollback", "12"]).is_ok());
        }
        assert!(helper(&["rollback", "1; rm"]).is_err());
        assert!(helper(&["rollback", "-1"]).is_err());
        assert!(helper(&["shell"]).is_err());
        assert!(helper(&["upgrade", "--overwrite=*"]).is_err());
    }

    #[test]
    fn urls_are_checked() {
        assert!(!open_url("-x"));
        assert!(!open_url("file:///etc/shadow"));
    }
}
