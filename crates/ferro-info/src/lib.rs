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

use ferro_gfx::{color, ellipsize, icons, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, default_button, dialog_answer, text_field_with_cursor, Key};
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod text;

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
        if self.group == TAG_GROUP {
            return self.tag == TAG_ROW;
        }
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
/// streams copied as they are, never re-encoded. Written beside it, then
/// put in its place, with its permissions and times as they were.
fn remux(path: &Path, extra: &[&str]) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mkv");
    let tmp = dir.join(format!(".ferro-info-{}.{ext}", std::process::id()));
    let (src, dst) = (path.to_string_lossy(), tmp.to_string_lossy());
    let mut args = vec!["-nostdin", "-loglevel", "error", "-y", "-i", &src, "-map", "0", "-c", "copy"];
    args.extend_from_slice(extra);
    args.push(&dst);
    let before = std::fs::metadata(path).ok();
    let result = run_jailed("ffmpeg", &[], &args).and_then(|_| {
        if let Some(m) = &before {
            keep_times(&tmp, m);
        }
        std::fs::rename(&tmp, path).map_err(|e| format!("It couldn't be replaced: {e}"))
    });
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Gives `file` the permissions and times `was` has.
fn keep_times(file: &Path, was: &std::fs::Metadata) {
    let _ = std::fs::set_permissions(file, was.permissions());
    if let (Ok(f), Ok(accessed), Ok(modified)) = (std::fs::File::options().write(true).open(file), was.accessed(), was.modified()) {
        let _ = f.set_times(std::fs::FileTimes::new().set_accessed(accessed).set_modified(modified));
    }
}

/// The tags `path` carries (see ferro_meta::tags: where each kind of file
/// keeps them). Read straight from the file, nothing started.
pub fn read_tags(path: &Path) -> Vec<String> {
    ferro_meta::tags::read_tags(path)
}

/// Whether `path` is a kind of file that can keep tags.
pub fn can_tag(path: &Path) -> bool {
    ferro_meta::tags::kind(path).is_some()
}

/// The fields that hold the tags: shown as the Tags rows instead.
const TAG_FIELDS: [(&str, &str); 6] =
    [("XMP-dc", "Subject"), ("IPTC", "Keywords"), ("PDF", "Keywords"), ("XMP-pdf", "Keywords"), ("Keys", "Keywords"), ("Matroska", "Keywords")];

/// Gives `path` exactly these tags, where other programs read them: the
/// picture and sound left as they are, the file's times too.
pub fn write_tags(path: &Path, tags: &[String]) -> Result<(), String> {
    use ferro_meta::tags::TagKind;
    let kind = ferro_meta::tags::kind(path).ok_or("Tags can't be kept inside this kind of file.")?;
    let joined = tags.join(", ");
    if kind == TagKind::Matroska {
        // ExifTool doesn't write these: ffmpeg, the Matroska tag KEYWORDS
        // (an empty one isn't kept).
        let meta = format!("KEYWORDS={joined}");
        remux(path, &["-map_metadata", "0", "-metadata", &meta])?;
    } else {
        let mut args: Vec<String> = ["-overwrite_original", "-P", "-charset", "filename=utf8", "-XMP-dc:Subject="].map(String::from).to_vec();
        args.extend(tags.iter().map(|t| format!("-XMP-dc:Subject={t}")));
        match kind {
            TagKind::Jpeg => {
                args.extend(["-IPTC:CodedCharacterSet=UTF8".into(), "-IPTC:Keywords=".into()]);
                args.extend(tags.iter().map(|t| format!("-IPTC:Keywords={t}")));
            }
            TagKind::Pdf => {
                args.push("-PDF:Keywords=".into());
                args.extend(tags.iter().map(|t| format!("-PDF:Keywords={t}")));
                args.push(format!("-XMP-pdf:Keywords={joined}"));
            }
            TagKind::QuickTime => args.push(format!("-Keys:Keywords={joined}")),
            _ => {}
        }
        args.extend(["--".into(), path.to_string_lossy().into_owned()]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = exiftool(&args)?;
        if !out.contains("1 image files updated") {
            return Err("They couldn't be written into this file.".into());
        }
    }
    if read_tags(path) != tags {
        return Err("The tags were written, but don't read back as they should.".into());
    }
    Ok(())
}

/// Strip All: removes all of it (`strip`), but for the tags if `keep_tags`,
/// which are put back.
pub fn strip_keeping(path: &Path, keep_tags: bool) -> Result<(), String> {
    let tags = if keep_tags { read_tags(path) } else { Vec::new() };
    strip(path)?;
    if !tags.is_empty() {
        write_tags(path, &tags).map_err(|e| format!("Stripped, but the tags couldn't be put back: {e}"))?;
    }
    Ok(())
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

/// The group the Tags rows are shown in, at the top of the list.
pub const TAG_GROUP: &str = "Tags";
/// A Tags row that's one of the tags (the other kind says there are none,
/// or can't be).
const TAG_ROW: &str = "Tag";

/// What the panel asks of the window it's in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Redraw,
}

/// The metadata panel: the tags and the list, and Add Tag, Edit, Remove,
/// Strip All, Open as Text.
pub struct Panel {
    pub path: PathBuf,
    /// What's shown: the Tags rows first, then what the file carries.
    pub fields: Vec<Field>,
    selected: Option<usize>,
    scroll: usize,
    /// Typing a new value: the text and the cursor.
    editing: Option<(String, usize)>,
    /// What's typed is a new tag (Add Tag), not a new value for the row.
    adding_tag: bool,
    confirm_strip: bool,
    /// Strip All keeps the tags (asked with it; ticked to begin with).
    keep_tags: bool,
    status: String,
    error: bool,
}

/// The Tags rows and the rest, as shown: the fields the tags are kept in
/// aren't listed again below them.
fn with_tag_rows(path: &Path, fields: Vec<Field>, tags: &[String]) -> Vec<Field> {
    let row = |tag: &str, value: &str| Field { group: TAG_GROUP.into(), tag: tag.into(), value: value.into() };
    let taggable = can_tag(path);
    let mut out: Vec<Field> = tags.iter().map(|t| row(TAG_ROW, t)).collect();
    if !taggable {
        out.push(row("Tags", "can't be kept in this kind of file"));
    } else if tags.is_empty() {
        out.push(row("Tags", "none yet (Add Tag... adds one)"));
    }
    out.extend(fields.into_iter().filter(|f| !(taggable && TAG_FIELDS.contains(&(f.group.as_str(), f.tag.as_str())))));
    out
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
            adding_tag: false,
            confirm_strip: false,
            keep_tags: true,
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
            adding_tag: false,
            confirm_strip: false,
            keep_tags: true,
            status: String::new(),
            error: false,
        };
        p.reload();
        p
    }

    fn reload(&mut self) {
        let tags = read_tags(&self.path);
        match read(&self.path) {
            Ok(f) => {
                self.fields = with_tag_rows(&self.path, f, &tags);
                let rest = self.fields.iter().filter(|f| f.group != TAG_GROUP);
                let (n, m) = rest.fold((0, 0), |(n, m), f| (n + 1, m + usize::from(f.editable())));
                let tagged = match tags.len() {
                    0 => String::new(),
                    1 => ", 1 tag".into(),
                    k => format!(", {k} tags"),
                };
                self.say(format!("{n} things in the file{tagged}; {m} can be changed."), false);
            }
            Err(e) => {
                self.fields = with_tag_rows(&self.path, Vec::new(), &tags);
                self.say(e, true);
            }
        }
        self.selected = self.selected.filter(|&i| i < self.fields.len());
    }

    /// The tags, as the Tags rows show them.
    pub fn tags(&self) -> Vec<String> {
        self.fields.iter().filter(|f| f.group == TAG_GROUP && f.tag == TAG_ROW).map(|f| f.value.clone()).collect()
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
    /// Add Tag, where the box to type in is while typing.
    fn add_tag_button(r: Rect) -> Rect {
        Rect::new(r.x, Self::edit_box(r).y, BTN_W + 12, 22)
    }
    /// Keep my tags, under "Remove all of it?".
    fn keep_box(r: Rect) -> Rect {
        let e = Self::edit_box(r);
        Rect::new(e.x, e.y + 4, text_width("Keep my tags") + 24, 15)
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

    fn taggable(&self) -> bool {
        can_tag(&self.path)
    }

    /// Whether Strip All asks about keeping the tags (there are some to keep).
    fn asks_keep(&self) -> bool {
        self.taggable() && !self.tags().is_empty()
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
            let name = if f.group == TAG_GROUP { f.tag.clone() } else { format!("{}: {}", f.group, f.tag) };
            let label = ellipsize(&name, label_chars.max(1) as usize);
            s.text(list.x + 6, y + 3, &label, if f.editable() { fg } else { dim });
            let value_fg = if f.group == TAG_GROUP && f.tag != TAG_ROW { dim } else { fg };
            s.text(list.x + label_w, y + 3, &ellipsize(&f.value, value_chars.max(1) as usize), value_fg);
        }
        s.set_clip(None);

        let color = if self.error { color::RED } else { color::BLACK };
        let line =
            if self.confirm_strip { "Remove all of it? It can't be undone (the colour profile stays).".to_owned() } else { self.status.clone() };
        s.text(r.x, Self::status_y(r), &ellipsize(&line, (r.w / GLYPH_W).max(1) as usize), color);

        if let Some((text, cursor)) = &self.editing {
            text_field_with_cursor(s, Self::edit_box(r), text, *cursor, None, true);
            let (ok, cancel) = Self::edit_buttons(r);
            default_button(s, ok, "OK", true);
            button(s, cancel, "Cancel", true);
        } else if self.confirm_strip {
            if self.asks_keep() {
                let k = Self::keep_box(r);
                let b = Rect::new(k.x, k.y + 1, 13, 13);
                s.fill(b, color::WHITE);
                s.bevel(b, Bevel::Sunken);
                if self.keep_tags {
                    s.sprite_mono(b.x + 3, b.y + 3, icons::GLYPH_CHECK, color::BLACK);
                }
                s.text(b.right() + 8, b.y + 3, "Keep my tags", color::BLACK);
            }
        } else {
            button(s, Self::add_tag_button(r), "Add Tag...", self.taggable());
        }
        let can_change = self.selected.and_then(|i| self.fields.get(i)).is_some_and(Field::editable);
        let [b0, b1, b2, b3] = Self::buttons(r);
        if self.confirm_strip {
            default_button(s, b0, "Strip", true);
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
        let Some(f) = self.selected.and_then(|i| self.fields.get(i)) else { return Action::Redraw };
        if f.editable() {
            let v = f.value.clone();
            let what = if f.group == TAG_GROUP { "the tag".to_owned() } else { f.tag.clone() };
            self.editing = Some((v.clone(), v.chars().count()));
            self.adding_tag = false;
            self.say(format!("New value for {what}:"), false);
        } else if f.group == TAG_GROUP {
            // "none yet": a new one.
            return self.start_add_tag();
        }
        Action::Redraw
    }

    fn start_add_tag(&mut self) -> Action {
        if self.taggable() && self.editing.is_none() {
            self.editing = Some((String::new(), 0));
            self.adding_tag = true;
            self.say("New tag (commas between several):".into(), false);
        }
        Action::Redraw
    }

    /// Gives the file `tags`, saying `done` if it worked.
    fn retag(&mut self, tags: &[String], done: &str) {
        match write_tags(&self.path, tags) {
            Ok(()) => {
                self.reload();
                self.say(done.into(), false);
            }
            Err(e) => {
                self.reload();
                self.say(e, true);
            }
        }
    }

    /// OK on what's typed (`Some`), or Remove (`None`).
    fn apply(&mut self, value: Option<String>) {
        if std::mem::take(&mut self.adding_tag) {
            let mut tags = self.tags();
            let new: Vec<String> = ferro_meta::tags::split_typed(value.as_deref().unwrap_or("")).into_iter().filter(|t| !tags.contains(t)).collect();
            if new.is_empty() {
                self.reload();
                return;
            }
            let said = if new.len() == 1 { format!("Tag '{}' added.", new[0]) } else { format!("{} tags added.", new.len()) };
            tags.extend(new);
            return self.retag(&tags, &said);
        }
        let Some(i) = self.selected else { return };
        let Some(f) = self.fields.get(i).cloned() else { return };
        if f.group == TAG_GROUP {
            // This tag: changed (into several, with commas) or taken off.
            let pos = self.fields[..i].iter().filter(|g| g.group == TAG_GROUP && g.tag == TAG_ROW).count();
            let mut tags = self.tags();
            let with = ferro_meta::tags::split_typed(value.as_deref().unwrap_or(""));
            let removed = with.is_empty();
            if pos < tags.len() {
                tags.splice(pos..=pos, with);
            }
            let tags = ferro_meta::tags::split_typed(&tags.join(","));
            let said = if removed { format!("Tag '{}' removed.", f.value) } else { "Tag changed.".to_owned() };
            return self.retag(&tags, &said);
        }
        match set(&self.path, &f, value.as_deref()) {
            Ok(()) => {
                self.reload();
                self.say(format!("{} {}.", f.tag, if value.is_some() { "changed" } else { "removed" }), false);
            }
            Err(e) => self.say(e, true),
        }
    }

    /// Strip, the answer to "Remove all of it?".
    fn strip_all(&mut self) {
        self.confirm_strip = false;
        let keep = self.keep_tags && self.asks_keep();
        match strip_keeping(&self.path, keep) {
            Ok(()) => {
                self.reload();
                let said = if keep { "Stripped, all but your tags." } else { "Stripped: what's left is what any copy of it has (size, type...)." };
                self.say(said.into(), false);
            }
            Err(e) => {
                self.reload();
                self.say(e, true);
            }
        }
    }

    /// A click at `x`, `y`; `double` for the second of a double-click.
    pub fn click(&mut self, r: Rect, x: i32, y: i32, double: bool) -> Action {
        let [b0, b1, b2, b3] = Self::buttons(r);
        if self.confirm_strip {
            if b0.contains(x, y) {
                self.strip_all();
            } else if b1.contains(x, y) {
                self.confirm_strip = false;
            } else if self.asks_keep() && Self::keep_box(r).contains(x, y) {
                self.keep_tags = !self.keep_tags;
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
                self.adding_tag = false;
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
        if Self::add_tag_button(r).contains(x, y) {
            return self.start_add_tag();
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
            self.keep_tags = true;
            ferro_ui::sound("ding");
            return Action::Redraw;
        }
        if b3.contains(x, y) && !self.fields.is_empty() {
            match text::show(&as_text(&self.path, &self.fields)) {
                Ok(()) => self.say("Opened in the text editor (the copy it reads from is removed once it has).".into(), false),
                Err(e) => self.say(e, true),
            }
            return Action::Redraw;
        }
        Action::None
    }

    /// A key; `true` if the panel took it.
    pub fn key(&mut self, r: Rect, k: Key) -> (bool, Action) {
        if let Some((text, cursor)) = &mut self.editing {
            let n = text.chars().count();
            match k {
                _ if dialog_answer(k) == Some(true) => {
                    let v = self.editing.take().map(|(t, _)| t);
                    self.apply(v);
                }
                _ if dialog_answer(k) == Some(false) => {
                    self.editing = None;
                    self.adding_tag = false;
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
        if self.confirm_strip {
            match (dialog_answer(k), k) {
                (Some(true), _) => self.strip_all(),
                (Some(false), _) => self.confirm_strip = false,
                // K: Keep my tags, ticked or not.
                (_, Key::Char('k' | 'K')) if self.asks_keep() => self.keep_tags = !self.keep_tags,
                _ => {}
            }
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

    fn field(group: &str, tag: &str, value: &str) -> Field {
        Field { group: group.into(), tag: tag.into(), value: value.into() }
    }

    #[test]
    fn tags_shown_first_and_not_twice() {
        let fields = vec![field("IFD0", "Make", "Canon"), field("XMP-dc", "Subject", "anime, comedy"), field("IPTC", "Keywords", "anime")];
        let tags = ["anime".to_owned(), "comedy".to_owned()];
        let rows = with_tag_rows(Path::new("a.jpg"), fields.clone(), &tags);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], field(TAG_GROUP, "Tag", "anime"));
        assert!(rows[1].editable());
        assert_eq!(rows[2].tag, "Make");
        // None yet: a row saying so, which can't be edited or removed.
        let none = with_tag_rows(Path::new("a.mkv"), Vec::new(), &[]);
        assert!(none.len() == 1 && !none[0].editable());
        // A kind of file that can't keep them: its own fields all shown.
        let mp3 = with_tag_rows(Path::new("a.mp3"), vec![field("ID3v2_4", "Genre", "Rock")], &[]);
        assert_eq!(mp3.len(), 2);
        assert!(mp3[0].value.contains("can't"));
    }

    /// Tags written into copies of the files in FERRO_TAG_SAMPLES (made by
    /// hand: needs ExifTool and ffmpeg) and read back:
    /// `cargo test -p ferro-info -- --ignored`.
    #[test]
    #[ignore]
    fn tags_written_where_they_belong() {
        let dir = std::env::var_os("FERRO_TAG_SAMPLES").expect("FERRO_TAG_SAMPLES");
        let out = std::env::temp_dir().join(format!("ferro-info-tags-{}", std::process::id()));
        std::fs::create_dir_all(&out).unwrap();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let src = e.path();
            if !can_tag(&src) {
                continue;
            }
            let p = out.join(src.file_name().unwrap());
            std::fs::copy(&src, &p).unwrap();
            let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
            std::fs::File::options().write(true).open(&p).unwrap().set_times(std::fs::FileTimes::new().set_modified(old)).unwrap();
            let tags: Vec<String> = ["anime", "ü sports", "Tom & Jerry <3"].map(String::from).to_vec();
            write_tags(&p, &tags).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert_eq!(read_tags(&p), tags, "{}", p.display());
            assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), old, "{}: its time kept", p.display());
            write_tags(&p, &tags[1..]).unwrap();
            assert_eq!(read_tags(&p), &tags[1..]);
            strip_keeping(&p, true).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert_eq!(read_tags(&p), &tags[1..], "{}: kept through Strip All", p.display());
            strip_keeping(&p, false).unwrap();
            assert!(read_tags(&p).is_empty(), "{}: stripped", p.display());
            println!("{}: ok", p.display());
        }
        std::fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn videos_and_pictures() {
        assert!(is_video(Path::new("a/clip.MP4")));
        assert!(!is_video(Path::new("a/photo.jpg")));
    }
}
