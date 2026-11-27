use crate::sys;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SITE: &str = "https://fiordlandlinux.org";
const ARCH_NEWS: &str = "https://archlinux.org/feeds/news/";
const NEWS_DIR: &str = "/usr/share/fiordland/news";
const PACMAN_LOG: &str = "/var/log/pacman.log";
const CHECK_TIMEOUT: Duration = Duration::from_secs(240);
const QUICK_TIMEOUT: Duration = Duration::from_secs(30);
const WEB_LIMIT: u64 = 512 * 1024;
const NEWS_LIMIT: u64 = 64 * 1024;
const LOG_TAIL: u64 = 1024 * 1024;

#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Outdated {
    pub name: String,
    #[serde(default)]
    pub old: String,
    #[serde(default)]
    pub new: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Pending {
    pub release: Option<String>,
    pub pacman: Option<Vec<Outdated>>,
    pub aur: Option<Vec<Outdated>>,
    pub flatpak: Option<Vec<Outdated>>,
    #[serde(skip)]
    pub via_tawaki: bool,
}

fn count(l: &Option<Vec<Outdated>>) -> usize {
    l.as_ref().map_or(0, Vec::len)
}

impl Pending {
    pub fn total(&self) -> usize {
        self.installable() + count(&self.aur)
    }

    pub fn installable(&self) -> usize {
        count(&self.pacman) + count(&self.flatpak)
    }

    pub fn failed(&self) -> bool {
        self.pacman.is_none()
    }

    pub fn new_release(&self) -> Option<String> {
        self.pacman
            .iter()
            .flatten()
            .find(|o| o.name == "fiordland-base")
            .map(|o| release_of(&o.new))
            .filter(|v| valid_release(v))
    }

    pub fn fingerprints(&self) -> std::collections::BTreeSet<u64> {
        [
            ("pacman", &self.pacman),
            ("aur", &self.aur),
            ("flatpak", &self.flatpak),
        ]
        .iter()
        .flat_map(|(src, l)| {
            l.iter()
                .flatten()
                .map(move |o| crate::config::fingerprint(src, &o.name, &o.new))
        })
        .collect()
    }

    fn sanitize(mut self) -> Pending {
        let keep = |l: Option<Vec<Outdated>>| {
            l.map(|v| {
                v.into_iter()
                    .filter(|o| {
                        sys::valid_pkg_name(&o.name)
                            && sys::valid_version(&o.old)
                            && sys::valid_version(&o.new)
                    })
                    .collect()
            })
        };
        self.pacman = keep(self.pacman);
        self.aur = keep(self.aur);
        self.flatpak = keep(self.flatpak);
        self.release = self.release.filter(|r| valid_release(r));
        self
    }
}

fn release_of(pkgver: &str) -> String {
    let v = pkgver.split_once(':').map_or(pkgver, |(_, v)| v);
    v.rsplit_once('-').map_or(v, |(v, _)| v).to_string()
}

pub fn valid_release(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 32
        && v.starts_with(|c: char| c.is_ascii_digit())
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
        && !v.contains("..")
}

pub fn parse_tawaki(json: &str) -> Option<Pending> {
    let p: Pending = serde_json::from_str(json.trim()).ok()?;
    Some(Pending {
        via_tawaki: true,
        ..p.sanitize()
    })
}

pub fn parse_arrows(out: &str) -> Vec<Outdated> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (name, old, arrow, new) = (it.next()?, it.next()?, it.next()?, it.next()?);
            (arrow == "->").then(|| Outdated {
                name: name.into(),
                old: old.into(),
                new: new.into(),
            })
        })
        .collect()
}

pub fn parse_flatpak_updates(out: &str) -> Vec<Outdated> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let id = it.next()?.trim();
            let ver = it.next().unwrap_or("").trim();
            (!id.is_empty() && id != "Application ID").then(|| Outdated {
                name: id.into(),
                old: String::new(),
                new: ver.into(),
            })
        })
        .collect()
}

fn checkup_db() -> Option<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let mut candidates = vec![std::env::temp_dir().join(format!("checkup-db-{uid}"))];
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    {
        candidates.push(cache.join("tawaki").join("checkup-db"));
    }
    candidates.into_iter().find(|d| {
        std::fs::symlink_metadata(d).is_ok_and(|m| {
            !m.file_type().is_symlink() && m.is_dir() && m.uid() == uid && m.mode() & 0o022 == 0
        })
    })
}

fn fallback() -> Pending {
    let pacman = if sys::has(sys::CHECKUPDATES) {
        let cmd_db = checkup_db();
        let mut c = sys::command(sys::CHECKUPDATES);
        c.arg("--nocolor");
        if let Some(db) = &cmd_db {
            c.env("CHECKUPDATES_DB", db);
        }
        run_checkupdates(c)
    } else {
        None
    };
    let flatpak = if sys::has(sys::FLATPAK) {
        sys::output(
            sys::FLATPAK,
            ["remote-ls", "--updates", "--columns=application,version"],
            QUICK_TIMEOUT * 4,
        )
        .map(|o| parse_flatpak_updates(&o))
    } else {
        Some(vec![])
    };
    Pending {
        release: Some(release()).filter(|r| !r.is_empty()),
        pacman,
        aur: None,
        flatpak,
        via_tawaki: false,
    }
    .sanitize()
}

fn run_checkupdates(mut c: std::process::Command) -> Option<Vec<Outdated>> {
    let (code, out) = sys::run_command(&mut c, CHECK_TIMEOUT)?;
    match code {
        Some(0) => Some(parse_arrows(&out)),
        Some(2) => Some(vec![]),
        _ => None,
    }
}

pub fn pending() -> Pending {
    if sys::has(sys::TAWAKI) {
        if let Some(p) = sys::output(sys::TAWAKI, ["check", "--json"], CHECK_TIMEOUT)
            .and_then(|o| parse_tawaki(&o))
        {
            return p;
        }
    }
    fallback()
}

pub fn parse_sizes(out: &str) -> HashMap<String, u64> {
    out.lines()
        .filter_map(|l| {
            let (n, s) = l.trim().split_once(' ')?;
            Some((n.to_string(), s.trim().parse().ok()?))
        })
        .collect()
}

pub fn sizes(list: &[Outdated]) -> HashMap<String, u64> {
    let names: Vec<&str> = list
        .iter()
        .map(|o| o.name.as_str())
        .filter(|n| sys::valid_pkg_name(n))
        .collect();
    if names.is_empty() || !sys::has(sys::PACMAN) {
        return HashMap::new();
    }
    let mut args: Vec<String> = vec!["-Sp".into(), "--print-format".into(), "%n %s".into()];
    if let Some(db) = checkup_db() {
        args.push("--dbpath".into());
        args.push(db.to_string_lossy().into_owned());
    }
    args.push("--".into());
    args.extend(names.iter().map(|n| n.to_string()));
    sys::output(sys::PACMAN, &args, QUICK_TIMEOUT)
        .map(|o| parse_sizes(&o))
        .unwrap_or_default()
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1000.0;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if v < 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.0} {}", UNITS[u])
    }
}

pub fn is_core(name: &str) -> bool {
    const EXACT: [&str; 12] = [
        "linux",
        "systemd",
        "systemd-libs",
        "systemd-sysvcompat",
        "glibc",
        "dbus",
        "dbus-broker",
        "dbus-broker-units",
        "mesa",
        "amd-ucode",
        "intel-ucode",
        "cryptsetup",
    ];
    if EXACT.contains(&name) {
        return true;
    }
    if name.ends_with("-headers") || name.ends_with("-docs") || name == "linux-api-headers" {
        return false;
    }
    name.starts_with("linux-") || name.starts_with("nvidia")
}

pub fn release() -> String {
    std::fs::read_to_string("/usr/lib/os-release")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("VERSION_ID="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .filter(|v| valid_release(v))
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Restart {
    pub needed: bool,
    pub what: Vec<String>,
}

impl Restart {
    pub fn reason(&self) -> String {
        if self.what.is_empty() {
            return "The new kernel is installed. Devices and drivers that aren't loaded yet won't work until you restart.".into();
        }
        let shown: Vec<&str> = self.what.iter().take(3).map(String::as_str).collect();
        let more = self.what.len().saturating_sub(3);
        let list = if more > 0 {
            format!("{} and {more} more", shown.join(", "))
        } else {
            shown.join(", ")
        };
        format!("Updated since you started this PC: {list}. Restart so everything runs the new versions.")
    }
}

pub fn core_changes_since(log: &str, boot: i64) -> Vec<String> {
    let mut seen = Vec::new();
    for line in log.lines() {
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some((stamp, rest)) = rest.split_once(']') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix("[ALPM] ") else {
            continue;
        };
        let mut words = rest.split_whitespace();
        let verb = words.next().unwrap_or("");
        if !matches!(
            verb,
            "upgraded" | "installed" | "downgraded" | "reinstalled"
        ) {
            continue;
        }
        let Some(name) = words.next() else {
            continue;
        };
        if !is_core(name) || seen.iter().any(|s: &String| s == name) {
            continue;
        }
        if iso_epoch(stamp).is_some_and(|t| t >= boot) {
            seen.push(name.to_string());
        }
    }
    seen
}

fn log_tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(LOG_TAIL);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.take(LOG_TAIL).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some(if start > 0 {
        text.split_once('\n')
            .map(|(_, t)| t.to_string())
            .unwrap_or_default()
    } else {
        text
    })
}

pub fn restart_needed() -> Restart {
    let kernel_gone = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty() && !k.contains('/') && !k.starts_with('.'))
        .is_some_and(|k| !Path::new("/usr/lib/modules").join(k).exists());
    let boot = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| sys::boot_time(&s));
    let what = match (boot, log_tail(Path::new(PACMAN_LOG))) {
        (Some(b), Some(log)) => core_changes_since(&log, b),
        _ => vec![],
    };
    Restart {
        needed: kernel_gone || !what.is_empty(),
        what,
    }
}

pub fn snapshot_note() -> String {
    if !sys::has(sys::SNAPPER) || !Path::new("/etc/snapper/configs/root").exists() {
        return String::new();
    }
    let snap_pac = Path::new("/usr/share/libalpm/hooks/05-snap-pac-pre.hook").exists();
    if snap_pac {
        "A Btrfs snapshot is taken right before the system update, so you can undo it from history if something breaks.".into()
    } else {
        "Snapper is set up, but snapshots aren't taken automatically before updates. Install snap-pac to be able to undo updates.".into()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct News {
    pub kind: &'static str,
    pub title: String,
    pub points: Vec<String>,
    pub link: String,
    pub warn: bool,
}

fn printable(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}

pub fn parse_news(version: &str, text: &str) -> News {
    let text = printable(text);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let title = lines
        .next()
        .map(str::to_string)
        .unwrap_or_else(|| format!("Fiordland {version}"));
    let mut points: Vec<String> = Vec::new();
    for l in lines {
        if let Some(p) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) {
            points.push(p.trim().to_string());
        } else if let Some(last) = points.last_mut() {
            last.push(' ');
            last.push_str(l);
        } else {
            points.push(l.to_string());
        }
    }
    points.truncate(24);
    for p in &mut points {
        if p.chars().count() > 600 {
            *p = p.chars().take(600).collect::<String>() + " ...";
        }
    }
    News {
        kind: "local",
        title,
        points,
        link: String::new(),
        warn: false,
    }
}

pub fn version_key(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

pub fn local_news() -> Vec<News> {
    let Ok(dir) = std::fs::read_dir(NEWS_DIR) else {
        return vec![];
    };
    let mut found: Vec<(Vec<u64>, String, PathBuf)> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let v = name.strip_suffix(".txt")?;
            valid_release(v).then(|| (version_key(v), v.to_string(), e.path()))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found
        .into_iter()
        .take(12)
        .filter_map(|(_, v, p)| {
            let text = sys::read_limited(&p, NEWS_LIMIT)?;
            Some(parse_news(&v, &text))
        })
        .collect()
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .redirects(2)
        .https_only(true)
        .user_agent(concat!("kuaka/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn fetch(url: &str) -> Option<String> {
    let resp = agent().get(url).call().ok()?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(WEB_LIMIT)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

pub fn alerts(p: &Pending) -> Vec<News> {
    let mut out = Vec::new();
    if let Some(v) = p.new_release() {
        let local = Path::new(NEWS_DIR).join(format!("{v}.txt"));
        let text = sys::read_limited(&local, NEWS_LIMIT)
            .or_else(|| fetch(&format!("{SITE}/news/{v}.txt")))
            .filter(|t| t.len() as u64 <= NEWS_LIMIT && !t.trim_start().starts_with('<'))
            .unwrap_or_default();
        let mut n = parse_news(&v, &text);
        if n.points.is_empty() {
            n.title = format!("Fiordland {v}");
        }
        n.kind = "release";
        n.link = format!("{SITE}/docs/updates/");
        out.push(n);
    }
    if p.pacman.as_ref().is_some_and(|l| !l.is_empty()) {
        out.extend(arch_news(last_upgrade()));
    }
    out
}

fn last_upgrade() -> Option<i64> {
    let log = log_tail(Path::new(PACMAN_LOG))?;
    log.lines()
        .rev()
        .find(|l| l.contains("starting full system upgrade"))
        .and_then(|l| l.strip_prefix('[')?.split(']').next())
        .and_then(iso_epoch)
}

fn arch_news(since: Option<i64>) -> Vec<News> {
    let Some(xml) = fetch(ARCH_NEWS) else {
        return vec![];
    };
    let cutoff = since.unwrap_or(sys::now() - 14 * 86400);
    rss_items(&xml)
        .into_iter()
        .filter(|(_, _, _, t)| *t > cutoff)
        .take(3)
        .map(|(title, link, body, _)| News {
            kind: "arch",
            title: printable(&title).replace('\n', " "),
            points: vec![body],
            link: if sys::https_url(&link) {
                link
            } else {
                String::new()
            },
            warn: true,
        })
        .collect()
}

fn rss_items(xml: &str) -> Vec<(String, String, String, i64)> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(xml);
    let mut items = Vec::new();
    let (mut title, mut link, mut desc, mut date) =
        (String::new(), String::new(), String::new(), 0);
    let mut field = String::new();
    let mut inside = false;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                let n = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if n == "item" {
                    inside = true;
                    (title, link, desc, date) = (String::new(), String::new(), String::new(), 0);
                }
                field = n;
            }
            Ok(Event::Text(t)) if inside => {
                let s = t.decode().map(|c| c.into_owned()).unwrap_or_default();
                push(&field, &s, &mut title, &mut link, &mut desc, &mut date);
            }
            Ok(Event::GeneralRef(r)) if inside => {
                let value = r
                    .resolve_char_ref()
                    .ok()
                    .flatten()
                    .map(|c| c.to_string())
                    .or_else(|| {
                        quick_xml::escape::resolve_xml_entity(&r.decode().ok()?).map(str::to_owned)
                    });
                if let Some(s) = value {
                    push(&field, &s, &mut title, &mut link, &mut desc, &mut date);
                }
            }
            Ok(Event::CData(t)) if inside => {
                let s = String::from_utf8_lossy(&t).into_owned();
                push(&field, &s, &mut title, &mut link, &mut desc, &mut date);
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"item" {
                    inside = false;
                    items.push((title.clone(), link.clone(), plain(&desc), date));
                    if items.len() >= 50 {
                        break;
                    }
                }
                field.clear();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    items
}

fn push(
    field: &str,
    s: &str,
    title: &mut String,
    link: &mut String,
    desc: &mut String,
    date: &mut i64,
) {
    match field {
        "title" => title.push_str(s),
        "link" => link.push_str(s.trim()),
        "description" => desc.push_str(s),
        "pubDate" => *date = rfc2822_epoch(s.trim()).unwrap_or(0),
        _ => {}
    }
}

fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in html.chars() {
        match c {
            '<' => tag = true,
            '>' => {
                tag = false;
                out.push(' ');
            }
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    let text = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let text = printable(&text);
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut short = words.iter().take(60).copied().collect::<Vec<_>>().join(" ");
    if words.len() > 60 {
        short.push_str(" ...");
    }
    short
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn offset_secs(z: &str) -> i64 {
    let z = z.trim();
    if z == "Z" || z == "GMT" || z == "UTC" || z.is_empty() {
        return 0;
    }
    let sign = if z.starts_with('-') { -1 } else { 1 };
    let digits: String = z.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return 0;
    }
    let h: i64 = digits[0..2].parse().unwrap_or(0);
    let m: i64 = digits[2..4].parse().unwrap_or(0);
    sign * (h * 3600 + m * 60)
}

fn hms(t: &str) -> Option<i64> {
    let mut it = t.split(':').map(|p| p.parse::<i64>().ok());
    Some(it.next()?? * 3600 + it.next()?? * 60 + it.next().flatten().unwrap_or(0))
}

pub fn iso_epoch(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let cut = rest.find(['+', '-', 'Z']).unwrap_or(rest.len());
    let (time, zone) = rest.split_at(cut);
    Some(days_from_civil(y, m, day) * 86400 + hms(time)? - offset_secs(zone))
}

fn rfc2822_epoch(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let p = if parts.first()?.ends_with(',') {
        &parts[1..]
    } else {
        &parts[..]
    };
    let day: i64 = p.first()?.parse().ok()?;
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mon = *p.get(1)?;
    let m = months.iter().position(|x| *x == mon)? as i64 + 1;
    let y: i64 = p.get(2)?.parse().ok()?;
    let t = hms(p.get(3)?)?;
    Some(days_from_civil(y, m, day) * 86400 + t - offset_secs(p.get(4).unwrap_or(&"")))
}

#[derive(Clone, Debug)]
pub struct Change {
    pub number: i32,
    pub date: String,
    pub what: String,
}

#[derive(Deserialize)]
struct Snap {
    number: i32,
    #[serde(rename = "type")]
    kind: String,
    date: String,
    description: String,
}

pub fn parse_history(out: &str) -> Vec<Change> {
    let v: serde_json::Value = serde_json::from_str(out).unwrap_or_default();
    let snaps: Vec<Snap> = v
        .get("root")
        .and_then(|r| serde_json::from_value(r.clone()).ok())
        .unwrap_or_default();
    let mut changes: Vec<Change> = snaps
        .into_iter()
        .filter(|s| s.kind == "pre" && s.number > 0)
        .map(|s| Change {
            number: s.number,
            date: printable(&s.date).chars().take(16).collect(),
            what: describe(&printable(&s.description)),
        })
        .collect();
    changes.reverse();
    changes.truncate(40);
    changes
}

pub fn history() -> Vec<Change> {
    if !sys::has(sys::SNAPPER) {
        return vec![];
    }
    sys::output(
        sys::SNAPPER,
        ["--jsonout", "-c", "root", "list"],
        QUICK_TIMEOUT,
    )
    .map(|o| parse_history(&o))
    .unwrap_or_default()
}

fn describe(cmd: &str) -> String {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let flags: String = words
        .iter()
        .filter(|w| w.starts_with('-') && !w.starts_with("--"))
        .map(|w| w.trim_start_matches('-'))
        .collect();
    let names: Vec<&str> = words
        .iter()
        .skip(1)
        .filter(|w| !w.starts_with('-'))
        .copied()
        .collect();
    let list = |n: &[&str]| {
        if n.len() > 3 {
            format!("{} and {} more", n[..3].join(", "), n.len() - 3)
        } else {
            n.join(", ")
        }
    };
    if words.first() != Some(&"pacman") {
        return cmd.to_string();
    }
    if flags.contains('S') && flags.contains('u') {
        "System update".into()
    } else if flags.contains('R') {
        format!("Removed {}", list(&names))
    } else if (flags.contains('S') || flags.contains('U')) && !names.is_empty() {
        format!("Installed {}", list(&names))
    } else {
        cmd.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(
            rfc2822_epoch("Mon, 23 Nov 2026 12:00:00 +0000"),
            Some(1795435200)
        );
        assert_eq!(iso_epoch("2026-11-23T07:00:00-0500"), Some(1795435200));
    }

    #[test]
    fn describes() {
        assert_eq!(describe("pacman -Syu"), "System update");
        assert_eq!(
            describe("pacman -S --needed --noconfirm kitty"),
            "Installed kitty"
        );
        assert_eq!(
            describe("pacman -Rns a b c d e"),
            "Removed a, b, c and 2 more"
        );
    }

    #[test]
    fn tawaki_json() {
        let json = r#"{"release":"1.0.0","pacman":[{"name":"linux","old":"6.17.1-1","new":"6.17.2-1"},{"name":"-Syu","old":"1","new":"2"}],"aur":null,"flatpak":[]}"#;
        let p = parse_tawaki(json).unwrap();
        assert!(p.via_tawaki);
        assert_eq!(p.pacman.as_ref().unwrap().len(), 1);
        assert!(p.aur.is_none());
        assert_eq!(p.total(), 1);
        assert!(!p.failed());
        assert!(parse_tawaki("not json").is_none());
        let failed =
            parse_tawaki(r#"{"release":null,"pacman":null,"aur":[],"flatpak":[]}"#).unwrap();
        assert!(failed.failed());
    }

    #[test]
    fn releases() {
        let p = Pending {
            pacman: Some(vec![Outdated {
                name: "fiordland-base".into(),
                old: "0.9.1-1".into(),
                new: "1.0.0-2".into(),
            }]),
            ..Default::default()
        };
        assert_eq!(p.new_release().as_deref(), Some("1.0.0"));
        assert!(!valid_release("../../etc"));
        assert!(!valid_release("1.0/x"));
        assert!(valid_release("1.0.0"));
        assert_eq!(release_of("2:1.2-3"), "1.2");
    }

    #[test]
    fn arrows_and_flatpak() {
        let a = parse_arrows("linux 6.1-1 -> 6.2-1\ngarbage\nmesa 1 -> 2\n");
        assert_eq!(a.len(), 2);
        assert_eq!(a[1].new, "2");
        let f = parse_flatpak_updates("org.mozilla.firefox\t133.0\norg.gnome.Platform\t\n");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].name, "org.mozilla.firefox");
    }

    #[test]
    fn size_parsing() {
        let s = parse_sizes("linux 141234567\nmesa 30000000\nbad line\n");
        assert_eq!(s.get("linux"), Some(&141234567));
        assert_eq!(s.len(), 2);
        assert_eq!(human_size(312_400_000), "312 MB");
        assert_eq!(human_size(1_500_000_000), "1.5 GB");
        assert_eq!(human_size(900), "900 B");
    }

    #[test]
    fn core_packages() {
        assert!(is_core("linux"));
        assert!(is_core("linux-cachyos"));
        assert!(is_core("linux-firmware-amdgpu"));
        assert!(is_core("systemd"));
        assert!(is_core("nvidia-open"));
        assert!(!is_core("linux-cachyos-headers"));
        assert!(!is_core("linux-api-headers"));
        assert!(!is_core("firefox"));
    }

    #[test]
    fn restart_from_log() {
        let log = "\
[2026-11-23T06:00:00-0500] [ALPM] upgraded systemd (257.1-1 -> 257.2-1)
[2026-11-23T08:00:00-0500] [ALPM] upgraded linux (6.17.1-1 -> 6.17.2-1)
[2026-11-23T08:00:01-0500] [ALPM] upgraded firefox (1-1 -> 2-1)
[2026-11-23T08:00:02-0500] [ALPM] installed linux (6.17.2-1)
[2026-11-23T08:00:03-0500] [ALPM-SCRIPTLET] upgraded glibc (fake)
";
        let boot = iso_epoch("2026-11-23T07:00:00-0500").unwrap();
        assert_eq!(core_changes_since(log, boot), vec!["linux".to_string()]);
        let r = Restart {
            needed: true,
            what: vec![
                "linux".into(),
                "systemd".into(),
                "mesa".into(),
                "glibc".into(),
            ],
        };
        assert!(r.reason().contains("and 1 more"));
    }

    #[test]
    fn news_format() {
        let n = parse_news(
            "1.0.0",
            "Fiordland 1\n- Faster start.\n  More on the next line.\n- Firewall on by default.\n",
        );
        assert_eq!(n.title, "Fiordland 1");
        assert_eq!(n.points.len(), 2);
        assert_eq!(n.points[0], "Faster start. More on the next line.");
        assert!(version_key("1.0.0") > version_key("0.9.1"));
        assert!(version_key("0.10.0") > version_key("0.9.1"));
    }

    #[test]
    fn history_json() {
        let out = r#"{"root":[{"number":0,"type":"single","date":"","description":"current"},{"number":41,"type":"pre","date":"2026-11-20 10:00:00","description":"pacman -Syu"},{"number":42,"type":"post","date":"2026-11-20 10:02:00","description":"pacman -Syu"}]}"#;
        let h = parse_history(out);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].number, 41);
        assert_eq!(h[0].what, "System update");
    }
}
