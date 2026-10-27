//! Metadata: what a picture or video carries besides itself (camera, place,
//! dates, names, programs, comments...). Read with ExifTool, which knows
//! nearly every format; changed with ExifTool, or for videos it can't write,
//! with ffmpeg; stripped with ExifTool (pictures: everything but the colour
//! profile) or ffmpeg (videos: copied over without it, picture and sound
//! untouched). Both run inside ferro-jail: a booby-trapped file that took
//! them over couldn't reach the internet or start anything.
//!
//! The panel that shows it is shared: the Info window (Image Viewer, Video
//! Player) and the Info tab in Properties draw the same thing.

use ferro_gfx::{color, ellipsize, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, text_field_with_cursor, Key};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One thing the file carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// Where in the file it is, as ExifTool names it: `IFD0`, `GPS`, `XMP-dc`...
    pub group: String,
    pub tag: String,
    pub value: String,
}

impl Field {
    /// ExifTool's own notes and what the file system says are shown, not
    /// changed: they aren't in the file.
    pub fn editable(&self) -> bool {
        !matches!(self.group.as_str(), "ExifTool" | "System" | "File" | "Composite") && !self.value.starts_with("(Binary data")
    }
}

const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "avi", "mov", "flv", "wmv", "mpg", "mpeg", "ts", "m2ts", "mts", "3gp", "ogv", "vob", "mp3", "m4a", "flac", "ogg",
    "opus", "wav",
];

/// A video or sound file (stripped by copying it over with ffmpeg).
pub fn is_video(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    VIDEO_EXTS.contains(&ext.as_str())
}

/// Runs a metadata tool inside ferro-jail (straight, where FerroOS's jail
/// isn't installed, as when building elsewhere): what it printed, or why not.
fn run_jailed(program: &str, extra_exec: &[&str], args: &[&str]) -> Result<String, String> {
    let jailed = Command::new("ferro-jail").args(extra_exec.iter().flat_map(|e| ["--exec", *e])).arg(program).args(args).output();
    let out = match jailed {
        Ok(o) => o,
        Err(_) => Command::new(program).args(args).output().map_err(|_| format!("{program} isn't installed."))?,
    };
    let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else if out.status.code() == Some(127) {
        Err(format!("{program} isn't installed."))
    } else {
        Err(if err.is_empty() { format!("{program} couldn't do it.") } else { first_line(&err) })
    }
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or(s).trim().to_owned()
}

fn exiftool(args: &[&str]) -> Result<String, String> {
    // ExifTool is a Perl program: Perl is what actually runs.
    run_jailed("exiftool", &["/usr/bin/perl"], args)
}

/// `-Group:Tag=Value` lines, as `exiftool -args -G1` prints them.
pub fn parse_args(out: &str) -> Vec<Field> {
    out.lines()
        .filter_map(|l| l.strip_prefix('-'))
        .filter_map(|l| {
            let (name, value) = l.split_once('=')?;
            let (group, tag) = name.split_once(':').unwrap_or(("", name));
            Some(Field { group: group.to_owned(), tag: tag.to_owned(), value: value.to_owned() })
        })
        .collect()
}

/// Everything `path` carries that ExifTool can find.
pub fn read(path: &Path) -> Result<Vec<Field>, String> {
    let p = path.to_string_lossy();
    let out = exiftool(&["-args", "-G1", "-a", "-s", "-charset", "filename=utf8", "-api", "LargeFileSupport=1", "--", &p])?;
    Ok(parse_args(&out))
}

/// Copies a video over itself with ffmpeg, `extra` deciding what's kept:
/// streams copied as they are, never re-encoded.
fn remux(path: &Path, extra: &[&str]) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mkv");
    let tmp = dir.join(format!(".ferro-info-{}.{ext}", std::process::id()));
    let (src, dst) = (path.to_string_lossy(), tmp.to_string_lossy());
    let mut args = vec!["-nostdin", "-loglevel", "error", "-y", "-i", &src, "-map", "0", "-c", "copy"];
    args.extend_from_slice(extra);
    args.push(&dst);
    let result = run_jailed("ffmpeg", &[], &args).and_then(|_| std::fs::rename(&tmp, path).map_err(|e| format!("It couldn't be replaced: {e}")));
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Changes one thing (`None`: removes it).
pub fn set(path: &Path, field: &Field, value: Option<&str>) -> Result<(), String> {
    let v = value.unwrap_or("");
    let arg = format!("-{}:{}={}", field.group, field.tag, v);
    let p = path.to_string_lossy();
    match exiftool(&["-overwrite_original", "-P", &arg, "--", &p]) {
        Ok(out) if out.contains("0 image files updated") && !is_video(path) => Err("It can't be changed in this kind of file.".into()),
        Ok(out) if !out.contains("0 image files updated") => Ok(()),
        // What ExifTool can't write in a video, ffmpeg can (container tags).
        _ if is_video(path) => {
            let meta = format!("{}={v}", field.tag.to_lowercase());
            remux(path, &["-map_metadata", "0", "-metadata", &meta])
        }
        Ok(_) => Err("It can't be changed in this kind of file.".into()),
        Err(e) => Err(e),
    }
}

/// Removes all of it: from a picture everything but its colour profile
/// (without which its colours would change); from a video everything, its
/// chapters too, the picture and sound copied over untouched.
pub fn strip(path: &Path) -> Result<(), String> {
    if is_video(path) {
        return remux(
            path,
            &[
                "-map_metadata",
                "-1",
                "-map_metadata:s",
                "-1",
                "-map_chapters",
                "-1",
                "-fflags",
                "+bitexact",
                "-flags:v",
                "+bitexact",
                "-flags:a",
                "+bitexact",
            ],
        );
    }
    let p = path.to_string_lossy();
    exiftool(&["-overwrite_original", "-P", "-all=", "--icc_profile:all", "--", &p]).map(drop)
}

/// The list as text (Open as Text, and copying).
pub fn as_text(path: &Path, fields: &[Field]) -> String {
    let mut out = format!("{}\n\n", path.display());
    for f in fields {
        out.push_str(&format!("[{}] {}: {}\n", f.group, f.tag, f.value));
    }
    out
}

const ROW_H: i32 = GLYPH_H + 6;
const BTN_W: i32 = 78;
const BTN_H: i32 = 24;

/// What the panel asks of the window it's in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Redraw,
    /// Show this text (Open as Text).
    OpenText(String),
}

/// The metadata panel: the list, and Edit, Remove, Strip All, Open as Text.
pub struct Panel {
    pub path: PathBuf,
    pub fields: Vec<Field>,
    selected: Option<usize>,
    scroll: usize,
    /// Typing a new value: the text and the cursor.
    editing: Option<(String, usize)>,
    confirm_strip: bool,
    status: String,
    error: bool,
}

impl Panel {
    /// A panel showing `fields` as they are, not read from anything (the
    /// preview pictures and tests).
    pub fn with_fields(path: &Path, fields: Vec<Field>) -> Self {
        Panel {
            path: path.to_path_buf(),
            fields,
            selected: Some(1),
            scroll: 0,
            editing: None,
            confirm_strip: false,
            status: "6 things in the file; 3 can be changed.".into(),
            error: false,
        }
    }

    pub fn new(path: &Path) -> Self {
        let mut p = Panel {
            path: path.to_path_buf(),
            fields: Vec::new(),
            selected: None,
            scroll: 0,
            editing: None,
            confirm_strip: false,
            status: String::new(),
            error: false,
        };
        p.reload();
        p
    }

    fn reload(&mut self) {
        match read(&self.path) {
            Ok(f) => {
                self.fields = f;
                let n = self.fields.iter().filter(|f| f.editable()).count();
                self.say(format!("{} things in the file; {n} can be changed.", self.fields.len()), false);
            }
            Err(e) => self.say(e, true),
        }
        self.selected = self.selected.filter(|&i| i < self.fields.len());
    }

    fn say(&mut self, s: String, error: bool) {
        self.status = s;
        self.error = error;
    }

    // Where the parts are, inside `r`.
    fn list(r: Rect) -> Rect {
        Rect::new(r.x, r.y, r.w, (r.h - BTN_H * 2 - 30).max(ROW_H))
    }
    fn status_y(r: Rect) -> i32 {
        Self::list(r).bottom() + 6
    }
    fn edit_box(r: Rect) -> Rect {
        Rect::new(r.x, Self::status_y(r) + GLYPH_H + 6, r.w - 2 * (BTN_W / 2 + 34) - 8, 22)
    }
    fn buttons(r: Rect) -> [Rect; 4] {
        let y = r.bottom() - BTN_H;
        [0, 1, 2, 3].map(|i| Rect::new(r.x + i * (BTN_W + 6), y, BTN_W, BTN_H))
    }
    fn edit_buttons(r: Rect) -> (Rect, Rect) {
        let e = Self::edit_box(r);
        (Rect::new(e.right() + 6, e.y, BTN_W / 2 + 30, 22), Rect::new(e.right() + 6 + BTN_W / 2 + 34, e.y, BTN_W / 2 + 30, 22))
    }
    fn rows(r: Rect) -> usize {
        ((Self::list(r).h - 4) / ROW_H).max(1) as usize
    }

    pub fn draw(&self, s: &mut Surface, r: Rect) {
        let list = Self::list(r);
        s.fill(list, color::WHITE);
        s.bevel(list, Bevel::Sunken);
        s.set_clip(Some(list.inset(2)));
        // Names two fifths of the width at most (the values are what matter).
        let label_w = (list.w * 2 / 5).min(250);
        let (label_chars, value_chars) = ((label_w - 12) / GLYPH_W, (list.w - label_w - 12) / GLYPH_W);
        for (row, (i, f)) in self.fields.iter().enumerate().skip(self.scroll).take(Self::rows(r)).enumerate() {
            let y = list.y + 3 + row as i32 * ROW_H;
            let selected = self.selected == Some(i);
            let (fg, dim) = if selected { (color::WHITE, color::WHITE) } else { (color::BLACK, color::SHADOW) };
            if selected {
                s.fill(Rect::new(list.x + 2, y, list.w - 4, ROW_H), color::NAVY);
            }
            let label = ellipsize(&format!("{}: {}", f.group, f.tag), label_chars.max(1) as usize);
            s.text(list.x + 6, y + 3, &label, if f.editable() { fg } else { dim });
            s.text(list.x + label_w, y + 3, &ellipsize(&f.value, value_chars.max(1) as usize), fg);
        }
        s.set_clip(None);

        let color = if self.error { color::RED } else { color::BLACK };
        let line =
            if self.confirm_strip { "Remove all of it? It can't be undone (the colour profile stays).".to_owned() } else { self.status.clone() };
        s.text(r.x, Self::status_y(r), &ellipsize(&line, (r.w / GLYPH_W).max(1) as usize), color);

        if let Some((text, cursor)) = &self.editing {
            text_field_with_cursor(s, Self::edit_box(r), text, *cursor, None, true);
            let (ok, cancel) = Self::edit_buttons(r);
            button(s, ok, "OK", true);
            button(s, cancel, "Cancel", true);
        }
        let can_change = self.selected.and_then(|i| self.fields.get(i)).is_some_and(Field::editable);
        let [b0, b1, b2, b3] = Self::buttons(r);
        if self.confirm_strip {
            button(s, b0, "Strip", true);
            button(s, b1, "Cancel", true);
        } else {
            button(s, b0, "Edit...", can_change && self.editing.is_none());
            button(s, b1, "Remove", can_change && self.editing.is_none());
            button(s, b2, "Strip All", !self.fields.is_empty());
            button(s, b3, "As Text", !self.fields.is_empty());
        }
    }

    pub fn wheel(&mut self, r: Rect, up: bool) -> Action {
        let most = self.fields.len().saturating_sub(Self::rows(r));
        self.scroll = if up { self.scroll.saturating_sub(3) } else { (self.scroll + 3).min(most) };
        Action::Redraw
    }

    fn start_edit(&mut self) -> Action {
        if let Some(f) = self.selected.and_then(|i| self.fields.get(i)).filter(|f| f.editable()) {
            let v = f.value.clone();
            self.editing = Some((v.clone(), v.chars().count()));
            self.say(format!("New value for {}:", f.tag), false);
        }
        Action::Redraw
    }

    fn apply(&mut self, value: Option<String>) {
        let Some(f) = self.selected.and_then(|i| self.fields.get(i)).cloned() else { return };
        match set(&self.path, &f, value.as_deref()) {
            Ok(()) => {
                self.reload();
                self.say(format!("{} {}.", f.tag, if value.is_some() { "changed" } else { "removed" }), false);
            }
            Err(e) => self.say(e, true),
        }
    }

    /// A click at `x`, `y`; `double` for the second of a double-click.
    pub fn click(&mut self, r: Rect, x: i32, y: i32, double: bool) -> Action {
        let [b0, b1, b2, b3] = Self::buttons(r);
        if self.confirm_strip {
            if b0.contains(x, y) {
                self.confirm_strip = false;
                match strip(&self.path) {
                    Ok(()) => {
                        self.reload();
                        self.say("Stripped: what's left is what any copy of it has (size, type...).".into(), false);
                    }
                    Err(e) => self.say(e, true),
                }
            } else if b1.contains(x, y) {
                self.confirm_strip = false;
            }
            return Action::Redraw;
        }
        if self.editing.is_some() {
            let (ok, cancel) = Self::edit_buttons(r);
            if ok.contains(x, y) {
                let v = self.editing.take().map(|(t, _)| t);
                self.apply(v);
            } else if cancel.contains(x, y) {
                self.editing = None;
                self.reload();
            }
            return Action::Redraw;
        }
        let list = Self::list(r);
        if list.contains(x, y) {
            let i = self.scroll + ((y - list.y - 3).max(0) / ROW_H) as usize;
            if i < self.fields.len() {
                self.selected = Some(i);
                if double {
                    return self.start_edit();
                }
            }
            return Action::Redraw;
        }
        let can_change = self.selected.and_then(|i| self.fields.get(i)).is_some_and(Field::editable);
        if b0.contains(x, y) && can_change {
            return self.start_edit();
        }
        if b1.contains(x, y) && can_change {
            self.apply(None);
            return Action::Redraw;
        }
        if b2.contains(x, y) && !self.fields.is_empty() {
            self.confirm_strip = true;
            return Action::Redraw;
        }
        if b3.contains(x, y) && !self.fields.is_empty() {
            return Action::OpenText(as_text(&self.path, &self.fields));
        }
        Action::None
    }

    /// A key; `true` if the panel took it.
    pub fn key(&mut self, r: Rect, k: Key) -> (bool, Action) {
        if let Some((text, cursor)) = &mut self.editing {
            let n = text.chars().count();
            match k {
                Key::Enter => {
                    let v = self.editing.take().map(|(t, _)| t);
                    self.apply(v);
                }
                Key::Escape => {
                    self.editing = None;
                    self.reload();
                }
                Key::Backspace if *cursor > 0 => {
                    *cursor -= 1;
                    let at = text.char_indices().nth(*cursor).map_or(text.len(), |(b, _)| b);
                    text.remove(at);
                }
                Key::Delete if *cursor < n => {
                    let at = text.char_indices().nth(*cursor).map_or(text.len(), |(b, _)| b);
                    text.remove(at);
                }
                Key::Left => *cursor = cursor.saturating_sub(1),
                Key::Right => *cursor = (*cursor + 1).min(n),
                Key::Home => *cursor = 0,
                Key::End => *cursor = n,
                Key::Char(c) if !c.is_control() => {
                    let at = text.char_indices().nth(*cursor).map_or(text.len(), |(b, _)| b);
                    text.insert(at, c);
                    *cursor += 1;
                }
                _ => {}
            }
            return (true, Action::Redraw);
        }
        if self.confirm_strip && k == Key::Escape {
            self.confirm_strip = false;
            return (true, Action::Redraw);
        }
        let n = self.fields.len();
        match k {
            Key::Up | Key::Down if n > 0 => {
                let i = match (self.selected, k) {
                    (None, _) => 0,
                    (Some(i), Key::Up) => i.saturating_sub(1),
                    (Some(i), _) => (i + 1).min(n - 1),
                };
                self.selected = Some(i);
                let rows = Self::rows(r);
                if i < self.scroll {
                    self.scroll = i;
                } else if i >= self.scroll + rows {
                    self.scroll = i + 1 - rows;
                }
                (true, Action::Redraw)
            }
            Key::Enter => (true, self.start_edit()),
            Key::Delete => {
                if self.selected.and_then(|i| self.fields.get(i)).is_some_and(Field::editable) {
                    self.apply(None);
                }
                (true, Action::Redraw)
            }
            _ => (false, Action::None),
        }
    }
}

/// Asks the desktop to open the Info window for `path`: what a sandboxed
/// program (the Image Viewer, the Video Player), which may start nothing,
/// does instead. The desktop opens only that, and only for a file.
pub fn ask_desktop(conn: &impl x11rb::connection::Connection, root: u32, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask, PropMode};
    let atom = |n: &[u8]| -> Result<u32, Box<dyn std::error::Error>> { Ok(conn.intern_atom(false, n)?.reply()?.atom) };
    let (request, utf8) = (atom(b"_FERRO_INFO")?, atom(b"UTF8_STRING")?);
    let bytes = path.to_string_lossy().into_owned().into_bytes();
    conn.change_property(PropMode::REPLACE, root, request, utf8, 8, bytes.len() as u32, &bytes)?;
    let event = ClientMessageEvent::new(32, root, request, [0u32; 5]);
    conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event)?;
    conn.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_exiftool_lines() {
        let f = parse_args("-File:FileName=a b.jpg\n-IFD0:Make=Canon\n-GPS:GPSLatitude=51 deg 30' 0.00\" N\n-XMP-dc:Description=x = y\n");
        assert_eq!(f.len(), 4);
        assert_eq!(f[1], Field { group: "IFD0".into(), tag: "Make".into(), value: "Canon".into() });
        assert_eq!(f[3].value, "x = y", "only the first = splits");
        assert!(!f[0].editable(), "the file system's, not the file's");
        assert!(f[2].editable());
    }

    #[test]
    fn videos_and_pictures() {
        assert!(is_video(Path::new("a/clip.MP4")));
        assert!(!is_video(Path::new("a/photo.jpg")));
    }
}
