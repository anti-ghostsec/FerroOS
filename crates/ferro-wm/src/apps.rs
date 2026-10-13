//! The programs on the Start menu: every installed program that describes
//! itself with a `.desktop` file, which is how Debian packages (and Steam,
//! and Steam's games) announce themselves.

/// One Start menu entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub name: String,
    /// The command line, ready for `sh -c`.
    pub exec: String,
    /// Wants a terminal window to run in.
    pub terminal: bool,
    pub category: String,
}

/// Reads one `.desktop` file. `None` for entries that aren't programs to
/// show (hidden ones, links, folders).
pub fn parse_desktop(text: &str) -> Option<App> {
    let mut in_entry = false;
    let (mut name, mut exec, mut terminal, mut raw_cat) = (None, None, false, String::new());
    for line in text.lines().map(str::trim) {
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
            "Name" => name = Some(value.to_owned()),
            "Exec" => exec = Some(strip_field_codes(value)),
            "Terminal" => terminal = value == "true",
            "Categories" => raw_cat = value.to_lowercase(),
            "Type" if value != "Application" => return None,
            "NoDisplay" | "Hidden" if value == "true" => return None,
            _ => {}
        }
    }
    let (name, exec) = (name?, exec?);
    let name_low = name.to_lowercase();
    let category = if raw_cat.contains("game") || name_low.contains("steam") {
        "Games".to_string()
    } else if raw_cat.contains("system") || raw_cat.contains("settings") || name_low.contains("task manager") || name_low.contains("install") || name_low.contains("add programs") || name_low.contains("command prompt") {
        "System Tools".to_string()
    } else if name_low.contains("computer") || name_low.contains("documents") || name_low.contains("files") {
        "Files".to_string()
    } else if raw_cat.contains("development") || raw_cat.contains("ide") {
        "Development".to_string()
    } else {
        "Accessories".to_string()
    };
    (!name.is_empty() && !exec.is_empty()).then_some(App { name, exec, terminal, category })
}

/// `%f`, `%U` and the like stand for files to open; started from the menu
/// there are none.
fn strip_field_codes(exec: &str) -> String {
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

/// Every program in `dirs`, by name. Later directories don't override
/// earlier ones (a user's own entry wins over the system's).
pub fn installed<P: AsRef<std::path::Path>>(dirs: &[P]) -> Vec<App> {
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        for entry in std::fs::read_dir(dir.as_ref()).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if !file.ends_with(".desktop") || !seen.insert(file) {
                continue;
            }
            if let Some(app) = std::fs::read_to_string(entry.path()).ok().and_then(|t| parse_desktop(&t)) {
                apps.push(app);
            }
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.name == b.name);
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_program() {
        let app = parse_desktop("[Desktop Entry]\nType=Application\nName=Steam\nName[de]=Dampf\nExec=/usr/games/steam %U\nTerminal=false\n\n[Desktop Action Library]\nName=Library\nExec=steam steam://open/games\n").unwrap();
        assert_eq!(app, App { name: "Steam".into(), exec: "/usr/games/steam".into(), terminal: false, category: "Games".into() });
    }

    #[test]
    fn skips_what_is_not_a_program_to_show() {
        assert_eq!(parse_desktop("[Desktop Entry]\nType=Application\nName=Helper\nExec=helper\nNoDisplay=true\n"), None);
        assert_eq!(parse_desktop("[Desktop Entry]\nType=Link\nName=Site\nURL=https://example.org\n"), None);
        assert_eq!(parse_desktop("[Desktop Entry]\nName=Nothing to run\n"), None);
    }

    #[test]
    fn drops_file_placeholders() {
        assert_eq!(strip_field_codes("vim %F"), "vim");
        assert_eq!(strip_field_codes("sh -c \"echo 100%% %u\""), "sh -c \"echo 100% \"");
        let app = parse_desktop("[Desktop Entry]\nName=Top\nExec=htop\nTerminal=true\n").unwrap();
        assert!(app.terminal);
    }
}
