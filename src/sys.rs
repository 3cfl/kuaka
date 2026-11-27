use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const PKEXEC: &str = "/usr/bin/pkexec";
pub const HELPER: &str = "/usr/lib/kuaka/kuaka-helper";
pub const TAWAKI: &str = "/usr/bin/tawaki";
pub const FLATPAK: &str = "/usr/bin/flatpak";
pub const PACMAN: &str = "/usr/bin/pacman";
pub const CHECKUPDATES: &str = "/usr/bin/checkupdates";
pub const SNAPPER: &str = "/usr/bin/snapper";
pub const SYSTEMCTL: &str = "/usr/bin/systemctl";
pub const XDG_OPEN: &str = "/usr/bin/xdg-open";
pub const KUAKA: &str = "/usr/bin/kuaka";
pub const SAFE_PATH: &str = "/usr/local/bin:/usr/bin";

const OUTPUT_LIMIT: u64 = 16 << 20;
const MIN_KB: u64 = 4 * 1024 * 1024;

pub fn has(bin: &str) -> bool {
    Path::new(bin).is_file()
}

pub fn command(bin: &str) -> Command {
    let mut c = Command::new(bin);
    c.env("PATH", SAFE_PATH)
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    c
}

fn wait_until(child: &mut Child, deadline: Instant) -> Option<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                // run_command gives each check its own group. checkupdates can
                // be waiting on a second process that also holds the output pipe.
                unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL) };
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => return None,
        }
    }
}

pub fn run_output<I, S>(bin: &str, args: I, timeout: Duration) -> Option<(Option<i32>, String)>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_command(command(bin).args(args), timeout)
}

pub fn run_command(cmd: &mut Command, timeout: Duration) -> Option<(Option<i32>, String)> {
    let mut child = cmd
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let out = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.take(OUTPUT_LIMIT).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + timeout;
    let status = wait_until(&mut child, deadline)?;
    let buf = rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(2))
        .ok()?;
    Some((status.code(), String::from_utf8_lossy(&buf).into_owned()))
}

pub fn output<I, S>(bin: &str, args: I, timeout: Duration) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    match run_output(bin, args, timeout)? {
        (Some(0), out) => Some(out),
        _ => None,
    }
}

pub fn spawn_detached<I, S>(bin: &str, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let child = command(bin)
        .env_remove("LC_ALL")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(mut c) => {
            std::thread::spawn(move || {
                let _ = c.wait();
            });
            true
        }
        Err(_) => false,
    }
}

pub fn read_limited(path: &Path, limit: u64) -> Option<String> {
    let f = std::fs::File::open(path).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    f.take(limit).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

pub fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            continue;
        }
        if c == '\t' {
            out.push(' ');
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

pub fn https_url(url: &str) -> bool {
    url.starts_with("https://")
        && url.len() < 2048
        && !url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '"' || c == '\\')
}

pub fn valid_pkg_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with(['-', '.'])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b))
}

pub fn valid_version(v: &str) -> bool {
    v.len() <= 128
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-:~".contains(&b))
}

pub fn flag(name: &str) -> Option<bool> {
    match std::env::var(name).ok()?.as_str() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

pub fn mem_total_kb(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
}

pub fn software_gl(get: impl Fn(&str) -> Option<String>) -> bool {
    if get("LIBGL_ALWAYS_SOFTWARE").is_some_and(|v| !v.is_empty() && v != "0") {
        return true;
    }
    if get("GALLIUM_DRIVER").is_some_and(|v| v == "llvmpipe" || v == "softpipe") {
        return true;
    }
    get("SLINT_BACKEND").is_some_and(|v| v.contains("software"))
}

fn render_node() -> bool {
    std::fs::read_dir("/dev/dri")
        .map(|d| {
            d.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        })
        .unwrap_or(false)
}

pub fn low_end() -> bool {
    if let Some(forced) = flag("KUAKA_LOW_END") {
        return forced;
    }
    if software_gl(|k| std::env::var(k).ok()) {
        return true;
    }
    let mem = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| mem_total_kb(&m));
    if mem.is_some_and(|kb| kb < MIN_KB) {
        return true;
    }
    !render_node()
}

pub fn reduce_motion(low: bool) -> bool {
    flag("KUAKA_REDUCE_MOTION").unwrap_or(low)
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ];
    if let Some(data) = data_home() {
        dirs.push(data.join("fonts"));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".fonts"));
    }
    dirs
}

pub fn symbols_font() -> bool {
    if let Some(forced) = flag("KUAKA_SYMBOLS") {
        return forced;
    }
    let is_symbols = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().starts_with("MaterialSymbolsRounded"))
            .unwrap_or(false)
    };
    for dir in font_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if is_symbols(&p) {
                return true;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if let Ok(sub) = std::fs::read_dir(&p) {
                    if sub.flatten().any(|s| is_symbols(&s.path())) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub fn theme_mode() -> i32 {
    match std::env::var("KUAKA_THEME").as_deref() {
        Ok("dark") => 1,
        Ok("light") => 2,
        _ => 0,
    }
}

fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|h| h.join(fallback))
        })
}

pub fn config_home() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

pub fn state_home() -> Option<PathBuf> {
    xdg_dir("XDG_STATE_HOME", ".local/state")
}

pub fn data_home() -> Option<PathBuf> {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn boot_time(stat: &str) -> Option<i64> {
    stat.lines()
        .find_map(|l| l.strip_prefix("btime "))
        .and_then(|v| v.trim().parse().ok())
}

pub fn on_battery() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    let mut battery = false;
    for e in entries.flatten() {
        let p = e.path();
        let read = |f: &str| {
            std::fs::read_to_string(p.join(f))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        match read("type").as_str() {
            "Mains" | "USB" if read("online") == "1" => return false,
            "Battery" if read("scope") != "Device" => battery = true,
            _ => {}
        }
    }
    battery
}

pub fn metered() -> bool {
    let check = || -> zbus::Result<u32> {
        let conn = zbus::blocking::Connection::system()?;
        let proxy = zbus::blocking::Proxy::new(
            &conn,
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager",
            "org.freedesktop.NetworkManager",
        )?;
        proxy.get_property::<u32>("Metered")
    };
    matches!(check(), Ok(1) | Ok(3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo() {
        assert_eq!(
            mem_total_kb("MemTotal:        3906848 kB\nMemFree: 1 kB"),
            Some(3906848)
        );
        assert_eq!(mem_total_kb("nothing"), None);
    }

    #[test]
    fn software() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(software_gl(env(&[("LIBGL_ALWAYS_SOFTWARE", "1")])));
        assert!(!software_gl(env(&[("LIBGL_ALWAYS_SOFTWARE", "0")])));
        assert!(software_gl(env(&[("GALLIUM_DRIVER", "llvmpipe")])));
        assert!(software_gl(env(&[("SLINT_BACKEND", "winit-software")])));
        assert!(!software_gl(env(&[])));
    }

    #[test]
    fn names() {
        assert!(valid_pkg_name("linux-firmware"));
        assert!(valid_pkg_name("lib32-mesa"));
        assert!(valid_pkg_name("org.mozilla.firefox"));
        assert!(!valid_pkg_name("-Syu"));
        assert!(!valid_pkg_name("a b"));
        assert!(!valid_pkg_name("a;rm"));
        assert!(!valid_pkg_name(""));
        assert!(valid_version("1:2.3.4-1"));
        assert!(!valid_version("1 2"));
    }

    #[test]
    fn urls() {
        assert!(https_url("https://archlinux.org/news/x/"));
        assert!(!https_url("http://example.com"));
        assert!(!https_url("--help"));
        assert!(!https_url("https://a b"));
        assert!(!https_url("file:///etc/passwd"));
    }

    #[test]
    fn update_check_times_out_and_preserves_no_updates_status() {
        assert!(run_output(
            "/bin/sh",
            ["-c", "sleep 5 & wait"],
            Duration::from_millis(50)
        )
        .is_none());
        assert_eq!(
            run_output("/bin/sh", ["-c", "exit 2"], Duration::from_secs(2)),
            Some((Some(2), String::new()))
        );
    }

    #[test]
    fn cleans() {
        assert_eq!(clean_line("\u{1b}[1;32mok\u{1b}[0m\tdone\r"), "ok done");
    }

    #[test]
    fn boot() {
        assert_eq!(boot_time("cpu 1 2\nbtime 1795435200\n"), Some(1795435200));
    }
}
