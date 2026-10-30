//! MPRIS (Media Player Remote Interfacing Specification) integration for FerroOS.
//! Allows listing active media players (browsers, video players, music players),
//! querying their volume, and setting volume/mute per application via D-Bus.

use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct MprisPlayer {
    pub dest: String,
    pub identity: String,
    pub label: String,
    pub sublabel: String,
    pub volume: u8,
    pub muted: bool,
    pub saved_volume: u8,
}

/// Discovers all currently registered MPRIS media players on the session bus.
pub fn list_players() -> Vec<MprisPlayer> {
    let output = match Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.DBus",
            "--type=method_call",
            "--print-reply",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.ListNames",
        ])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => return Vec::new(),
    };

    let mut players = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("string \"org.mpris.MediaPlayer2.") {
            if let Some(start) = trimmed.find('"') {
                if let Some(end) = trimmed[start + 1..].find('"') {
                    let dest = trimmed[start + 1..start + 1 + end].to_string();
                    if let Some(player) = inspect_player(&dest) {
                        players.push(player);
                    }
                }
            }
        }
    }
    players
}

/// Queries Identity and Volume for an MPRIS player.
fn inspect_player(dest: &str) -> Option<MprisPlayer> {
    // 1. Identity
    let mut identity = String::new();
    if let Ok(out) = Command::new("dbus-send")
        .args([
            "--session",
            &format!("--dest={dest}"),
            "--type=method_call",
            "--print-reply",
            "/org/mpris/MediaPlayer2",
            "org.freedesktop.DBus.Properties.Get",
            "string:org.mpris.MediaPlayer2",
            "string:Identity",
        ])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("string \"") && trimmed.ends_with('"') {
                    identity = trimmed["string \"".len()..trimmed.len() - 1].to_string();
                    break;
                }
            }
        }
    }

    if identity.is_empty() {
        // Fallback to name segment after MediaPlayer2.
        if let Some(tail) = dest.strip_prefix("org.mpris.MediaPlayer2.") {
            identity = tail.split('.').next().unwrap_or(tail).to_string();
        } else {
            identity = dest.to_string();
        }
    }

    // 2. Volume
    let mut volume = 100u8;
    if let Ok(out) = Command::new("dbus-send")
        .args([
            "--session",
            &format!("--dest={dest}"),
            "--type=method_call",
            "--print-reply",
            "/org/mpris/MediaPlayer2",
            "org.freedesktop.DBus.Properties.Get",
            "string:org.mpris.MediaPlayer2.Player",
            "string:Volume",
        ])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                let trimmed = line.trim();
                if let Some(num_str) = trimmed.strip_prefix("double ") {
                    if let Ok(v) = num_str.trim().parse::<f64>() {
                        volume = ((v * 100.0).round() as u8).clamp(0, 100);
                        break;
                    }
                }
            }
        }
    }

    let (label, sublabel) = format_labels(dest, &identity);
    let muted = volume == 0;
    let saved_volume = if volume > 0 { volume } else { 80 };

    Some(MprisPlayer { dest: dest.to_string(), identity, label, sublabel, volume, muted, saved_volume })
}

/// Generates clean, authentic retro labels for known apps.
pub fn format_labels(dest: &str, identity: &str) -> (String, String) {
    let lower_id = identity.to_ascii_lowercase();
    let lower_dest = dest.to_ascii_lowercase();

    if lower_id.contains("wisp") || lower_dest.contains("wisp") {
        ("Browser".to_string(), "Wisp".to_string())
    } else if lower_id.contains("librewolf") || lower_dest.contains("librewolf") {
        ("Browser".to_string(), "LibreWolf".to_string())
    } else if lower_id.contains("firefox") || lower_dest.contains("firefox") {
        ("Browser".to_string(), "Firefox".to_string())
    } else if lower_id.contains("mpv") || lower_dest.contains("mpv") {
        ("Video".to_string(), "MPV Player".to_string())
    } else if lower_id.contains("vlc") || lower_dest.contains("vlc") {
        ("Video".to_string(), "VLC Player".to_string())
    } else if lower_id.contains("chromium") || lower_dest.contains("chromium") || lower_id.contains("chrome") {
        ("Browser".to_string(), "Chromium".to_string())
    } else if lower_id.contains("tor") || lower_dest.contains("tor") {
        ("Browser".to_string(), "TorBrowser".to_string())
    } else if lower_id.contains("spotify") || lower_dest.contains("spotify") {
        ("Music".to_string(), "Spotify".to_string())
    } else {
        let label = if identity.len() > 10 { format!("{}.", &identity[..9]) } else { identity.to_string() };
        (label, "App".to_string())
    }
}

/// Sets the volume of an MPRIS player (0-100%).
pub fn set_player_volume(dest: &str, vol_pct: u8) {
    let frac = (vol_pct.min(100) as f64) / 100.0;
    let _ = Command::new("dbus-send")
        .args([
            "--session",
            &format!("--dest={dest}"),
            "--type=method_call",
            "/org/mpris/MediaPlayer2",
            "org.freedesktop.DBus.Properties.Set",
            "string:org.mpris.MediaPlayer2.Player",
            "string:Volume",
            &format!("variant:double:{frac:.4}"),
        ])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_labels() {
        let (lbl, sub) = format_labels("org.mpris.MediaPlayer2.firefox.instance_1_18", "Mozilla librewolf");
        assert_eq!(lbl, "Browser");
        assert_eq!(sub, "LibreWolf");

        let (lbl, sub) = format_labels("org.mpris.MediaPlayer2.io.github.wisp.Browser", "Wisp");
        assert_eq!(lbl, "Browser");
        assert_eq!(sub, "Wisp");

        let (lbl2, sub2) = format_labels("org.mpris.MediaPlayer2.mpv", "mpv");
        assert_eq!(lbl2, "Video");
        assert_eq!(sub2, "MPV Player");
    }
}
