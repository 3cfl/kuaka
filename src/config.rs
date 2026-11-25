use crate::sys;
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const LIMIT: u64 = 64 * 1024;
pub const NOTIFY_GAP: i64 = 20 * 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub interval_hours: u32,
    pub notify: bool,
    pub pause_on_battery: bool,
    pub pause_on_metered: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            interval_hours: 6,
            notify: true,
            pause_on_battery: true,
            pause_on_metered: true,
        }
    }
}

fn boolean(v: &str) -> Option<bool> {
    match v {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "check_every_hours" => {
                    if let Ok(h) = v.parse::<u32>() {
                        s.interval_hours = h.clamp(1, 168);
                    }
                }
                "notify" => s.notify = boolean(v).unwrap_or(s.notify),
                "pause_on_battery" => s.pause_on_battery = boolean(v).unwrap_or(s.pause_on_battery),
                "pause_on_metered" => s.pause_on_metered = boolean(v).unwrap_or(s.pause_on_metered),
                _ => {}
            }
        }
        s
    }

    pub fn render(&self) -> String {
        format!(
            "check_every_hours = {}\nnotify = {}\npause_on_battery = {}\npause_on_metered = {}\n",
            self.interval_hours.clamp(1, 168),
            self.notify,
            self.pause_on_battery,
            self.pause_on_metered
        )
    }

    pub fn load() -> Settings {
        config_file()
            .and_then(|p| sys::read_limited(&p, LIMIT))
            .map(|t| Settings::parse(&t))
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = config_file().context("no home folder")?;
        write_private(&path, self.render().as_bytes())
    }
}

fn config_file() -> Option<PathBuf> {
    sys::config_home().map(|d| d.join("kuaka").join("kuaka.conf"))
}

fn state_file() -> Option<PathBuf> {
    sys::state_home().map(|d| d.join("kuaka").join("notified"))
}

fn private_dir(dir: &Path) -> Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(_) => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .with_context(|| format!("couldn't create {}", dir.display()))?;
        }
    }
    let meta = std::fs::symlink_metadata(dir)?;
    let uid = euid();
    if meta.file_type().is_symlink() || !meta.is_dir() || meta.uid() != uid {
        bail!("{} is not a private folder owned by you", dir.display());
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().context("bad path")?;
    private_dir(dir)?;
    let name = path.file_name().context("bad path")?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)
        .with_context(|| format!("couldn't write {}", tmp.display()))?;
    let written = f.write_all(data).and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = written.and_then(|_| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Notified {
    pub at: i64,
    pub items: BTreeSet<u64>,
}

impl Notified {
    pub fn parse(text: &str) -> Notified {
        let mut lines = text.lines();
        let at = lines
            .next()
            .and_then(|l| l.trim().parse().ok())
            .unwrap_or(0);
        let items = lines
            .filter_map(|l| u64::from_str_radix(l.trim(), 16).ok())
            .collect();
        Notified { at, items }
    }

    pub fn render(&self) -> String {
        let mut s = format!("{}\n", self.at);
        for i in &self.items {
            s.push_str(&format!("{i:016x}\n"));
        }
        s
    }

    pub fn load() -> Notified {
        state_file()
            .and_then(|p| sys::read_limited(&p, 1 << 20))
            .map(|t| Notified::parse(&t))
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = state_file().context("no home folder")?;
        write_private(&path, self.render().as_bytes())
    }

    pub fn should_notify(&self, current: &BTreeSet<u64>, now: i64) -> bool {
        current.iter().any(|h| !self.items.contains(h)) && now - self.at >= NOTIFY_GAP
    }
}

pub fn fingerprint(source: &str, name: &str, version: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in source
        .bytes()
        .chain([0])
        .chain(name.bytes())
        .chain([0])
        .chain(version.bytes())
    {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let s = Settings {
            interval_hours: 12,
            notify: false,
            pause_on_battery: true,
            pause_on_metered: false,
        };
        assert_eq!(Settings::parse(&s.render()), s);
    }

    #[test]
    fn settings_are_clamped_and_forgiving() {
        let s = Settings::parse("check_every_hours = 0\nnotify = maybe\njunk\n");
        assert_eq!(s.interval_hours, 1);
        assert!(s.notify);
        assert_eq!(
            Settings::parse("check_every_hours=9999").interval_hours,
            168
        );
        assert_eq!(Settings::parse("").interval_hours, 6);
    }

    #[test]
    fn notify_only_for_new_items_and_not_too_often() {
        let a = fingerprint("pacman", "a", "1");
        let b = fingerprint("pacman", "b", "1");
        let seen = Notified {
            at: 1000,
            items: [a].into_iter().collect(),
        };
        let only_a: BTreeSet<u64> = [a].into_iter().collect();
        let both: BTreeSet<u64> = [a, b].into_iter().collect();
        assert!(!seen.should_notify(&only_a, 1000 + NOTIFY_GAP * 2));
        assert!(!seen.should_notify(&both, 1000 + 60));
        assert!(seen.should_notify(&both, 1000 + NOTIFY_GAP));
        assert!(!seen.should_notify(&BTreeSet::new(), 1000 + NOTIFY_GAP));
        assert_eq!(Notified::parse(&seen.render()), seen);
    }

    #[test]
    fn fingerprints_differ_by_field() {
        assert_ne!(fingerprint("a", "bc", "d"), fingerprint("ab", "c", "d"));
        assert_ne!(
            fingerprint("pacman", "a", "1"),
            fingerprint("pacman", "a", "2")
        );
    }
}
