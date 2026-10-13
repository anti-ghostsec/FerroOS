//! Wi-Fi. The kernel drivers are FerroOS's; the WPA2/WPA3 handshake is
//! Debian's `wpa_supplicant`, and the cards' firmware files are Debian's
//! too, so Wi-Fi needs the Debian layer and starts once it's ready.
//!
//! The network to join (name and password) is one file in the vault,
//! [`CONFIG`]. While it exists and the network switch is on, `wpa_supplicant`
//! runs inside Debian on the first Wi-Fi card; ferro-net then treats that
//! card like any other (random MAC address, DHCP, encrypted DNS).

/// The network to join, as `wpa_supplicant` reads it. Saved with the vault.
pub const CONFIG: &str = "/ProgramData/ferro/wifi.conf";
/// Wi-Fi drivers init found hardware for but left for us to load, one per
/// line: their firmware is in the Debian layer.
pub const DEFERRED: &str = "/run/ferro/deferred-modules";

/// The `wpa_supplicant` configuration for one network. The name goes in as
/// hex, so it can hold anything; an empty password means an open network.
pub fn config(ssid: &str, password: &str) -> String {
    let hex: String = ssid.bytes().map(|b| format!("{b:02x}")).collect();
    let auth = if password.is_empty() {
        "    key_mgmt=NONE\n".to_owned()
    } else {
        let escaped = password.replace('\\', "\\\\").replace('"', "\\\"");
        // WPA2 or WPA3, whichever the access point offers; management
        // frames protected where it can.
        format!("    key_mgmt=WPA-PSK SAE\n    psk=\"{escaped}\"\n    ieee80211w=1\n")
    };
    format!("network={{\n    ssid={hex}\n{auth}}}\n")
}

/// Network names and signal strengths from `iw ... scan` output, strongest
/// first, each name once.
pub fn parse_scan(text: &str) -> Vec<(String, i32)> {
    let mut found: Vec<(String, i32)> = Vec::new();
    let mut signal = -100;
    for line in text.lines().map(str::trim) {
        if let Some(s) = line.strip_prefix("signal:") {
            signal = s.trim().split(['.', ' ']).next().and_then(|v| v.parse().ok()).unwrap_or(-100);
        } else if let Some(name) = line.strip_prefix("SSID:").map(str::trim).filter(|n| !n.is_empty()) {
            match found.iter_mut().find(|(n, _)| n == name) {
                Some(seen) => seen.1 = seen.1.max(signal),
                None => found.push((name.to_owned(), signal)),
            }
        }
    }
    found.sort_by_key(|(_, s)| -s);
    found
}

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Child, Stdio};
    use std::sync::Mutex;
    use std::time::Duration;

    /// `wpa_supplicant`'s own messages, inside Debian (RAM).
    const LOG: &str = "run/ferro-wifi.log";

    /// The running `wpa_supplicant` and the configuration it was started with.
    static RUNNING: Mutex<Option<(Child, String)>> = Mutex::new(None);

    /// Loads the Wi-Fi drivers init left for us, now that their firmware can
    /// be found. `ferro.hwsim` on the kernel command line adds two simulated
    /// radios, for testing in a VM.
    pub fn load_drivers() {
        let mut names: Vec<String> = fs::read_to_string(DEFERRED).unwrap_or_default().lines().map(str::to_owned).collect();
        if fs::read_to_string("/proc/cmdline").unwrap_or_default().split_whitespace().any(|w| w == "ferro.hwsim") {
            names.push("mac80211_hwsim".into());
        }
        for name in names.iter().filter(|n| !n.is_empty()) {
            match ferro_net::route::load_module(name) {
                Ok(()) => eprintln!("ferro-system: Wi-Fi driver {name} loaded"),
                Err(e) => eprintln!("ferro-system: Wi-Fi driver {name}: {e}"),
            }
        }
    }

    /// The first Wi-Fi card.
    pub fn interface() -> Option<String> {
        let mut names: Vec<String> = fs::read_dir("/sys/class/net")
            .ok()?
            .flatten()
            .filter(|e| e.path().join("phy80211").exists())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names.into_iter().next()
    }

    fn tool(program: &str, args: &[&str]) -> Result<String, String> {
        let mut cmd = crate::layer::command(program).ok_or("Debian isn't ready yet (log on first)")?;
        let out = cmd.args(args).stdin(Stdio::null()).output().map_err(|e| format!("{program}: {e}"))?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr);
            return Err(format!("{program}: {}", why.lines().last().unwrap_or("failed").trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// `name|signal dBm;...`, strongest first.
    pub fn scan() -> Result<String, String> {
        let card = interface().ok_or("No Wi-Fi card was found.")?;
        // Scanning needs the card switched on; ferro-net may not have yet.
        let _ = ferro_net::netif::set_up(&card, true);
        let text = tool("/usr/sbin/iw", &["dev", &card, "scan"])?;
        Ok(parse_scan(&text).iter().map(|(n, s)| format!("{}|{s}", n.replace(['|', ';'], " "))).collect::<Vec<_>>().join(";"))
    }

    /// One line for people: what the card is connected to.
    pub fn status() -> String {
        let Some(card) = interface() else { return "No Wi-Fi card was found.".into() };
        if !Path::new(CONFIG).exists() {
            return format!("Wi-Fi ({card}): off. WIFI LIST shows networks, WIFI JOIN <name> connects.");
        }
        match tool("/usr/sbin/iw", &["dev", &card, "link"]) {
            Ok(link) => match link.lines().find_map(|l| l.trim().strip_prefix("SSID:")) {
                Some(name) => {
                    let signal = link.lines().find_map(|l| l.trim().strip_prefix("signal:")).unwrap_or("").trim().to_owned();
                    format!("Wi-Fi ({card}): connected to {} ({signal})", name.trim())
                }
                None => {
                    let log = fs::read_to_string(crate::layer::path(LOG)).unwrap_or_default();
                    let last = log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("starting");
                    format!("Wi-Fi ({card}): connecting ({})", last.trim())
                }
            },
            Err(e) => format!("Wi-Fi ({card}): {e}"),
        }
    }

    /// Saves the network to join; [`supervise`] connects to it.
    pub fn join(ssid: &str, password: &str) -> Result<(), String> {
        if ssid.is_empty() || ssid.len() > 32 {
            return Err("a network name is 1 to 32 bytes".into());
        }
        if !password.is_empty() && !(8..=63).contains(&password.len()) {
            return Err("a Wi-Fi password is 8 to 63 characters".into());
        }
        if password.contains(['\n', '\r']) {
            return Err("the password can't contain line breaks".into());
        }
        interface().ok_or("No Wi-Fi card was found.")?;
        let dest = Path::new(CONFIG);
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::write(dest, config(ssid, password)).map_err(|e| e.to_string())?;
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(0o600));
        Ok(())
    }

    /// Forgets the network; [`supervise`] disconnects.
    pub fn off() {
        let _ = fs::remove_file(CONFIG);
    }

    fn stop(running: &mut Option<(Child, String)>) {
        if let Some((mut child, _)) = running.take() {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("ferro-system: Wi-Fi off");
        }
    }

    /// Keeps `wpa_supplicant` matching what's wanted: running with the saved
    /// network while the network switch is on, stopped otherwise. Never
    /// returns; meant for a background thread.
    pub fn supervise() -> ! {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let wanted = fs::read_to_string(CONFIG).ok().filter(|_| !Path::new(ferro_net::KILL_FILE).exists());
            let mut running = RUNNING.lock().unwrap();
            // Gone by itself (a driver hiccup, a bad setting): start it
            // again below, after a pause so a failure can't spin.
            if running.as_mut().is_some_and(|(child, _)| !matches!(child.try_wait(), Ok(None))) {
                *running = None;
                drop(running);
                std::thread::sleep(Duration::from_secs(10));
                continue;
            }
            match (&wanted, &*running) {
                (Some(config), Some((_, current))) if config == current => {}
                (None, None) => {}
                (None, Some(_)) => stop(&mut running),
                (Some(config), _) => {
                    stop(&mut running);
                    let (Some(card), Some(mut cmd)) = (interface(), crate::layer::command("/usr/sbin/wpa_supplicant")) else { continue };
                    // The WPA handshake travels in raw packets.
                    if let Err(e) = ferro_net::route::load_module("af_packet") {
                        eprintln!("ferro-system: Wi-Fi: {e}");
                    }
                    // Its copy lives in Debian's /run (RAM), readable by root only.
                    let copy = crate::layer::path("run/ferro-wifi.conf");
                    if fs::write(&copy, config).is_err() {
                        continue;
                    }
                    let _ = fs::set_permissions(&copy, fs::Permissions::from_mode(0o600));
                    // What it says (never the password) is kept for WIFI to show.
                    let Ok(log) = fs::File::create(crate::layer::path(LOG)) else { continue };
                    let Ok(log2) = log.try_clone() else { continue };
                    cmd.args(["-i", &card, "-D", "nl80211", "-c", "/run/ferro-wifi.conf"]).stdin(Stdio::null()).stdout(log).stderr(log2);
                    match cmd.spawn() {
                        Ok(child) => {
                            eprintln!("ferro-system: Wi-Fi on ({card})");
                            *running = Some((child, config.clone()));
                        }
                        Err(e) => eprintln!("ferro-system: Wi-Fi: {e}"),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_network() {
        let c = config("Home", "correct horse");
        assert!(c.contains("ssid=486f6d65\n") && c.contains("psk=\"correct horse\"\n") && c.contains("key_mgmt=WPA-PSK SAE"));
        assert!(config("Cafe", "").contains("key_mgmt=NONE") && !config("Cafe", "").contains("psk"));
    }

    #[test]
    fn reads_a_scan() {
        let text = "BSS aa:bb(on wlan0)\n\tsignal: -61.00 dBm\n\tSSID: Home\nBSS cc:dd(on wlan0)\n\tsignal: -40.00 dBm\n\tSSID: Cafe\nBSS ee:ff(on wlan0)\n\tsignal: -50.00 dBm\n\tSSID: Home\n\tSSID: \n";
        assert_eq!(parse_scan(text), [("Cafe".to_owned(), -40), ("Home".to_owned(), -50)]);
    }
}
