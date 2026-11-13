//! What a script run in the progress window says, a line at a time. Most
//! lines are just what it prints (shown under Details); these are for the
//! window itself:
//!
//!   @ferro step TEXT                what it's doing now
//!   @ferro yesno QUESTION           Yes or No; answered `y` or `n`
//!   @ferro choose TITLE|A|B|...     one of a list; answered with its number
//!                                   (1 for A), or 0 for Cancel
//!   @ferro text PROMPT|SUGGESTED    a line typed; answered `=TEXT`, or `!`
//!   @ferro secret PROMPT            the same, the typing hidden
//!   @ferro done TEXT                finished, and how it went
//!   @ferro fail TEXT                stopped, and why
//!
//! and apt's own progress (APT::Status-Fd=1): `pmstatus:PKG:PERCENT:WHAT`
//! while installing, `dlstatus:N:PERCENT:WHAT` while downloading. Each
//! answer is one line on the script's stdin.

#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    Step(String),
    YesNo(String),
    Choose(String, Vec<String>),
    Text {
        prompt: String,
        suggested: String,
        secret: bool,
    },
    Done(String),
    Fail(String),
    /// apt: how far (0 to 100), and what it's at.
    Progress(f32, String),
    Log(String),
}

/// A line as it may be shown: no control characters (colours, a carriage
/// return's leftovers), tabs as spaces.
pub fn printable(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // An escape sequence (colours): up to its final letter.
            '\u{1b}' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for d in chars.by_ref() {
                        if d.is_ascii_alphabetic() || d == '~' {
                            break;
                        }
                    }
                }
            }
            '\t' => out.push_str("    "),
            '\r' => out.clear(),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.trim_end().to_owned()
}

pub fn parse(line: &str) -> Said {
    let line = printable(line);
    if let Some(rest) = line.strip_prefix("@ferro ") {
        let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        let arg = arg.trim().to_owned();
        return match verb {
            "step" => Said::Step(arg),
            "yesno" => Said::YesNo(arg),
            "choose" => {
                let mut parts = arg.split('|').map(|p| p.trim().to_owned());
                let title = parts.next().unwrap_or_default();
                Said::Choose(title, parts.filter(|p| !p.is_empty()).collect())
            }
            "text" | "secret" => {
                let (prompt, suggested) = arg.split_once('|').unwrap_or((&arg, ""));
                Said::Text { prompt: prompt.trim().to_owned(), suggested: suggested.to_owned(), secret: verb == "secret" }
            }
            "done" => Said::Done(arg),
            "fail" => Said::Fail(arg),
            _ => Said::Log(line.clone()),
        };
    }
    for kind in ["pmstatus:", "dlstatus:"] {
        if let Some(rest) = line.strip_prefix(kind) {
            let f: Vec<&str> = rest.splitn(3, ':').collect();
            if let [_, pct, what] = f[..] {
                if let Ok(p) = pct.parse::<f32>() {
                    return Said::Progress(p.clamp(0.0, 100.0), what.to_owned());
                }
            }
        }
    }
    Said::Log(line)
}

/// `text` in lines of at most `cols` characters, broken between words (a
/// word longer than a line, at the line's end).
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ').filter(|w| !w.is_empty()) {
            let mut word = word.to_owned();
            loop {
                let have = line.chars().count();
                let need = word.chars().count() + usize::from(have > 0);
                if have + need <= cols {
                    if have > 0 {
                        line.push(' ');
                    }
                    line.push_str(&word);
                    break;
                }
                if have > 0 {
                    lines.push(std::mem::take(&mut line));
                    continue;
                }
                let cut: String = word.chars().take(cols).collect();
                word = word.chars().skip(cols).collect();
                lines.push(cut);
                if word.is_empty() {
                    break;
                }
            }
        }
        lines.push(line);
    }
    while lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// A Debian package name as `apt-get install` takes one (and nothing a
/// command line could take for something else).
pub fn is_package_name(name: &str) -> bool {
    let b = name.as_bytes();
    (2..=100).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"+-.:".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_words() {
        assert_eq!(parse("@ferro step Downloading Steam"), Said::Step("Downloading Steam".into()));
        assert_eq!(parse("@ferro yesno Install it?"), Said::YesNo("Install it?".into()));
        assert_eq!(
            parse("@ferro choose Which folder?|Games|Internet|"),
            Said::Choose("Which folder?".into(), vec!["Games".into(), "Internet".into()])
        );
        assert_eq!(parse("@ferro text Name for it|My Game"), Said::Text { prompt: "Name for it".into(), suggested: "My Game".into(), secret: false });
        assert_eq!(
            parse("@ferro secret One-time password"),
            Said::Text { prompt: "One-time password".into(), suggested: String::new(), secret: true }
        );
        assert_eq!(parse("@ferro done Installed."), Said::Done("Installed.".into()));
        assert_eq!(parse("@ferro fail No room."), Said::Fail("No room.".into()));
        assert_eq!(parse("@ferro dance"), Said::Log("@ferro dance".into()));
    }

    #[test]
    fn apts_progress() {
        assert_eq!(parse("pmstatus:libc6:42.8571:Installing libc6 (amd64)"), Said::Progress(42.8571, "Installing libc6 (amd64)".into()));
        assert_eq!(parse("dlstatus:3:9.5:Retrieving file 3 of 30"), Said::Progress(9.5, "Retrieving file 3 of 30".into()));
        assert_eq!(parse("pmstatus:dpkg-exec:120:x"), Said::Progress(100.0, "x".into()));
        assert_eq!(parse("pmstatus:broken"), Said::Log("pmstatus:broken".into()));
    }

    #[test]
    fn lines_as_shown() {
        assert_eq!(printable("\u{1b}[31mError\u{1b}[0m:\tgone\r\n"), "");
        assert_eq!(printable("\u{1b}[31mError\u{1b}[0m:\tgone"), "Error:    gone");
        assert_eq!(printable("50%\r100%"), "100%");
    }

    #[test]
    fn wraps_between_words() {
        assert_eq!(wrap("Install Steam now? It asks first", 12), ["Install", "Steam now?", "It asks", "first"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("one\ntwo", 10), ["one", "two"]);
        assert_eq!(wrap("", 10), [""]);
    }

    #[test]
    fn package_names() {
        assert!(is_package_name("vlc"));
        assert!(is_package_name("libc6:i386"));
        assert!(is_package_name("g++"));
        for bad in ["", "x", "-y", "--option=1", "VLC", "a b", "a;b", "../x"] {
            assert!(!is_package_name(bad), "{bad}");
        }
    }
}
