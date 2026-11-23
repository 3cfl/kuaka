use serde::Deserialize;
use std::process::{Command, Stdio};
use std::time::Duration;

const SITE: &str = "https://fiordland.apollo44.lat";
const ARCH_NEWS: &str = "https://archlinux.org/feeds/news/";

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Outdated {
    pub name: String,
    pub old: String,
    pub new: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Pending {
    pub release: Option<String>,
    pub pacman: Option<Vec<Outdated>>,
    pub aur: Option<Vec<Outdated>>,
    pub flatpak: Option<Vec<Outdated>>,
}

impl Pending {
    pub fn total(&self) -> usize {
        [&self.pacman, &self.aur, &self.flatpak]
            .iter()
            .map(|l| l.as_ref().map_or(0, Vec::len))
            .sum()
    }

    pub fn failed(&self) -> bool {
        self.pacman.is_none()
    }

    pub fn new_release(&self) -> Option<String> {
        self.pacman
            .iter()
            .flatten()
            .find(|o| o.name == "fiordland-base")
            .map(|o| o.new.split('-').next().unwrap_or(&o.new).to_string())
    }
}

#[derive(Clone, Debug)]
pub struct News {
    pub title: String,
    pub body: String,
    pub link: String,
    pub warn: bool,
}

#[derive(Clone, Debug)]
pub struct Change {
    pub number: i32,
    pub date: String,
    pub what: String,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .user_agent(concat!("kuaka/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn output(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn pending() -> Pending {
    output("tawaki", &["check", "--json"])
        .and_then(|o| serde_json::from_str(o.trim()).ok())
        .unwrap_or_default()
}

pub fn release() -> String {
    std::fs::read_to_string("/usr/lib/os-release")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("VERSION_ID="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_default()
}

pub fn restart_needed() -> bool {
    let Some(kernel) = output("uname", &["-r"]) else {
        return false;
    };
    !std::path::Path::new("/usr/lib/modules")
        .join(kernel.trim())
        .exists()
}

pub fn news(p: &Pending) -> Vec<News> {
    let mut out = Vec::new();
    if let Some(v) = p.new_release() {
        let body = agent()
            .get(&format!("{SITE}/news/{v}.txt"))
            .call()
            .ok()
            .and_then(|r| r.into_string().ok())
            .unwrap_or_default();
        out.push(News {
            title: format!("Fiordland {v}"),
            body: body.trim().to_string(),
            link: format!("{SITE}/docs/updates/"),
            warn: false,
        });
    }
    out.extend(arch_news(last_upgrade()));
    out
}

fn last_upgrade() -> Option<i64> {
    let log = std::fs::read_to_string("/var/log/pacman.log").ok()?;
    log.lines()
        .rev()
        .find(|l| l.contains("starting full system upgrade"))
        .and_then(|l| l.strip_prefix('[')?.split(']').next())
        .and_then(iso_epoch)
}

fn arch_news(since: Option<i64>) -> Vec<News> {
    let Some(xml) = agent()
        .get(ARCH_NEWS)
        .call()
        .ok()
        .and_then(|r| r.into_string().ok())
    else {
        return vec![];
    };
    let now = now();
    let cutoff = since.unwrap_or(now - 14 * 86400);
    rss_items(&xml)
        .into_iter()
        .filter(|(_, _, _, t)| *t > cutoff)
        .take(3)
        .map(|(title, link, body, _)| News {
            title,
            body,
            link,
            warn: true,
        })
        .collect()
}

fn rss_items(xml: &str) -> Vec<(String, String, String, i64)> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(xml);
    let mut items = Vec::new();
    let (mut title, mut link, mut desc, mut date) = (String::new(), String::new(), String::new(), 0);
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
                let s = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                push(&field, &s, &mut title, &mut link, &mut desc, &mut date);
            }
            Ok(Event::CData(t)) if inside => {
                let s = String::from_utf8_lossy(&t).into_owned();
                push(&field, &s, &mut title, &mut link, &mut desc, &mut date);
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"item" {
                    inside = false;
                    items.push((title.clone(), link.clone(), plain(&desc), date));
                }
                field.clear();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    items
}

fn push(field: &str, s: &str, title: &mut String, link: &mut String, desc: &mut String, date: &mut i64) {
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
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut short = words.iter().take(60).copied().collect::<Vec<_>>().join(" ");
    if words.len() > 60 {
        short.push_str(" ...");
    }
    short
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

fn iso_epoch(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let cut = rest.find(['+', '-', 'Z']).unwrap_or(rest.len());
    let (time, zone) = rest.split_at(cut);
    Some(days_from_civil(y, m, day) * 86400 + hms(time)? - offset_secs(zone))
}

fn rfc2822_epoch(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let p = if parts.first()?.ends_with(',') { &parts[1..] } else { &parts[..] };
    let day: i64 = p.first()?.parse().ok()?;
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mon = *p.get(1)?;
    let m = months.iter().position(|x| *x == mon)? as i64 + 1;
    let y: i64 = p.get(2)?.parse().ok()?;
    let t = hms(p.get(3)?)?;
    Some(days_from_civil(y, m, day) * 86400 + t - offset_secs(p.get(4).unwrap_or(&"")))
}

#[derive(Deserialize)]
struct Snap {
    number: i32,
    #[serde(rename = "type")]
    kind: String,
    date: String,
    description: String,
}

pub fn history() -> Vec<Change> {
    let Some(out) = output("snapper", &["--jsonout", "-c", "root", "list"]) else {
        return vec![];
    };
    let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_default();
    let snaps: Vec<Snap> = v
        .get("root")
        .and_then(|r| serde_json::from_value(r.clone()).ok())
        .unwrap_or_default();
    let mut changes: Vec<Change> = snaps
        .into_iter()
        .filter(|s| s.kind == "pre")
        .map(|s| Change {
            number: s.number,
            date: s.date.chars().take(16).collect(),
            what: describe(&s.description),
        })
        .collect();
    changes.reverse();
    changes.truncate(40);
    changes
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
        assert_eq!(rfc2822_epoch("Mon, 23 Nov 2026 12:00:00 +0000"), Some(1795435200));
        assert_eq!(iso_epoch("2026-11-23T07:00:00-0500"), Some(1795435200));
    }

    #[test]
    fn describes() {
        assert_eq!(describe("pacman -Syu"), "System update");
        assert_eq!(describe("pacman -S --needed --noconfirm kitty"), "Installed kitty");
        assert_eq!(describe("pacman -Rns a b c d e"), "Removed a, b, c and 2 more");
    }
}
