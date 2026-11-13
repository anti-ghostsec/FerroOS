//! Remembered choices: per-app network permission and RAM budget, plus the
//! tray kill-switch states. One small plain-text file, so forgetting
//! everything is as simple as deleting it, and the user can read exactly
//! what is remembered:
//!
//! ```text
//! # FerroOS remembered choices. Delete this file to forget them all.
//! switch network=on microphone=off camera=off vpn=off tor=off
//! app browser network=allow memory=5368709120
//! app photo-viewer network=deny memory=52428800
//! ```
//!
//! What's in it decides which programs may reach the network, so it's only
//! read if nobody else could have written it: see [`trusted`].

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

/// Where choices are kept (`C:\ProgramData\ferro\choices.conf`).
pub const DEFAULT_PATH: &str = "/ProgramData/ferro/choices.conf";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetChoice {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppChoice {
    pub network: Option<NetChoice>,
    /// RAM budget in bytes; `Some(None)` means "remembered: no budget".
    pub memory: Option<Option<u64>>,
}

impl AppChoice {
    fn is_empty(&self) -> bool {
        self.network.is_none() && self.memory.is_none()
    }
}

/// The kill switches in the taskbar tray, plus the VPN and Tor. `true` =
/// enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switches {
    pub network: bool,
    pub microphone: bool,
    pub camera: bool,
    /// All traffic through the WireGuard VPN (`C:\ProgramData\ferro\vpn.conf`).
    pub vpn: bool,
    /// Only Tor may reach the internet (through the VPN when that's on).
    pub tor: bool,
}

impl Default for Switches {
    /// Privacy-first: network on, microphone and camera off until wanted.
    /// The VPN and Tor are opt-in.
    fn default() -> Self {
        Self { network: true, microphone: false, camera: false, vpn: false, tor: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Store {
    pub path: PathBuf,
    pub switches: Switches,
    pub apps: BTreeMap<String, AppChoice>,
    pub bridges: Vec<String>,
}

impl Store {
    /// The choices in `path`, or none remembered if it isn't there or isn't
    /// [`trusted`].
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let text = if trusted(&path) { fs::read_to_string(&path).unwrap_or_default() } else { String::new() };
        let mut s = Self::parse(&text);
        s.path = path;
        s
    }

    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut words = line.split_whitespace();
            match words.next() {
                Some("switch") => {
                    for kv in words {
                        let Some((k, v)) = kv.split_once('=') else { continue };
                        let on = v == "on";
                        match k {
                            "network" => s.switches.network = on,
                            "microphone" => s.switches.microphone = on,
                            "camera" => s.switches.camera = on,
                            "vpn" => s.switches.vpn = on,
                            "tor" => s.switches.tor = on,
                            _ => {}
                        }
                    }
                }
                Some("app") => {
                    let Some(name) = words.next() else { continue };
                    let mut choice = AppChoice::default();
                    for kv in words {
                        match kv.split_once('=') {
                            Some(("network", "allow")) => choice.network = Some(NetChoice::Allow),
                            Some(("network", "deny")) => choice.network = Some(NetChoice::Deny),
                            Some(("memory", "none")) => choice.memory = Some(None),
                            Some(("memory", v)) => choice.memory = v.parse().ok().map(Some),
                            _ => {}
                        }
                    }
                    if !choice.is_empty() {
                        s.apps.insert(name.to_owned(), choice);
                    }
                }
                Some("bridge") => {
                    let line = words.collect::<Vec<_>>().join(" ");
                    if !line.is_empty() {
                        s.bridges.push(line);
                    }
                }
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let on = |b: bool| if b { "on" } else { "off" };
        let mut out = String::from("# FerroOS remembered choices. Delete this file to forget them all.\n");
        let sw = self.switches;
        out += &format!(
            "switch network={} microphone={} camera={} vpn={} tor={}\n",
            on(sw.network),
            on(sw.microphone),
            on(sw.camera),
            on(sw.vpn),
            on(sw.tor)
        );
        for b in &self.bridges {
            out += &format!("bridge {b}\n");
        }
        for (name, c) in &self.apps {
            out += &format!("app {name}");
            if let Some(n) = c.network {
                out += if n == NetChoice::Allow { " network=allow" } else { " network=deny" };
            }
            match c.memory {
                Some(Some(b)) => out += &format!(" memory={b}"),
                Some(None) => out += " memory=none",
                None => {}
            }
            out.push('\n');
        }
        out
    }

    /// Writes atomically (temp file + rename) so a crash never leaves half a
    /// file. The temp file is a new one of its own, never one already there
    /// (nor a link someone left in its place).
    pub fn save(&self) -> io::Result<()> {
        use std::io::Write;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension(format!("tmp.{}", std::process::id()));
        let _ = fs::remove_file(&tmp);
        let written = fs::OpenOptions::new().write(true).create_new(true).open(&tmp).and_then(|mut f| f.write_all(self.to_text().as_bytes()));
        match written.and_then(|()| fs::rename(&tmp, &self.path)) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    pub fn app(&self, name: &str) -> AppChoice {
        self.apps.get(name).cloned().unwrap_or_default()
    }

    pub fn set_network(&mut self, name: &str, choice: Option<NetChoice>) {
        self.update(name, |c| c.network = choice);
    }

    pub fn set_memory(&mut self, name: &str, memory: Option<Option<u64>>) {
        self.update(name, |c| c.memory = memory);
    }

    fn update(&mut self, name: &str, f: impl FnOnce(&mut AppChoice)) {
        let mut c = self.app(name);
        f(&mut c);
        if c.is_empty() {
            self.apps.remove(name);
        } else {
            self.apps.insert(name.to_owned(), c);
        }
    }

    /// Forgets one app's choices.
    pub fn forget(&mut self, name: &str) {
        self.apps.remove(name);
    }

    /// Forgets every per-app choice (kill switches keep their state).
    pub fn forget_all(&mut self) {
        self.apps.clear();
    }
}

/// Whether the choices file at `path` can be believed: a plain file (not a
/// link) nobody else may change, put there by someone allowed to. In a
/// folder every account may write in (FerroOS Heavy's `/ProgramData/ferro`
/// was, before it was the administrators'), anyone could have made it
/// first, so only root's or your own counts; otherwise only those the
/// folder lets write could have.
#[cfg(unix)]
pub fn trusted(path: &std::path::Path) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let Ok(file) = fs::symlink_metadata(path) else { return false };
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let Ok(dir) = fs::metadata(dir) else { return false };
    // SAFETY: geteuid has no preconditions and can't fail.
    let me = unsafe { libc::geteuid() };
    let open_to_all = |m: &fs::Metadata| m.permissions().mode() & 0o002 != 0;
    file.file_type().is_file() && !open_to_all(&file) && (file.uid() == 0 || file.uid() == me || !open_to_all(&dir))
}

#[cfg(not(unix))]
pub fn trusted(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_forget() {
        let mut s = Store::default();
        assert_eq!(s.switches, Switches { network: true, microphone: false, camera: false, vpn: false, tor: false });
        s.set_network("browser", Some(NetChoice::Allow));
        s.set_memory("browser", Some(Some(5 << 30)));
        s.set_memory("viewer", Some(Some(50 << 20)));
        s.set_network("tool", Some(NetChoice::Deny));
        s.switches.camera = true;
        let back = Store::parse(&s.to_text());
        assert_eq!(back.apps, s.apps);
        assert!(back.switches.camera);
        assert_eq!(back.app("browser").memory, Some(Some(5 << 30)));

        s.forget("tool");
        assert!(!s.apps.contains_key("tool"));
        s.set_memory("viewer", None); // clearing the only choice drops the app
        assert!(!s.apps.contains_key("viewer"));
        s.forget_all();
        assert!(s.apps.is_empty() && s.switches.camera);
    }

    #[test]
    fn ignores_garbage() {
        let s = Store::parse("junk\napp\napp x memory=lots network=maybe\nswitch camera=on\n");
        assert!(s.apps.is_empty());
        assert!(s.switches.camera);
    }

    #[test]
    fn bridge_round_trip() {
        let mut s = Store::default();
        s.bridges.push("obfs4 192.0.2.1:443 752FC352A747A1548C1ECACDD8048D547D45FC5B cert=xyz iat-mode=0".into());
        s.bridges.push("snowflake 192.0.2.3:1 2B280B23DD74683028100AB73E41B3BB52F4F354".into());
        let back = Store::parse(&s.to_text());
        assert_eq!(back.bridges, s.bridges);
    }

    #[cfg(unix)]
    fn folder(tag: &str, mode: u32) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ferro-store-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn only_a_file_nobody_else_could_write_is_believed() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o755, 0o1777] {
            let dir = folder(&format!("trust{mode:o}"), mode);
            let path = dir.join("choices.conf");
            fs::write(&path, "app evil network=allow\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(trusted(&path), "your own, in a {mode:o} folder");
            assert_eq!(Store::load(&path).app("evil").network, Some(NetChoice::Allow));

            fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
            assert!(!trusted(&path), "anyone may change it");
            assert_eq!(Store::load(&path).app("evil"), AppChoice::default(), "so none of it counts");

            let link = dir.join("link.conf");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(!trusted(&link), "a link, put where the file goes");
            assert!(!trusted(&dir.join("missing.conf")));
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[cfg(unix)]
    #[test]
    fn saving_never_writes_through_a_file_left_in_the_way() {
        let dir = folder("save", 0o1777);
        let path = dir.join("choices.conf");
        let elsewhere = dir.join("elsewhere");
        fs::write(&elsewhere, "untouched").unwrap();
        // Where the temp file goes, a link to someone's file.
        std::os::unix::fs::symlink(&elsewhere, path.with_extension(format!("tmp.{}", std::process::id()))).unwrap();
        let mut s = Store { path: path.clone(), ..Store::default() };
        s.set_network("browser", Some(NetChoice::Deny));
        s.save().unwrap();
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "untouched");
        assert_eq!(Store::load(&path).app("browser").network, Some(NetChoice::Deny));
        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_file());
        let _ = fs::remove_dir_all(&dir);
    }
}
