//! Share: a program hands a file, some text or a link to another program
//! the user picks, as Android's Share button does, but private first.
//!
//! - The program sharing only opens the Share box (`ferro-share`, or this
//!   crate's `share_*`). It isn't told which programs could take it, which
//!   one was picked, or whether anything was: the box always ends the same.
//! - Only the user picks, in the box. There's no way to name a program to
//!   share with, no list of recent ones and no suggested people: nothing is
//!   remembered between one share and the next.
//! - The program picked gets a copy, read-only, in memory: a folder of its
//!   own under $XDG_RUNTIME_DIR (RAM, and FerroOS's only swap is zram, also
//!   RAM). It never touches the disk, and the program never learns where
//!   the file came from. When it has been read (or ten minutes after the
//!   program closed without reading it), it's gone. Where $XDG_RUNTIME_DIR
//!   isn't in memory, nothing is shared.
//! - Pictures go without what they carry about where and with what they
//!   were taken (EXIF, GPS, XMP: ferro-meta), unless the user unticks it.
//!
//! The programs offered are the ones whose `.desktop` entry says they open
//! that kind of file (`MimeType=`) and takes a file or link (`%f`, `%F`,
//! `%u` or `%U` in `Exec=`), as Open With has them. `X-Ferro-Share=false`
//! keeps a program out.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The Share box, by its full path: not whatever is first on a PATH.
pub const BIN: &str = "/usr/local/bin/ferro-share";
/// The most one share may hold: it's all in memory.
pub const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Opens the Share box for these files. Returns at once; nothing comes back.
pub fn share_files(paths: &[&Path]) -> std::io::Result<()> {
    let mut cmd = Command::new(BIN);
    cmd.arg("--").args(paths);
    spawn(cmd, None)
}

/// Opens the Share box for `data`, offered as a file named `name` (its
/// extension says what it is: `photo.jpg`). It goes to the box through a
/// pipe, never through a file.
pub fn share_bytes(name: &str, data: &[u8]) -> std::io::Result<()> {
    let mut cmd = Command::new(BIN);
    cmd.args(["--stdin", name]);
    spawn(cmd, Some(data))
}

/// Opens the Share box for some text.
pub fn share_text(text: &str) -> std::io::Result<()> {
    let mut cmd = Command::new(BIN);
    cmd.args(["--text", "-"]);
    spawn(cmd, Some(text.as_bytes()))
}

/// Opens the Share box for a link (`https://...`).
pub fn share_url(url: &str) -> std::io::Result<()> {
    let mut cmd = Command::new(BIN);
    cmd.args(["--url", url]);
    spawn(cmd, None)
}

fn spawn(mut cmd: Command, input: Option<&[u8]>) -> std::io::Result<()> {
    cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = cmd.spawn()?;
    if let (Some(data), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(data)?;
    }
    // Waited for aside, so it isn't left a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// What's being shared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Share {
    /// Files (or text, as `Shared text.txt`), held in memory.
    Files(Vec<Item>),
    /// A link, handed over as it is.
    Url(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Its name: no folder, nothing that says where it came from.
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
}

impl Item {
    pub fn new(name: &str, data: Vec<u8>) -> Item {
        let name = safe_name(name);
        let mime = mime_for(&name, &data).to_owned();
        Item { name, mime, data }
    }

    /// A picture ferro-meta can take what it carries out of.
    pub fn strippable(&self) -> bool {
        ferro_meta::detect(&self.data).is_some()
    }
}

impl Share {
    pub fn text(text: &str) -> Share {
        Share::Files(vec![Item { name: "Shared text.txt".into(), mime: "text/plain".into(), data: text.as_bytes().to_vec() }])
    }

    /// The link, if it's one: a scheme of letters (so never an `-option`).
    pub fn url(url: &str) -> Result<Share, String> {
        let url = url.trim();
        match scheme(url) {
            Some(_) if !url.contains(char::is_whitespace) => Ok(Share::Url(url.to_owned())),
            _ => Err(format!("'{url}' isn't a link.")),
        }
    }

    /// What the box says it's sharing.
    pub fn describe(&self) -> String {
        match self {
            Share::Url(_) => "this link".into(),
            Share::Files(items) if items.len() == 1 && items[0].name == "Shared text.txt" => "this text".into(),
            Share::Files(items) if items.len() == 1 => format!("'{}'", items[0].name),
            Share::Files(items) => format!("{} items", items.len()),
        }
    }

    /// The kinds a program must take to be offered.
    fn kinds(&self) -> Vec<String> {
        match self {
            Share::Url(u) => vec![format!("x-scheme-handler/{}", scheme(u).unwrap_or_default())],
            Share::Files(items) => items.iter().map(|i| i.mime.clone()).collect(),
        }
    }
}

fn scheme(url: &str) -> Option<String> {
    let (s, rest) = url.split_once(':')?;
    let ok =
        s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) && !rest.is_empty();
    ok.then(|| s.to_ascii_lowercase())
}

/// A file name that is only a name: no folder, not `.` or `..`, not empty,
/// no control characters, not too long.
pub fn safe_name(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or("");
    let clean: String = base.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim().trim_start_matches('.').to_owned();
    if clean.is_empty() {
        return "Shared file".into();
    }
    // At most 200 bytes, on a character boundary, keeping the extension.
    if clean.len() <= 200 {
        return clean;
    }
    let ext = clean.rsplit_once('.').map(|(_, e)| e).filter(|e| e.len() <= 10).unwrap_or("");
    let mut cut = 200 - ext.len() - 1;
    while !clean.is_char_boundary(cut) {
        cut -= 1;
    }
    if ext.is_empty() {
        clean[..cut].to_owned()
    } else {
        format!("{}.{ext}", &clean[..cut])
    }
}

/// What kind of file this is: by its first bytes for pictures, else by its
/// extension; text that's not known otherwise is `text/plain`.
pub fn mime_for(name: &str, data: &[u8]) -> &'static str {
    match ferro_meta::detect(data) {
        Some(ferro_meta::Format::Jpeg) => return "image/jpeg",
        Some(ferro_meta::Format::Png) => return "image/png",
        Some(ferro_meta::Format::WebP) => return "image/webp",
        None => {}
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if let Some(known) = kind_of_ext(&ext) {
        known
    } else if std::str::from_utf8(data).is_ok_and(|t| !t.contains('\0')) {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

/// What kind of file an extension (lower case, no dot) says it is, if it's
/// one known here.
pub fn kind_of_ext(ext: &str) -> Option<&'static str> {
    let known = match ext {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" | "jfif" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "tif" | "tiff" => "image/tiff",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "flac" => "audio/flac",
        "wav" => "audio/x-wav",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "html" | "htm" => "text/html",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "txt" | "log" | "conf" | "ini" | "cfg" => "text/plain",
        "rs" => "text/x-rust",
        "c" => "text/x-csrc",
        "h" => "text/x-chdr",
        "cpp" | "cc" | "cxx" => "text/x-c++src",
        "py" => "text/x-python",
        "css" => "text/css",
        "sh" | "bash" => "application/x-shellscript",
        "js" | "mjs" => "application/javascript",
        "ts" => "text/x-typescript",
        "xml" => "application/xml",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/x-yaml",
        "exe" | "msi" => "application/x-ms-dos-executable",
        _ => return None,
    };
    Some(known)
}

/// Kinds that are text underneath (freedesktop.org's subclasses of
/// text/plain): a program that opens text opens them too.
fn is_text(kind: &str) -> bool {
    kind.starts_with("text/")
        || matches!(
            kind,
            "application/json"
                | "application/javascript"
                | "application/x-shellscript"
                | "application/xml"
                | "application/toml"
                | "application/x-yaml"
        )
}

/// A program that can be shared with: from its `.desktop` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub exec: String,
    pub kinds: Vec<String>,
}

impl Target {
    /// Whether it takes `kind`: named, as `image/*`, or (for text of any
    /// kind) as `text/plain`.
    fn takes(&self, kind: &str) -> bool {
        let major = kind.split('/').next().unwrap_or("");
        self.kinds.iter().any(|k| {
            k.eq_ignore_ascii_case(kind)
                || k.strip_suffix("/*").is_some_and(|m| m.eq_ignore_ascii_case(major))
                || (k.eq_ignore_ascii_case("text/plain") && is_text(kind))
        })
    }

    /// Its `Exec=` as a command for Open With and File Associations (one
    /// file, `%f`; words with spaces quoted for the shell).
    pub fn open_with(&self) -> String {
        let quote = |w: &str| {
            if w.is_empty() || w.contains(|c: char| c.is_whitespace() || "'\"\\$`;&|<>()*?[]{}#~".contains(c)) {
                format!("'{}'", w.replace('\'', "'\\''"))
            } else {
                w.to_owned()
            }
        };
        let mut out: Vec<String> = Vec::new();
        for w in split_exec(&self.exec) {
            if matches!(w.as_str(), "%f" | "%F" | "%u" | "%U") {
                if !out.iter().any(|o| o == "%f") {
                    out.push("%f".into());
                }
            } else {
                // A file code inside a word (`--file=%f`) stays where it is.
                let w = expand(&w, Some("%f"));
                if !w.is_empty() {
                    out.push(quote(&w));
                }
            }
        }
        out.join(" ")
    }

    /// The command lines that hand it `files` (or the link): `%F`/`%U` take
    /// them all at once, `%f`/`%u` one each, so one start per file.
    pub fn commands(&self, files: &[String]) -> Vec<Vec<String>> {
        let words = split_exec(&self.exec);
        let all = words.iter().any(|w| w.contains("%F") || w.contains("%U"));
        let one_each = |file: Option<&str>| -> Vec<String> {
            let mut argv = Vec::new();
            for w in &words {
                if w == "%F" || w == "%U" {
                    argv.extend(files.iter().cloned());
                } else if w == "%f" || w == "%u" {
                    argv.extend(file.map(str::to_owned));
                } else {
                    let expanded = expand(w, file);
                    if !expanded.is_empty() {
                        argv.push(expanded);
                    }
                }
            }
            argv
        };
        if all {
            vec![one_each(None)]
        } else {
            files.iter().map(|f| one_each(Some(f))).collect()
        }
    }
}

/// A word of `Exec=` with its field codes filled in (or dropped).
fn expand(word: &str, file: Option<&str>) -> String {
    let mut out = String::new();
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('f' | 'u') => out.push_str(file.unwrap_or("")),
            _ => {}
        }
    }
    out
}

/// `Exec=` split into words, as freedesktop.org quotes them ("..." with
/// \" \` \$ \\ inside).
pub fn split_exec(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut any) = (false, false);
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            '\\' if quoted => word.extend(chars.next()),
            c if c.is_whitespace() && !quoted => {
                if any || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                any = false;
            }
            c => word.push(c),
        }
    }
    if any || !word.is_empty() {
        words.push(word);
    }
    words
}

/// A `.desktop` entry as a program to share with, if it is one.
pub fn parse_entry(content: &str) -> Option<Target> {
    parse_program(content, true)
}

/// A `.desktop` entry as a program that opens files, if it is one; for
/// Share (`share`), not one that keeps out of it (`X-Ferro-Share=false`).
fn parse_program(content: &str, share: bool) -> Option<Target> {
    let (mut name, mut exec, mut kinds) = (None, None, Vec::new());
    let (mut app, mut hidden, mut out) = (false, false, false);
    let mut in_entry = false;
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        let Some((k, v)) = line.split_once('=').filter(|_| in_entry) else { continue };
        let v = v.trim();
        match k.trim() {
            "Type" => app = v == "Application",
            "Name" => name = Some(v.to_owned()),
            "Exec" => exec = Some(v.to_owned()),
            "MimeType" => kinds = v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            "Hidden" => hidden = v.eq_ignore_ascii_case("true"),
            "X-Ferro-Share" => out = v.eq_ignore_ascii_case("false"),
            _ => {}
        }
    }
    let exec = exec?;
    let takes_files = ["%f", "%F", "%u", "%U"].iter().any(|c| exec.contains(c));
    let out = out && share;
    (app && !hidden && !out && takes_files && !kinds.is_empty() && !exec.contains("ferro-share")).then(|| Target {
        name: name.unwrap_or_default(),
        exec,
        kinds,
    })
}

/// Where `.desktop` entries are, the user's first (theirs replaces the
/// system's of the same name).
pub fn entry_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    dirs.push("/usr/local/share/applications".into());
    dirs.push("/usr/share/applications".into());
    dirs
}

/// The programs that open files of `kind` (Open With), by name.
pub fn programs(kind: &str, dirs: &[PathBuf]) -> Vec<Target> {
    find(&[kind.to_owned()], dirs, false)
}

/// The programs that take everything in `share`, by name.
pub fn targets(share: &Share, dirs: &[PathBuf]) -> Vec<Target> {
    find(&share.kinds(), dirs, true)
}

fn find(kinds: &[String], dirs: &[PathBuf], share: bool) -> Vec<Target> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Target> = Vec::new();
    for dir in dirs {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")).collect();
        files.sort();
        for p in files {
            let id = p.file_name().map(|n| n.to_owned()).unwrap_or_default();
            if !seen.insert(id) {
                continue;
            }
            let Some(t) = fs::read_to_string(&p).ok().and_then(|c| parse_program(&c, share)) else { continue };
            if kinds.iter().all(|k| t.takes(k)) && !out.iter().any(|o| o.name == t.name) {
                out.push(t);
            }
        }
    }
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

/// Whether `dir` is held in memory (tmpfs or ramfs), never on a disk.
#[cfg(target_os = "linux")]
pub fn in_memory(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    const TMPFS: i64 = 0x0102_1994;
    const RAMFS: i64 = 0x8584_58f6;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: a zeroed statfs is a valid buffer; the path lives through the call.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    #[allow(clippy::unnecessary_cast)]
    let t = st.f_type as i64;
    t == TMPFS || t == RAMFS
}

#[cfg(not(target_os = "linux"))]
pub fn in_memory(_dir: &Path) -> bool {
    false
}

/// The folder shares are put in, in memory; `None` if there's none.
pub fn share_root() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    in_memory(&runtime).then(|| runtime.join("ferro-share"))
}

/// Puts `items` in a new folder of their own under `root` (0700, the files
/// 0400), with names that don't clash. Returns the folder and the files.
pub fn stage(root: &Path, items: &[Item]) -> std::io::Result<(PathBuf, Vec<PathBuf>)> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    fs::DirBuilder::new().mode(0o700).recursive(true).create(root)?;
    let dir = root.join(random_name()?);
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let mut paths = Vec::new();
    for item in items {
        let mut name = item.name.clone();
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
            _ => (name.clone(), String::new()),
        };
        let mut n = 2;
        while paths.iter().any(|p: &PathBuf| p.file_name().is_some_and(|f| f == name.as_str())) {
            name = format!("{stem} ({n}){ext}");
            n += 1;
        }
        let path = dir.join(&name);
        let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        f.write_all(&item.data)?;
        drop(f);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
        paths.push(path);
    }
    Ok((dir, paths))
}

/// 128 random bits, as hex: a folder name nothing can guess.
fn random_name() -> std::io::Result<String> {
    let mut b = [0u8; 16];
    fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Removes shares left behind (the box killed before it could): a day old.
pub fn sweep(root: &Path) {
    let Ok(rd) = fs::read_dir(root) else { return };
    let day = std::time::Duration::from_secs(24 * 3600);
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > day);
        if old {
            remove(&e.path());
        }
    }
}

/// Deletes a share folder: its files are read-only, the folder isn't.
pub fn remove(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(kinds: &[&str], exec: &str) -> Target {
        Target { name: "T".into(), exec: exec.into(), kinds: kinds.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn names_are_only_names() {
        assert_eq!(safe_name("/home/nully/Pictures/cat.jpg"), "cat.jpg");
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name(".."), "Shared file");
        assert_eq!(safe_name(""), "Shared file");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("a\nb\u{7}.txt"), "ab.txt");
        let long = format!("{}.png", "x".repeat(300));
        let s = safe_name(&long);
        assert!(s.len() <= 200 && s.ends_with(".png"));
    }

    #[test]
    fn kinds_by_content_then_name() {
        assert_eq!(mime_for("x.txt", b"\x89PNG\r\n\x1a\nrest"), "image/png");
        assert_eq!(mime_for("x.mp4", b"\0\0\0"), "video/mp4");
        assert_eq!(mime_for("notes", b"hello"), "text/plain");
        assert_eq!(mime_for("blob", b"\0\x01"), "application/octet-stream");
    }

    #[test]
    fn exec_words_and_field_codes() {
        assert_eq!(split_exec(r#"mpv --mode="pseudo gui" -- %U"#), ["mpv", "--mode=pseudo gui", "--", "%U"]);
        assert_eq!(split_exec(r#""/opt/My App/app" %f"#), ["/opt/My App/app", "%f"]);
        let files = vec!["/a".to_string(), "/b".to_string()];
        assert_eq!(target(&[], "geany %F").commands(&files), vec![vec!["geany", "/a", "/b"]]);
        assert_eq!(target(&[], "view %f --x %i").commands(&files), vec![vec!["view", "/a", "--x"], vec!["view", "/b", "--x"]]);
        assert_eq!(target(&[], "app --open=%u").commands(&files[..1]), vec![vec!["app", "--open=/a"]]);
    }

    #[test]
    fn entries_that_can_be_shared_with() {
        let e = "[Desktop Entry]\nType=Application\nName=Viewer\nExec=viewer %f\nMimeType=image/*;\n";
        let t = parse_entry(e).unwrap();
        assert!(t.takes("image/png") && !t.takes("text/plain"));
        assert!(parse_entry("[Desktop Entry]\nType=Application\nName=X\nExec=x\nMimeType=image/png;\n").is_none(), "takes no file");
        assert!(parse_entry(&format!("{e}X-Ferro-Share=false\n")).is_none());
        assert!(parse_entry(&format!("{e}Hidden=true\n")).is_none());
        assert!(parse_entry("[Desktop Entry]\nType=Application\nName=X\nExec=x %f\n").is_none(), "no kinds");
        assert!(parse_entry("[Other]\nType=Application\nExec=x %f\nMimeType=a/b;\n").is_none());
    }

    #[test]
    fn open_with_commands_and_text_kinds() {
        assert_eq!(target(&[], "geany %F").open_with(), "geany %f");
        assert_eq!(target(&[], r#""/opt/My App/app" --new %U %i"#).open_with(), "'/opt/My App/app' --new %f");
        assert_eq!(target(&[], "app --file=%u").open_with(), "app --file=%f");
        let editor = target(&["text/plain"], "e %f");
        assert!(editor.takes("text/x-rust") && editor.takes("application/json") && !editor.takes("image/png"));
        assert_eq!(kind_of_ext("rs"), Some("text/x-rust"));
        assert_eq!(kind_of_ext("weird"), None);
        let out = "[Desktop Entry]\nType=Application\nName=X\nExec=x %f\nMimeType=a/b;\nX-Ferro-Share=false\n";
        assert!(parse_entry(out).is_none() && parse_program(out, false).is_some(), "out of Share, still in Open With");
    }

    #[test]
    fn links() {
        assert_eq!(Share::url(" https://example.org/a "), Ok(Share::Url("https://example.org/a".into())));
        assert!(Share::url("-oProxy=x:y").is_err());
        assert!(Share::url("/etc/passwd").is_err());
        assert!(Share::url("https://a b").is_err());
        assert_eq!(Share::Url("HTTPS://x".into()).kinds(), ["x-scheme-handler/https"]);
    }

    #[test]
    fn targets_take_everything_shared() {
        let dir = std::env::temp_dir().join(format!("ferro-share-apps-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("v.desktop"), "[Desktop Entry]\nType=Application\nName=Viewer\nExec=v %f\nMimeType=image/*;\n").unwrap();
        fs::write(dir.join("e.desktop"), "[Desktop Entry]\nType=Application\nName=Editor\nExec=e %F\nMimeType=text/plain;\n").unwrap();
        let pic = Share::Files(vec![Item::new("a.png", b"\x89PNG\r\n\x1a\n".to_vec())]);
        assert_eq!(targets(&pic, std::slice::from_ref(&dir)).iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Viewer"]);
        let both = Share::Files(vec![Item::new("a.png", b"\x89PNG\r\n\x1a\n".to_vec()), Item::new("b.txt", b"hi".to_vec())]);
        assert!(targets(&both, std::slice::from_ref(&dir)).is_empty());
        assert_eq!(targets(&Share::text("hi"), std::slice::from_ref(&dir))[0].name, "Editor");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn staged_copies_are_read_only_and_kept_apart() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("ferro-share-stage-{}", std::process::id()));
        let items = [Item::new("a.txt", b"one".to_vec()), Item::new("a.txt", b"two".to_vec())];
        let (dir, paths) = stage(&root, &items).unwrap();
        assert_eq!(paths[0].file_name().unwrap(), "a.txt");
        assert_eq!(paths[1].file_name().unwrap(), "a (2).txt");
        assert_eq!(fs::read(&paths[1]).unwrap(), b"two");
        assert_eq!(fs::metadata(&paths[0]).unwrap().permissions().mode() & 0o777, 0o400);
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        remove(&dir);
        assert!(!dir.exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_runtime_folder_is_in_memory() {
        assert!(in_memory(Path::new("/dev/shm")) || !Path::new("/dev/shm").exists());
        assert!(!in_memory(Path::new("/nonexistent-ferro")));
    }
}
