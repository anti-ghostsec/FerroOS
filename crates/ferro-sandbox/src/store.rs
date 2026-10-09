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
}

impl Store {
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let text = fs::read_to_string(&path).unwrap_or_default();
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

    /// Writes atomically (temp file + rename) so a crash never leaves half a file.
    pub fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("tmp");
        fs::write(&tmp, self.to_text())?;
        fs::rename(&tmp, &self.path)
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
}
