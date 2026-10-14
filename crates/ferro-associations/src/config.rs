//! File association configuration parsing, resolution, and persistence.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssociationEntry {
    pub ext: String,
    pub desc: String,
    pub command: String,
}

pub const PRESET_APPS: [(&str, &str); 6] = [
    ("Geany Text Editor", "geany %f"),
    ("qView Image Viewer", "qview %f"),
    ("MPV Media Player", "mpv %f"),
    ("LibreWolf Browser", "librewolf %f"),
    ("Terminal Editor (Nano)", "xterm -bg \"#000000\" -fg \"#FFFFFF\" -e nano %f"),
    ("System Default (xdg-open)", "xdg-open %f"),
];

pub fn default_associations() -> Vec<AssociationEntry> {
    vec![
        AssociationEntry { ext: "txt".into(), desc: "Text Document".into(), command: "geany %f".into() },
        AssociationEntry { ext: "md".into(), desc: "Markdown Document".into(), command: "geany %f".into() },
        AssociationEntry { ext: "rs".into(), desc: "Rust Source File".into(), command: "geany %f".into() },
        AssociationEntry { ext: "c".into(), desc: "C Source File".into(), command: "geany %f".into() },
        AssociationEntry { ext: "h".into(), desc: "C Header File".into(), command: "geany %f".into() },
        AssociationEntry { ext: "sh".into(), desc: "Shell Script".into(), command: "geany %f".into() },
        AssociationEntry { ext: "conf".into(), desc: "Configuration File".into(), command: "geany %f".into() },
        AssociationEntry { ext: "ini".into(), desc: "INI Configuration".into(), command: "geany %f".into() },
        AssociationEntry { ext: "png".into(), desc: "PNG Image".into(), command: "qview %f".into() },
        AssociationEntry { ext: "jpg".into(), desc: "JPEG Image".into(), command: "qview %f".into() },
        AssociationEntry { ext: "jpeg".into(), desc: "JPEG Image".into(), command: "qview %f".into() },
        AssociationEntry { ext: "bmp".into(), desc: "Bitmap Image".into(), command: "qview %f".into() },
        AssociationEntry { ext: "gif".into(), desc: "GIF Animation".into(), command: "qview %f".into() },
        AssociationEntry { ext: "mp4".into(), desc: "MP4 Video".into(), command: "mpv %f".into() },
        AssociationEntry { ext: "mkv".into(), desc: "Matroska Video".into(), command: "mpv %f".into() },
        AssociationEntry { ext: "mp3".into(), desc: "MP3 Audio".into(), command: "mpv %f".into() },
        AssociationEntry { ext: "wav".into(), desc: "Waveform Audio".into(), command: "mpv %f".into() },
        AssociationEntry { ext: "html".into(), desc: "HTML Document".into(), command: "librewolf %f".into() },
        AssociationEntry { ext: "pdf".into(), desc: "PDF Document".into(), command: "xdg-open %f".into() },
    ]
}

pub fn config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("ferro").join("associations.conf")
    } else {
        PathBuf::from("/etc/ferro/associations.conf")
    }
}

pub fn load_associations() -> Vec<AssociationEntry> {
    let mut map: BTreeMap<String, AssociationEntry> = default_associations().into_iter().map(|a| (a.ext.clone(), a)).collect();

    let path = config_path();
    if let Ok(content) = fs::read_to_string(&path) {
        for line in content.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((ext, cmd)) = line.split_once('=') {
                let ext = ext.trim().trim_start_matches('.').to_lowercase();
                let cmd = cmd.trim().to_string();
                if let Some(entry) = map.get_mut(&ext) {
                    entry.command = cmd;
                } else {
                    map.insert(ext.clone(), AssociationEntry { ext: ext.clone(), desc: format!("{ext} File"), command: cmd });
                }
            }
        }
    }

    map.into_values().collect()
}

pub fn save_associations(entries: &[AssociationEntry]) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut lines = vec!["# FerroOS File Associations".to_string(), "# Format: extension = command %f".to_string()];
    for e in entries {
        lines.push(format!("{} = {}", e.ext.to_lowercase(), e.command));
    }
    let _ = fs::write(&path, lines.join("\n") + "\n");
}

/// Strips FreeDesktop field codes (%f, %F, %u, %U, %i, %c, %k, %v, %m, etc.)
pub fn strip_field_codes(exec: &str) -> String {
    let mut out = String::new();
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('%') {
            out.push('%');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_desktop_entry(path: &Path) -> Option<String> {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let content = fs::read_to_string(&resolved).or_else(|_| fs::read_to_string(path)).ok()?;
    let parent = resolved.parent().or_else(|| path.parent());
    parse_desktop_entry_content(&content, parent)
}

pub fn parse_desktop_entry_content(content: &str, working_dir: Option<&Path>) -> Option<String> {
    let mut in_entry = false;
    let mut exec: Option<String> = None;
    let mut terminal = false;
    let mut url: Option<String> = None;
    let mut entry_type = String::new();
    let mut path_dir: Option<String> = None;

    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "Type" => entry_type = value.to_string(),
            "Exec" => exec = Some(strip_field_codes(value)),
            "Terminal" => terminal = value.eq_ignore_ascii_case("true"),
            "URL" => url = Some(value.to_string()),
            "Path" => {
                if !value.is_empty() {
                    path_dir = Some(value.to_string())
                }
            }
            _ => {}
        }
    }

    if entry_type.eq_ignore_ascii_case("Link") {
        if let Some(u) = url {
            return Some(format!("xdg-open \"{}\"", u));
        }
    }

    let raw_exec = exec?;
    let trimmed = raw_exec.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_lowercase();
    let cmd = if lower == "ferro-cmd"
        || lower == "cmd"
        || lower == "terminal"
        || lower == "x-terminal-emulator"
        || (lower.contains("ferro-cmd") && !lower.contains("xterm") && !lower.contains("lxterminal"))
    {
        "lxterminal -t \"Command Prompt\" --geometry=80x24 -e \"stty erase \\x27^?\\x27 2>/dev/null; ferro-cmd || bash\" || xterm -T \"Command Prompt\" -bg \"#000000\" -fg \"#33FF33\" -cr \"#33FF33\" -fa \"DejaVu Sans Mono\" -fs 10 -e \"stty erase \\x27^?\\x27 2>/dev/null; ferro-cmd || bash\" || x-terminal-emulator || ferro-cmd".to_string()
    } else if lower.contains("librewolf") || lower.contains("firefox") || lower == "browser" {
        "librewolf || firefox-esr || firefox || x-www-browser".to_string()
    } else if terminal {
        format!("lxterminal -e \"{}\" || xterm -e \"{}\" || x-terminal-emulator -e \"{}\"", trimmed, trimmed, trimmed)
    } else {
        trimmed.to_string()
    };

    let full_cmd = if let Some(dir) = path_dir.or_else(|| working_dir.map(|p| p.to_string_lossy().into_owned())) {
        format!("cd \"{}\" && ({})", dir, cmd)
    } else {
        cmd
    };

    Some(full_cmd)
}

pub fn resolve_command_for_path(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let resolved_ext = resolved.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

    if ext == "desktop" || resolved_ext == "desktop" {
        if let Some(cmd) = parse_desktop_entry(path).or_else(|| parse_desktop_entry(&resolved)) {
            return cmd;
        }
    }

    if let Ok(head) = fs::read_to_string(&resolved).or_else(|_| fs::read_to_string(path)) {
        if head.trim_start().starts_with("[Desktop Entry]") {
            if let Some(cmd) = parse_desktop_entry_content(&head, resolved.parent().or_else(|| path.parent())) {
                return cmd;
            }
        }
    }

    let entries = load_associations();
    let p_str = path.to_string_lossy();

    for e in entries {
        if e.ext.eq_ignore_ascii_case(&ext) {
            if e.command.contains("%f") {
                return e.command.replace("%f", &format!("\"{p_str}\""));
            } else {
                return format!("{} \"{p_str}\"", e.command);
            }
        }
    }

    format!("xdg-open \"{p_str}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_associations_contain_required_apps() {
        let defaults = default_associations();
        let txt = defaults.iter().find(|a| a.ext == "txt").unwrap();
        assert!(txt.command.contains("geany"));

        let png = defaults.iter().find(|a| a.ext == "png").unwrap();
        assert!(png.command.contains("qview"));

        let mp4 = defaults.iter().find(|a| a.ext == "mp4").unwrap();
        assert!(mp4.command.contains("mpv"));

        let html = defaults.iter().find(|a| a.ext == "html").unwrap();
        assert!(html.command.contains("librewolf"));
    }

    #[test]
    fn resolve_command_replaces_placeholder() {
        let p = Path::new("/home/atom/notes.txt");
        let cmd = resolve_command_for_path(p);
        assert!(cmd.contains("/home/atom/notes.txt"));
        assert!(cmd.contains("geany"));
    }

    #[test]
    fn parse_desktop_entry_application_and_links() {
        let text_app = "[Desktop Entry]\nType=Application\nName=Steam\nExec=/usr/games/steam %U\nTerminal=false\n";
        let cmd = parse_desktop_entry_content(text_app, None).unwrap();
        assert_eq!(cmd, "/usr/games/steam");

        let text_term = "[Desktop Entry]\nType=Application\nName=Top\nExec=htop\nTerminal=true\n";
        let cmd_term = parse_desktop_entry_content(text_term, None).unwrap();
        assert!(cmd_term.contains("lxterminal -e \"htop\"") || cmd_term.contains("xterm"));

        let text_link = "[Desktop Entry]\nType=Link\nURL=https://debian.org\n";
        let cmd_link = parse_desktop_entry_content(text_link, None).unwrap();
        assert_eq!(cmd_link, "xdg-open \"https://debian.org\"");
    }
}
