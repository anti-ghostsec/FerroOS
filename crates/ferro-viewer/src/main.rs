//! ferro-viewer: Fast, retro Windows 95 photo and animation viewer for FerroOS.
//!
//! Inspired by Enclave's built-in memory-only media viewer and authentic Windows 95
//! aesthetics. Supports PNG, JPEG, BMP, WebP, and looping animated GIFs and WebPs with full
//! zoom, pan, slideshow, directory browsing, and delete capabilities; comic books (.cbz),
//! read page by page in memory; and an edit mode to zoom to, crop or black out a box and
//! save the result as a new file.

use ferro_gfx::image::{DecodedImage, ImageAnimation};
use ferro_gfx::{color, ellipsize, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::edit::{Edit, TextEdit};
use ferro_ui::{button, default_button, Keymap, Res};
use std::cmp::Ordering;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const TOOLBAR_H: i32 = 32;
const STATUS_H: i32 = 22;

/// The narrowest the window goes: the toolbar's last button still fits.
const MIN_W: i32 = 560;
const MIN_H: i32 = 240;

const PREV_BTN: Rect = Rect::new(4, 4, 68, 24);
const NEXT_BTN: Rect = Rect::new(76, 4, 68, 24);
const ZOOM_OUT_BTN: Rect = Rect::new(154, 4, 32, 24);
const ZOOM_IN_BTN: Rect = Rect::new(190, 4, 32, 24);
const ACTUAL_BTN: Rect = Rect::new(226, 4, 44, 24);
const FIT_BTN: Rect = Rect::new(274, 4, 44, 24);
const SLIDESHOW_BTN: Rect = Rect::new(328, 4, 76, 24);
const DELETE_BTN: Rect = Rect::new(414, 4, 68, 24);
const EDIT_BTN: Rect = Rect::new(492, 4, 52, 24);

// Edit mode's own toolbar: Undo and Save As where Prev and Next were, Fit
// in 1:1's place, then what's done to the box; Edit stays where it was.
const UNDO_BTN: Rect = Rect::new(4, 4, 52, 24);
const SAVE_BTN: Rect = Rect::new(60, 4, 88, 24);
const EDIT_FIT_BTN: Rect = Rect::new(226, 4, 44, 24);
const BOX_ZOOM_BTN: Rect = Rect::new(280, 4, 52, 24);
const CROP_BTN: Rect = Rect::new(336, 4, 52, 24);
const BLACK_BTN: Rect = Rect::new(392, 4, 88, 24);

const KEY_ESCAPE: u8 = 9;
const KEY_SPACE: u8 = 65;
const KEY_RETURN: u8 = 36;
const KEY_KP_ENTER: u8 = 104;
const KEY_BACKSPACE: u8 = 22;
const KEY_DELETE: u8 = 119;
const KEY_LEFT: u8 = 113;
const KEY_RIGHT: u8 = 114;
const KEY_UP: u8 = 111;
const KEY_DOWN: u8 = 116;
const KEY_PAGE_UP: u8 = 112;
const KEY_PAGE_DOWN: u8 = 117;
const KEY_HOME: u8 = 110;
const KEY_END: u8 = 115;
const KEY_PLUS: u8 = 21;
const KEY_KP_ADD: u8 = 86;
const KEY_MINUS: u8 = 20;
const KEY_KP_SUB: u8 = 82;
const KEY_F: u8 = 41;
const KEY_1: u8 = 10;
const KEY_S: u8 = 39;
const KEY_Q: u8 = 24;
const KEY_Y: u8 = 29;
const KEY_N: u8 = 57;
const KEY_D: u8 = 40;
const KEY_E: u8 = 26;
const KEY_B: u8 = 56;
const KEY_C: u8 = 54;
const KEY_Z: u8 = 52;
const KEY_T: u8 = 28;

/// The most one picture in a comic may unpack to. A booby-trapped archive
/// (a "zip bomb", a few KB that unpack to gigabytes) stops there.
const MAX_PAGE_BYTES: u64 = 256 << 20;

/// How much Undo keeps; the oldest steps go past it (one is always kept).
const MAX_UNDO_BYTES: usize = 256 << 20;

fn dialog_rect(w: i32, h: i32) -> Rect {
    let (dw, dh) = (360, 130);
    Rect::new((w - dw) / 2, (h - dh) / 2, dw, dh)
}

/// Yes and No (or the notice's OK, in No's place).
fn dialog_buttons(r: Rect) -> (Rect, Rect) {
    (Rect::new(r.right() - 170, r.bottom() - 34, 75, 24), Rect::new(r.right() - 87, r.bottom() - 34, 75, 24))
}

/// Save, Discard and Cancel.
fn dialog_buttons3(r: Rect) -> [Rect; 3] {
    [-253, -170, -87].map(|dx| Rect::new(r.right() + dx, r.bottom() - 34, 75, 24))
}

/// Where the new file's name is typed in Save As.
fn name_field(r: Rect) -> Rect {
    Rect::new(r.x + 16, r.y + 48, r.w - 32, 20)
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase()
}

fn is_supported_image(path: &Path) -> bool {
    matches!(extension(path).as_str(), "png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif" | "pnm" | "ppm" | "cbz")
}

fn is_comic(path: &Path) -> bool {
    extension(path) == "cbz"
}

fn is_jpeg_name(name: &str) -> bool {
    matches!(extension(Path::new(name)).as_str(), "jpg" | "jpeg")
}

/// Names as people count: "page2" before "page10", capitals or not. Runs
/// of digits compare as numbers; ties fall back to the plain order, so no
/// two different names are ever "the same".
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < x.len() && j < y.len() {
        if x[i].is_ascii_digit() && y[j].is_ascii_digit() {
            let run = |s: &[u8], at: usize| s[at..].iter().take_while(|c| c.is_ascii_digit()).count();
            let (ri, rj) = (run(x, i), run(y, j));
            // Leading zeros don't count; then more digits is a bigger number.
            let trim = |s: &[u8]| -> usize { s.iter().take_while(|&&c| c == b'0').count() };
            let (na, nb) = (&x[i..i + ri], &y[j..j + rj]);
            let (na, nb) = (&na[trim(na)..], &nb[trim(nb)..]);
            let o = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
            if o != Ordering::Equal {
                return o;
            }
            i += ri;
            j += rj;
        } else {
            let o = x[i].to_ascii_lowercase().cmp(&y[j].to_ascii_lowercase());
            if o != Ordering::Equal {
                return o;
            }
            i += 1;
            j += 1;
        }
    }
    (x.len() - i).cmp(&(y.len() - j)).then_with(|| a.cmp(b))
}

fn natural_path_cmp(a: &Path, b: &Path) -> Ordering {
    natural_cmp(&a.file_name().unwrap_or_default().to_string_lossy(), &b.file_name().unwrap_or_default().to_string_lossy())
}

fn scan_directory_images(path: &Path) -> (Vec<PathBuf>, usize) {
    let abs = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = abs.parent().unwrap_or_else(|| Path::new("/"));
    let mut files = Vec::new();

    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && is_supported_image(&p) {
                files.push(p);
            }
        }
    }
    files.sort_by(|a, b| natural_path_cmp(a, b));

    let current_idx = files.iter().position(|p| p == &abs).unwrap_or(0);
    if files.is_empty() {
        files.push(abs);
    }
    (files, current_idx)
}

/// The pictures in a comic book (a zip of them): each one's place in the
/// archive and its name, in reading order. Names are only for sorting and
/// showing; nothing is ever written out under them.
fn comic_pages(bytes: &[u8]) -> Option<Vec<(usize, String)>> {
    let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut pages: Vec<(usize, String)> = (0..zip.len())
        .filter_map(|i| Some((i, zip.name_for_index(i)?.ok()?.into_owned())))
        .filter(|(_, name)| {
            let base = name.rsplit('/').next().unwrap_or("");
            // Not folders, nor the hidden "._page.jpg" twins Macs add.
            !name.ends_with('/') && !base.starts_with('.') && !name.starts_with("__MACOSX/") && is_page_name(base)
        })
        .collect();
    pages.sort_by(|a, b| natural_cmp(&a.1, &b.1));
    Some(pages)
}

fn is_page_name(name: &str) -> bool {
    matches!(extension(Path::new(name)).as_str(), "png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif")
}

/// One picture of a comic, unpacked in memory: `None` if it's damaged,
/// locked, or would unpack to more than `cap` bytes.
fn comic_page(bytes: &[u8], index: usize, cap: u64) -> Option<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let entry = zip.by_index(index).ok()?;
    if entry.size() > cap {
        return None;
    }
    // The size it claims can be a lie: read one byte past the cap to tell.
    let mut out = Vec::with_capacity(entry.size() as usize);
    entry.take(cap + 1).read_to_end(&mut out).ok()?;
    if out.len() as u64 > cap {
        return None;
    }
    Some(out)
}

/// A comic book open: the archive as read from disk (it's never unpacked
/// there), its pictures in order, and the one showing.
struct Comic {
    bytes: Vec<u8>,
    pages: Vec<(usize, String)>,
    page: usize,
}

impl Comic {
    fn open(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        let pages = comic_pages(&bytes)?;
        if pages.is_empty() {
            return None;
        }
        Some(Self { bytes, pages, page: 0 })
    }

    fn page_name(&self) -> &str {
        &self.pages[self.page].1
    }

    fn load_page(&self) -> Option<LoadedMedia> {
        let (index, name) = &self.pages[self.page];
        let data = comic_page(&self.bytes, *index, MAX_PAGE_BYTES)?;
        LoadedMedia::from_bytes(data, &extension(Path::new(name)), self.bytes.len() as u64)
    }
}

struct LoadedMedia {
    single: Option<DecodedImage>,
    animation: Option<ImageAnimation>,
    /// The animation's frames still being decoded, aside.
    pending: Option<std::sync::mpsc::Receiver<(DecodedImage, Duration)>>,
    frame_idx: usize,
    last_frame_time: Instant,
    file_size_bytes: u64,
    dim: (usize, usize),
    format_name: String,
}

impl LoadedMedia {
    fn load(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        let size = bytes.len() as u64;
        Self::from_bytes(bytes, &extension(path), size)
    }

    /// A picture from its bytes; `ext` says what it should be, `file_size_bytes`
    /// is what the status bar shows (the comic's own, for one of its pages).
    fn from_bytes(bytes: Vec<u8>, ext: &str, file_size_bytes: u64) -> Option<Self> {
        // Shown from its first frame; the rest come as they're decoded.
        let streamed = match ext {
            "gif" => ImageAnimation::stream(bytes.clone(), false),
            "webp" => ImageAnimation::stream(bytes.clone(), true),
            _ => None,
        };
        if let Some((anim, pending)) = streamed {
            if !anim.frames.is_empty() {
                let (w, h) = (anim.frames[0].0.width, anim.frames[0].0.height);
                return Some(Self {
                    single: None,
                    animation: Some(anim),
                    pending: Some(pending),
                    frame_idx: 0,
                    last_frame_time: Instant::now(),
                    file_size_bytes,
                    dim: (w, h),
                    format_name: ext.to_uppercase(),
                });
            }
        }

        if let Some(img) = DecodedImage::decode(&bytes) {
            let (w, h) = (img.width, img.height);
            let fmt = ext.to_uppercase();
            return Some(Self {
                single: Some(img),
                animation: None,
                pending: None,
                frame_idx: 0,
                last_frame_time: Instant::now(),
                file_size_bytes,
                dim: (w, h),
                format_name: if fmt.is_empty() { "IMAGE".into() } else { fmt },
            });
        }

        None
    }

    fn current_frame(&self) -> Option<&DecodedImage> {
        if let Some(ref single) = self.single {
            Some(single)
        } else if let Some(ref anim) = self.animation {
            anim.frames.get(self.frame_idx).map(|(f, _)| f)
        } else {
            None
        }
    }

    fn advance_animation(&mut self, now: Instant) -> bool {
        if let Some(ref mut anim) = self.animation {
            if let Some(rx) = &self.pending {
                loop {
                    match rx.try_recv() {
                        Ok(f) => anim.frames.push(f),
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            self.pending = None;
                            break;
                        }
                    }
                }
            }
            if anim.frames.len() <= 1 {
                return false;
            }
            // At the last frame decoded so far: the next isn't ready yet.
            if self.frame_idx + 1 == anim.frames.len() && self.pending.is_some() {
                return false;
            }
            let delay = anim.frames[self.frame_idx].1;
            if now.duration_since(self.last_frame_time) >= delay {
                self.frame_idx = (self.frame_idx + 1) % anim.frames.len();
                self.last_frame_time = now;
                return true;
            }
        }
        false
    }
}

/// A box dragged on the screen from `a` to `b`, in the picture's own pixels
/// (so it stays on the same spot however it's zoomed or moved after): every
/// pixel the drag went over, cut to the picture. `at` is where the picture
/// is drawn and how big. `None` for a click, or a drag off the picture.
fn box_from_drag(a: (i32, i32), b: (i32, i32), at: Rect, size: (usize, usize)) -> Option<Rect> {
    if (a.0 - b.0).abs() < 3 && (a.1 - b.1).abs() < 3 {
        return None;
    }
    // Screen pixel `c` in from the picture's edge shows picture pixel
    // c * size / drawn (as `draw_image_scaled` does it).
    let span = |p: i32, q: i32, edge: i32, drawn: i32, n: usize| -> (i32, i32) {
        let (n, drawn) = (n as i64, i64::from(drawn.max(1)));
        let lo = (i64::from(p.min(q) - edge) * n).div_euclid(drawn);
        let hi = (i64::from(p.max(q) - edge) * n).div_euclid(drawn) + 1;
        (lo.clamp(0, n) as i32, hi.clamp(0, n) as i32)
    };
    let (x0, x1) = span(a.0, b.0, at.x, at.w, size.0);
    let (y0, y1) = span(a.1, b.1, at.y, at.h, size.1);
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// Where picture box `r` is on the screen, the picture drawn at `at`.
fn box_on_screen(r: Rect, at: Rect, size: (usize, usize)) -> Rect {
    let edge = |i: i32, start: i32, drawn: i32, n: usize| -> i32 {
        let n = (n as i64).max(1);
        // The first screen pixel showing picture pixel `i`.
        start + (i64::from(i) * i64::from(drawn) + n - 1).div_euclid(n) as i32
    };
    let (x0, x1) = (edge(r.x, at.x, at.w, size.0), edge(r.right(), at.x, at.w, size.0));
    let (y0, y1) = (edge(r.y, at.y, at.h, size.1), edge(r.bottom(), at.y, at.h, size.1));
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// The scale and pan that show picture box `r` as big as fits in `vp`, in
/// its middle (pans as `compute_scale_and_pos` takes them).
fn zoom_to_box(vp: Rect, size: (usize, usize), r: Rect) -> (f32, i32, i32) {
    let scale = (vp.w as f32 / r.w.max(1) as f32).min(vp.h as f32 / r.h.max(1) as f32).clamp(0.05, 25.0);
    let pan = |view: i32, n: usize, from: i32, len: i32| -> i32 {
        let drawn = (n as f32 * scale).round() as i32;
        let middle = (from as f32 + len as f32 / 2.0) * drawn as f32 / n.max(1) as f32;
        (view / 2) - (view - drawn) / 2 - middle.round() as i32
    };
    (scale, pan(vp.w, size.0, r.x, r.w), pan(vp.h, size.1, r.y, r.h))
}

/// `r` cut to a `w` x `h` picture.
fn clip_box(r: Rect, w: usize, h: usize) -> Rect {
    r.intersect(&Rect::new(0, 0, w as i32, h as i32))
}

/// Just box `r` of the picture.
/// What a resized picture was made from and for: the picture's pixels
/// (where they are and its size) and the size drawn.
type SmoothKey = (usize, usize, usize, i32, i32);

/// Bigger than this (a picture zoomed far in), pictures are drawn by
/// repeating pixels: resizing would be slow and pixels are what's looked at.
const SMOOTH_MOST: usize = 12_000_000;

/// `img` at `w` x `h`, resized with Lanczos (edges stay crisp, no blocks
/// or jagged steps), then sharpened a little if it was enlarged: lettering
/// in a comic page shown bigger than it was scanned reads clearly.
fn smooth_resize(img: &DecodedImage, w: usize, h: usize) -> DecodedImage {
    use image::imageops::{self, FilterType};
    let mut rgb = image::RgbImage::new(img.width as u32, img.height as u32);
    for (p, &px) in rgb.pixels_mut().zip(&img.pixels) {
        *p = image::Rgb([(px >> 16) as u8, (px >> 8) as u8, px as u8]);
    }
    let mut out = imageops::resize(&rgb, w as u32, h as u32, FilterType::Lanczos3);
    if w * h > img.width * img.height {
        out = imageops::unsharpen(&out, 0.7, 2);
    }
    let pixels = out.pixels().map(|p| (u32::from(p.0[0]) << 16) | (u32::from(p.0[1]) << 8) | u32::from(p.0[2])).collect();
    DecodedImage::new(w, h, pixels)
}

fn crop(img: &DecodedImage, r: Rect) -> DecodedImage {
    let r = clip_box(r, img.width, img.height);
    let (x, w) = (r.x as usize, r.w as usize);
    let mut pixels = Vec::with_capacity(w * r.h as usize);
    for y in r.y as usize..r.bottom() as usize {
        pixels.extend_from_slice(&img.pixels[y * img.width + x..y * img.width + x + w]);
    }
    DecodedImage::new(w, r.h as usize, pixels)
}

/// Box `r` painted solid black: the pixels themselves are written over, so
/// what was there is gone from the picture (and from any file saved from it).
fn black_box(img: &mut DecodedImage, r: Rect) {
    let r = clip_box(r, img.width, img.height);
    for y in r.y as usize..r.bottom() as usize {
        img.pixels[y * img.width + r.x as usize..y * img.width + r.right() as usize].fill(color::BLACK);
    }
}

/// The picture as a new PNG or JPEG file. It's made from the pixels alone,
/// so nothing else of the original file comes along: no EXIF, no GPS
/// position, no camera, date or editing details.
fn encode(img: &DecodedImage, jpeg: bool) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let rgb: Vec<u8> = img.pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    let (w, h) = (u32::try_from(img.width).ok()?, u32::try_from(img.height).ok()?);
    let mut out = Vec::new();
    if jpeg {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).ok()?;
    } else {
        image::codecs::png::PngEncoder::new(&mut out).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).ok()?;
    }
    Some(out)
}

/// "`stem`-edited.png" in `dir`, or -edited-2 and on if that's taken.
fn free_name(dir: &Path, stem: &str, ext: &str) -> String {
    (1..)
        .map(|n| if n == 1 { format!("{stem}-edited.{ext}") } else { format!("{stem}-edited-{n}.{ext}") })
        .find(|name| !dir.join(name).exists())
        .unwrap_or_default()
}

/// The file name typed in Save As, with an extension (`ext` if it had
/// none we can write); `None` if it isn't a name for a file in the folder.
fn typed_name(typed: &str, ext: &str) -> Option<String> {
    let name = typed.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return None;
    }
    Some(match extension(Path::new(name)).as_str() {
        "png" | "jpg" | "jpeg" => name.to_string(),
        _ => format!("{name}.{ext}"),
    })
}

/// Edit mode: a copy of the picture being changed (the file itself is
/// never touched; Save As writes a new one), the steps back, and the box
/// drawn on it.
struct Editing {
    image: DecodedImage,
    undo: Vec<DecodedImage>,
    /// How many steps back is what was last saved (or what was opened);
    /// `None` once that can't be got back to.
    saved: Option<usize>,
    /// The box, in the picture's pixels.
    sel: Option<Rect>,
    /// What a saved copy is called before "-edited", and whether it's a
    /// JPEG (the original was one) or a PNG.
    stem: String,
    jpeg: bool,
}

impl Editing {
    fn new(image: DecodedImage, stem: String, jpeg: bool) -> Self {
        Self { image, undo: Vec::new(), saved: Some(0), sel: None, stem, jpeg }
    }

    /// Changes not saved yet.
    fn unsaved(&self) -> bool {
        self.saved != Some(self.undo.len())
    }

    /// Keeps the picture as it is for Undo, before a change.
    fn push(&mut self) {
        if self.saved.is_some_and(|n| n > self.undo.len()) {
            self.saved = None; // undone past the save, and now changed again
        }
        self.undo.push(self.image.clone());
        let mut kept: usize = self.undo.iter().map(|i| i.pixels.len() * 4).sum();
        while self.undo.len() > 1 && kept > MAX_UNDO_BYTES {
            kept -= self.undo.remove(0).pixels.len() * 4;
            self.saved = self.saved.and_then(|n| n.checked_sub(1));
        }
    }

    fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else { return false };
        self.image = prev;
        self.sel = None;
        true
    }

    /// Cuts the picture to the box.
    fn crop(&mut self) -> bool {
        let Some(r) = self.sel.take().map(|r| clip_box(r, self.image.width, self.image.height)) else { return false };
        if r.w <= 0 || r.h <= 0 || (r.w as usize, r.h as usize) == (self.image.width, self.image.height) {
            return false;
        }
        self.push();
        self.image = crop(&self.image, r);
        true
    }

    /// Paints the box black.
    fn black_box(&mut self) -> bool {
        let Some(r) = self.sel.take() else { return false };
        self.push();
        black_box(&mut self.image, r);
        true
    }
}

/// What's done once the changes are saved or let go.
#[derive(Clone, Copy, Debug, PartialEq)]
enum After {
    /// Go on editing (Save As from the toolbar).
    Stay,
    /// Out of edit mode.
    Leave,
    /// Close the viewer.
    Quit,
}

/// A question over the picture, for edit mode.
enum Ask {
    /// "Save the changes?": Save, Discard, Cancel.
    Unsaved(After),
    /// The new file's name, being typed.
    SaveAs(TextEdit, After),
    /// That name's taken: replace the file?
    Replace(PathBuf, After),
}

/// A box being dragged out: from where, to where the pointer is now, and
/// whether it's to zoom to (the right button, or Ctrl) or edit mode's box.
struct Boxing {
    from: (i32, i32),
    to: (i32, i32),
    zoom: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ZoomMode {
    Fit,
    Scale(f32),
}

struct App {
    conn: RustConnection,
    win: Window,
    gc: Gcontext,
    depth: u8,
    w: i32,
    h: i32,
    files: Vec<PathBuf>,
    idx: usize,
    media: Option<LoadedMedia>,
    /// The comic book open, when the file is one; `media` is its page.
    comic: Option<Comic>,
    /// A comic's pages drawn bigger or smaller than they are: resized with a
    /// good filter (and sharpened a little when enlarged) instead of by
    /// repeating or dropping pixels, which on a big screen made the lettering
    /// blocky. Other pictures are drawn as they are. T switches it off and on.
    smooth: bool,
    /// The picture showing, resized for the window: what (pixels, size) and
    /// to what size, and the result.
    smoothed: Option<(SmoothKey, DecodedImage)>,
    /// Being resized, aside (it takes a third of a second for a page): what,
    /// and where the result comes. Until then the picture is drawn as is.
    smoothing: Option<(SmoothKey, std::sync::mpsc::Receiver<DecodedImage>)>,
    zoom: ZoomMode,
    pan_x: i32,
    pan_y: i32,
    dragging: bool,
    drag_start: (i32, i32),
    pan_start: (i32, i32),
    boxing: Option<Boxing>,
    slideshow: bool,
    last_slide_time: Instant,
    last_click: (Instant, i32, i32),
    atoms: Atoms,
    keymap: Keymap,
    /// "Delete this picture?" is showing, for this file; and whether it's
    /// for good (it's on a removable drive, so can't go to the Recycle Bin).
    confirm_delete: Option<(PathBuf, bool)>,
    /// What went wrong, shown until the next key or click.
    notice: Option<String>,
    /// Edit mode, when it's on.
    edit: Option<Editing>,
    ask: Option<Ask>,
    /// Something changed since the window was last drawn. Drawn once all the
    /// waiting events are in, not once per mouse move or wheel notch: the
    /// whole window goes to the X server each time.
    dirty: bool,
}

struct Atoms {
    wm_protocols: Atom,
    wm_delete: Atom,
    net_wm_name: Atom,
    utf8_string: Atom,
}

impl App {
    fn new(conn: RustConnection, screen_num: usize, initial_path: Option<&Path>) -> Res<Self> {
        let (root_depth, root, root_visual) = {
            let screen = &conn.setup().roots[screen_num];
            (screen.root_depth, screen.root, screen.root_visual)
        };
        let (files, idx) = if let Some(p) = initial_path { scan_directory_images(p) } else { (Vec::new(), 0) };

        let w = 820;
        let h = 600;
        let win = conn.generate_id()?;
        let events = EventMask::EXPOSURE
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::KEY_PRESS;

        conn.create_window(
            root_depth,
            win,
            root,
            100,
            100,
            w as u16,
            h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            root_visual,
            &CreateWindowAux::new().event_mask(events),
        )?;

        let gc = conn.generate_id()?;
        conn.create_gc(gc, win, &CreateGCAux::new())?;

        let atom = |name: &str| -> Res<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
        let atoms = Atoms {
            wm_protocols: atom("WM_PROTOCOLS")?,
            wm_delete: atom("WM_DELETE_WINDOW")?,
            net_wm_name: atom("_NET_WM_NAME")?,
            utf8_string: atom("UTF8_STRING")?,
        };

        conn.change_property32(PropMode::REPLACE, win, atoms.wm_protocols, AtomEnum::ATOM, &[atoms.wm_delete])?;
        x11rb::properties::WmSizeHints { min_size: Some((MIN_W, MIN_H)), ..Default::default() }.set_normal_hints(&conn, win)?;
        conn.map_window(win)?;
        let keymap = Keymap::load(&conn)?;

        let mut app = Self {
            conn,
            win,
            gc,
            depth: root_depth,
            w,
            h,
            files,
            idx,
            media: None,
            comic: None,
            zoom: ZoomMode::Fit,
            smooth: true,
            smoothed: None,
            smoothing: None,
            pan_x: 0,
            pan_y: 0,
            dragging: false,
            drag_start: (0, 0),
            pan_start: (0, 0),
            boxing: None,
            slideshow: false,
            last_slide_time: Instant::now(),
            last_click: (Instant::now(), 0, 0),
            confirm_delete: None,
            notice: None,
            edit: None,
            ask: None,
            dirty: true,
            atoms,
            keymap,
        };
        app.load_current()?;
        Ok(app)
    }

    fn update_title(&self) -> Res<()> {
        let title = if let Some(p) = self.files.get(self.idx) {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            let count = self.files.len();
            let mut idx_str = format!("[{}/{count}] {name}", self.idx + 1);
            if let Some(c) = &self.comic {
                idx_str += &format!(" - page {} of {}", c.page + 1, c.pages.len());
            }
            if let Some(ref m) = self.media {
                format!("{idx_str} ({}x{}) - FerroOS Viewer", m.dim.0, m.dim.1)
            } else {
                format!("{idx_str} - FerroOS Viewer")
            }
        } else {
            "FerroOS Viewer".into()
        };

        self.conn.change_property8(PropMode::REPLACE, self.win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        self.conn.change_property8(PropMode::REPLACE, self.win, self.atoms.net_wm_name, self.atoms.utf8_string, title.as_bytes())?;
        Ok(())
    }

    fn load_current(&mut self) -> Res<()> {
        self.load_file(false)
    }

    /// The file at `idx`; a comic at its first page, or its last (paging
    /// back into it from the next file).
    fn load_file(&mut self, last_page: bool) -> Res<()> {
        self.media = None;
        self.comic = None;
        if let Some(p) = self.files.get(self.idx) {
            if is_comic(p) {
                self.comic = Comic::open(p);
                if let Some(c) = &mut self.comic {
                    c.page = if last_page { c.pages.len() - 1 } else { 0 };
                    self.media = c.load_page();
                }
            } else {
                self.media = LoadedMedia::load(p);
            }
        }
        self.pan_x = 0;
        self.pan_y = 0;
        self.page_top();
        self.update_title()?;
        self.paint()
    }

    /// A comic's page from its top (where reading starts), or its bottom
    /// (scrolling back up into it from the next page).
    fn page_top(&mut self) {
        self.page_edge(true);
    }

    fn page_edge(&mut self, top: bool) {
        if self.comic.is_none() {
            return;
        }
        self.pan_y = if top { i32::MAX } else { i32::MIN };
        self.keep_in_sight();
    }

    /// How far the picture can still move down (`room.0`, toward its top)
    /// and up (`room.1`, toward its bottom) in the window.
    fn scroll_room(&self) -> (i32, i32) {
        let Some(f) = self.shown() else { return (0, 0) };
        let vp = self.viewport_rect();
        let (_, _, _, _, dh) = self.compute_scale_and_pos(vp, f.width, f.height);
        let half = (dh - vp.h).max(0) / 2;
        (half - self.pan_y, self.pan_y + half)
    }

    /// The wheel in a comic reads down the page, and on to the next: a
    /// whole book without leaving the wheel. At a page's end the next
    /// notch turns the page (so a page is never skipped by a fast spin
    /// that only meant to reach its bottom).
    fn wheel_read(&mut self, down: bool) -> Res<()> {
        let step = (self.viewport_rect().h / 5).max(40);
        let (above, below) = self.scroll_room();
        if down {
            if below > 0 {
                self.pan_y -= step.min(below);
                return self.paint();
            }
            self.next_image()
        } else {
            if above > 0 {
                self.pan_y += step.min(above);
                return self.paint();
            }
            self.prev_image_at(false)
        }
    }

    /// Turns to page `n` of the comic open, at its top.
    fn show_page(&mut self, n: usize) -> Res<()> {
        self.show_page_at(n, true)
    }

    fn show_page_at(&mut self, n: usize, top: bool) -> Res<()> {
        let Some(c) = &mut self.comic else { return Ok(()) };
        c.page = n.min(c.pages.len() - 1);
        self.media = c.load_page();
        self.pan_x = 0;
        self.pan_y = 0;
        self.page_edge(top);
        self.update_title()?;
        self.paint()
    }

    /// The next page of a comic (after its last, its first again: a comic
    /// is read on its own, not run on into the folder's other files), or
    /// the next file.
    fn next_image(&mut self) -> Res<()> {
        if let Some(c) = &self.comic {
            let n = if c.page + 1 < c.pages.len() { c.page + 1 } else { 0 };
            return self.show_page(n);
        }
        if self.files.is_empty() {
            return Ok(());
        }
        self.idx = (self.idx + 1) % self.files.len();
        self.load_file(false)
    }

    fn prev_image(&mut self) -> Res<()> {
        self.prev_image_at(true)
    }

    /// The page before (before the first, the last), at its top or bottom.
    fn prev_image_at(&mut self, top: bool) -> Res<()> {
        if let Some(c) = &self.comic {
            let n = if c.page > 0 { c.page - 1 } else { c.pages.len() - 1 };
            return self.show_page_at(n, top);
        }
        if self.files.is_empty() {
            return Ok(());
        }
        self.idx = if self.idx == 0 { self.files.len() - 1 } else { self.idx - 1 };
        self.load_file(true)
    }

    /// Delete (the key or the button): asks first. Yes is the default
    /// button, as on Windows 95: Enter or Y moves the picture to the
    /// Recycle Bin (where it can be got back), or, on a removable drive,
    /// deletes it for good, written over (it's never copied onto the main
    /// drive); Esc, N and Delete again leave it alone. In a comic it's the
    /// whole .cbz: nothing's ever taken out of one.
    fn ask_delete(&mut self) -> Res<()> {
        if let Some(p) = self.files.get(self.idx) {
            self.confirm_delete = Some((p.clone(), !ferro_associations::trash::can_take(p)));
            self.paint()?;
        }
        Ok(())
    }

    fn answer_delete(&mut self, yes: bool) -> Res<()> {
        let Some((path, for_good)) = self.confirm_delete.take() else { return Ok(()) };
        if yes {
            let done = if for_good { ferro_associations::trash::shred(&path) } else { ferro_associations::trash::put(&path).map(drop) };
            match done {
                Ok(()) => {
                    if let Some(i) = self.files.iter().position(|f| *f == path) {
                        self.files.remove(i);
                    }
                    if self.idx >= self.files.len() && !self.files.is_empty() {
                        self.idx = self.files.len() - 1;
                    }
                    return self.load_current();
                }
                Err(e) if for_good => self.notice = Some(format!("Couldn't delete it: {e}")),
                Err(e) => self.notice = Some(format!("Couldn't move it to the Recycle Bin: {e}")),
            }
        }
        self.paint()
    }

    /// Edit mode on, with the picture showing (an animation's frame
    /// showing, as a still: the frames' timing can't be saved, so it's
    /// edited and saved as the one picture).
    fn enter_edit(&mut self) -> Res<()> {
        let Some(frame) = self.media.as_ref().and_then(|m| m.current_frame()).cloned() else { return Ok(()) };
        let Some(path) = self.files.get(self.idx) else { return Ok(()) };
        let file_stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let (stem, jpeg) = match &self.comic {
            // A page is saved beside the .cbz, named after both.
            Some(c) => {
                let page = Path::new(c.page_name()).file_stem().map(|s| s.to_string_lossy().into_owned());
                let page = page.unwrap_or_else(|| format!("page{}", c.page + 1));
                (format!("{file_stem}-{page}"), is_jpeg_name(c.page_name()))
            }
            None => (file_stem, is_jpeg_name(&path.to_string_lossy())),
        };
        self.slideshow = false;
        self.edit = Some(Editing::new(frame, stem, jpeg));
        self.paint()
    }

    /// Out of edit mode, or quitting: asks first if there are changes.
    fn request_leave(&mut self, after: After) -> Res<()> {
        if self.edit.as_ref().is_some_and(|e| e.unsaved()) {
            self.ask = Some(Ask::Unsaved(after));
            return self.paint();
        }
        self.finish(after)
    }

    fn finish(&mut self, after: After) -> Res<()> {
        match after {
            After::Stay => {}
            After::Leave => {
                self.edit = None;
                self.boxing = None;
                self.zoom = ZoomMode::Fit;
                self.pan_x = 0;
                self.pan_y = 0;
            }
            After::Quit => std::process::exit(0),
        }
        self.paint()
    }

    /// Save As: asks for the new file's name, a free one next to the
    /// original offered.
    fn ask_save(&mut self, after: After) -> Res<()> {
        let (Some(e), Some(dir)) = (&self.edit, self.save_dir()) else { return Ok(()) };
        let name = free_name(&dir, &e.stem, if e.jpeg { "jpg" } else { "png" });
        // The name without its extension selected, ready to type over.
        let stem_len = name.rfind('.').unwrap_or(name.len());
        self.ask = Some(Ask::SaveAs(TextEdit::with_selection(&name, 0, stem_len), after));
        self.paint()
    }

    /// The folder the picture (or the comic) is in.
    fn save_dir(&self) -> Option<PathBuf> {
        Some(self.files.get(self.idx)?.parent()?.to_path_buf())
    }

    /// Save in the Save As box: saves, unless the name's taken (asks).
    fn save_typed(&mut self) -> Res<()> {
        let Some(Ask::SaveAs(field, after)) = self.ask.take() else { return Ok(()) };
        let (Some(e), Some(dir)) = (&self.edit, self.save_dir()) else { return self.paint() };
        let Some(name) = typed_name(field.text(), if e.jpeg { "jpg" } else { "png" }) else {
            self.notice = Some("That isn't a file name (no / in it).".into());
            self.ask = Some(Ask::SaveAs(field, after));
            return self.paint();
        };
        let path = dir.join(name);
        if path.exists() {
            self.ask = Some(Ask::Replace(path, after));
            return self.paint();
        }
        self.write_edit(&path, false, after)
    }

    /// Writes the edited picture to `path`, as a PNG or (named .jpg) a JPEG.
    fn write_edit(&mut self, path: &Path, replace: bool, after: After) -> Res<()> {
        let Some(e) = &mut self.edit else { return Ok(()) };
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let Some(bytes) = encode(&e.image, is_jpeg_name(&name)) else {
            self.notice = Some("Couldn't make the picture to save.".into());
            return self.paint();
        };
        let written = if replace {
            fs::File::create(path)
        } else {
            // Never over a file that turned up since.
            fs::OpenOptions::new().write(true).create_new(true).open(path)
        }
        .and_then(|mut f| f.write_all(&bytes).and_then(|()| f.sync_all()));
        if let Err(err) = written {
            self.notice = Some(format!("Couldn't save it: {err}"));
            return self.paint();
        }
        e.saved = Some(e.undo.len());
        self.add_file(path);
        if after == After::Quit {
            std::process::exit(0);
        }
        self.notice = Some(format!("Saved as '{}'.", ellipsize(&name, 30)));
        self.finish(after)
    }

    /// A file just saved goes in the list too, in its place, if it's in the
    /// same folder (and not there already).
    fn add_file(&mut self, path: &Path) {
        let same_folder = self.files.get(self.idx).and_then(|f| f.parent()) == path.parent();
        if !same_folder || !is_supported_image(path) || self.files.iter().any(|f| f == path) {
            return;
        }
        let current = self.files[self.idx].clone();
        let at = self.files.partition_point(|f| natural_path_cmp(f, path) == Ordering::Less);
        self.files.insert(at, path.to_path_buf());
        self.idx = self.files.iter().position(|f| *f == current).unwrap_or(0);
        let _ = self.update_title();
    }

    /// An answer to edit mode's questions, by key or button.
    fn answer_unsaved(&mut self, choice: Option<bool>) -> Res<()> {
        let Some(Ask::Unsaved(after)) = self.ask.take() else { return Ok(()) };
        match choice {
            Some(true) => self.ask_save(after),
            Some(false) => {
                // Let go: the changes go with it.
                if let Some(e) = &mut self.edit {
                    e.saved = Some(e.undo.len());
                }
                self.finish(after)
            }
            None => self.paint(),
        }
    }

    fn answer_replace(&mut self, yes: bool) -> Res<()> {
        let Some(Ask::Replace(path, after)) = self.ask.take() else { return Ok(()) };
        if yes {
            return self.write_edit(&path, true, after);
        }
        // Back to the name, to type another.
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        self.ask = Some(Ask::SaveAs(TextEdit::new(&name), after));
        self.paint()
    }

    /// The question (or a notice) over the picture, with its buttons.
    fn draw_dialog(&self, s: &mut Surface) {
        let r = dialog_rect(self.w, self.h);
        let frame = |s: &mut Surface, title: &str| {
            s.fill(r, color::FACE);
            s.bevel(r, Bevel::Window);
            let bar = Rect::new(r.x + 3, r.y + 3, r.w - 6, 18);
            s.fill(bar, color::NAVY);
            s.text_bold(bar.x + 6, bar.y + 3, title, color::WHITE);
        };
        let lines = |s: &mut Surface, lines: &[String]| {
            for (i, l) in lines.iter().enumerate() {
                s.text(r.x + 16, r.y + 34 + i as i32 * (GLYPH_H + 6), l, color::BLACK);
            }
        };
        let (yes, no) = dialog_buttons(r);
        if let Some(n) = &self.notice {
            frame(s, "Image Viewer");
            lines(s, &[ellipsize(n, 46)]);
            default_button(s, no, "OK", true);
            return;
        }
        if let Some((p, for_good)) = &self.confirm_delete {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            let keep = "Press Enter for Yes, or Esc to keep it.".to_string();
            frame(s, "Confirm Delete");
            if *for_good {
                let first = format!("'{}' is on a removable drive,", ellipsize(&name, 14));
                lines(s, &[first, "so it can't go to the Recycle Bin.".into(), "Delete it for good? It can't be got back.".into(), keep]);
            } else {
                lines(s, &[format!("Move '{}' to the Recycle Bin?", ellipsize(&name, 34)), keep]);
            }
            default_button(s, yes, "Yes", true);
            button(s, no, "No", true);
            return;
        }
        match &self.ask {
            Some(Ask::Unsaved(_)) => {
                frame(s, "Image Viewer");
                lines(s, &["The picture has changes not saved.".into(), "Save them as a new file?".into()]);
                let [save, discard, cancel] = dialog_buttons3(r);
                default_button(s, save, "Save", true);
                button(s, discard, "Discard", true);
                button(s, cancel, "Cancel", true);
            }
            Some(Ask::SaveAs(field, _)) => {
                frame(s, "Save As");
                lines(s, &["Save the edited picture beside it, as:".into()]);
                let f = name_field(r);
                s.bevel(f, Bevel::Sunken);
                field.draw(s, f.inset(2));
                s.text(r.x + 16, f.bottom() + 8, "A new file: no EXIF or GPS details.", color::SHADOW);
                default_button(s, yes, "Save", true);
                button(s, no, "Cancel", true);
            }
            Some(Ask::Replace(p, _)) => {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                frame(s, "Save As");
                lines(s, &[format!("'{}' already exists.", ellipsize(&name, 30)), "Replace it?".into()]);
                default_button(s, yes, "Yes", true);
                button(s, no, "No", true);
            }
            None => {}
        }
    }

    fn viewport_rect(&self) -> Rect {
        Rect::new(4, TOOLBAR_H + 2, (self.w - 8).max(10), (self.h - TOOLBAR_H - STATUS_H - 4).max(10))
    }

    /// The picture showing: edit mode's copy, or the file's (its frame now).
    fn shown(&self) -> Option<&DecodedImage> {
        match &self.edit {
            Some(e) => Some(&e.image),
            None => self.media.as_ref()?.current_frame(),
        }
    }

    /// Where the picture is drawn and how big, and its own size.
    fn placement(&self) -> Option<(Rect, (usize, usize))> {
        let f = self.shown()?;
        let (_, dx, dy, dw, dh) = self.compute_scale_and_pos(self.viewport_rect(), f.width, f.height);
        Some((Rect::new(dx, dy, dw, dh), (f.width, f.height)))
    }

    /// The scale the picture is shown at now (Fit's too).
    fn scale_now(&self) -> Option<f32> {
        let f = self.shown()?;
        Some(self.compute_scale_and_pos(self.viewport_rect(), f.width, f.height).0)
    }

    /// Bigger or smaller by `factor`, from the scale it's shown at (not
    /// from 100% when it was fitted), keeping the point under `anchor`
    /// (the pointer) where it is, or the middle; and in sight after.
    fn zoom_by(&mut self, factor: f32, anchor: Option<(i32, i32)>) {
        let Some(old) = self.scale_now() else { return };
        let new = (old * factor).clamp(0.02, 25.0);
        let vp = self.viewport_rect();
        let (ax, ay) = anchor.unwrap_or((vp.x + vp.w / 2, vp.y + vp.h / 2));
        // The picture's middle is at the viewport's middle plus the pan;
        // the point under the anchor is (anchor - middle) / scale from it.
        let (mx, my) = (vp.x + vp.w / 2 + self.pan_x, vp.y + vp.h / 2 + self.pan_y);
        let k = new / old;
        self.pan_x = (ax as f32 - (ax - mx) as f32 * k) as i32 - (vp.x + vp.w / 2);
        self.pan_y = (ay as f32 - (ay - my) as f32 * k) as i32 - (vp.y + vp.h / 2);
        self.zoom = ZoomMode::Scale(new);
        self.keep_in_sight();
    }

    /// Picture box `r` as big as fits in the window.
    fn zoom_to(&mut self, r: Rect) {
        let Some((_, size)) = self.placement() else { return };
        let (scale, px, py) = zoom_to_box(self.viewport_rect(), size, r);
        self.zoom = ZoomMode::Scale(scale);
        (self.pan_x, self.pan_y) = (px, py);
        self.keep_in_sight();
    }

    /// No dragging or zooming the picture out of sight: smaller than the
    /// window, it's in the middle; bigger, its edges stay at the window's.
    fn keep_in_sight(&mut self) {
        let Some((fw, fh)) = self.shown().map(|f| (f.width, f.height)) else { return };
        let vp = self.viewport_rect();
        let (_, _, _, dw, dh) = self.compute_scale_and_pos(vp, fw, fh);
        let room = |draw: i32, view: i32, pan: i32| if draw <= view { 0 } else { pan.clamp(-(draw - view) / 2, (draw - view) / 2) };
        self.pan_x = room(dw, vp.w, self.pan_x);
        self.pan_y = room(dh, vp.h, self.pan_y);
    }

    fn compute_scale_and_pos(&self, vp: Rect, img_w: usize, img_h: usize) -> (f32, i32, i32, i32, i32) {
        let iw = img_w as f32;
        let ih = img_h as f32;
        let vw = vp.w as f32;
        let vh = vp.h as f32;

        let scale = match self.zoom {
            // A comic's page as wide as the window, read top to bottom: the
            // panels big enough to see and the small writing to read.
            ZoomMode::Fit if self.comic.is_some() && self.edit.is_none() => (vw / iw).min(10.0),
            ZoomMode::Fit => (vw / iw).min(vh / ih).min(10.0),
            ZoomMode::Scale(s) => s.clamp(0.05, 30.0),
        };

        let draw_w = (iw * scale).round() as i32;
        let draw_h = (ih * scale).round() as i32;

        let center_x = vp.x + (vp.w - draw_w) / 2 + self.pan_x;
        let center_y = vp.y + (vp.h - draw_h) / 2 + self.pan_y;

        (scale, center_x, center_y, draw_w, draw_h)
    }

    /// The picture resized for the window, made once for each picture and
    /// size and kept (scrolling and redrawing reuse it). Not in edit mode,
    /// whose pixels are what gets saved, nor for animations; not past
    /// `SMOOTH_MOST` pixels (zoomed far in), where pixels are what's wanted.
    fn prepare_smooth(&mut self) {
        let animated = self.media.as_ref().is_some_and(|m| m.animation.is_some());
        // Comics only: smoothing is for lettering. A photo shrunk to the
        // window that way loses its finest detail (stars in a night sky
        // faded away a moment after it opened).
        if !self.smooth || self.comic.is_none() || self.edit.is_some() || animated {
            self.smoothed = None;
            self.smoothing = None;
            return;
        }
        let vp = self.viewport_rect();
        let Some(frame) = self.shown() else { return };
        let (_, _, _, dw, dh) = self.compute_scale_and_pos(vp, frame.width, frame.height);
        let key = (frame.pixels.as_ptr() as usize, frame.width, frame.height, dw, dh);
        let same_size = dw as usize == frame.width && dh as usize == frame.height;
        if same_size || dw <= 0 || dh <= 0 || dw as usize * dh as usize > SMOOTH_MOST {
            self.smoothed = None;
            return;
        }
        if self.smoothed.as_ref().is_some_and(|(k, _)| *k == key) || self.smoothing.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        // Aside, so turning a page never waits: it shows at once as is, and
        // smooth when ready (`collect_smooth`).
        let copy = DecodedImage::new(frame.width, frame.height, frame.pixels.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(smooth_resize(&copy, dw as usize, dh as usize));
        });
        self.smoothing = Some((key, rx));
    }

    /// The resized picture, if it's ready: drawn again with it.
    fn collect_smooth(&mut self) {
        let Some((key, rx)) = &self.smoothing else { return };
        match rx.try_recv() {
            Ok(img) => {
                self.smoothed = Some((*key, img));
                self.smoothing = None;
                self.dirty = true;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.smoothing = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    fn draw_image_scaled(s: &mut Surface, vp: Rect, img: &DecodedImage, dx: i32, dy: i32, dw: i32, dh: i32) {
        if dw <= 0 || dh <= 0 {
            return;
        }
        let iw = img.width;
        let ih = img.height;

        let start_x = dx.max(vp.x);
        let end_x = (dx + dw).min(vp.right());
        let start_y = dy.max(vp.y);
        let end_y = (dy + dh).min(vp.bottom());

        if start_x >= end_x || start_y >= end_y {
            return;
        }

        // Which picture column each screen column shows: the same on every row.
        let src_xs: Vec<usize> = (start_x..end_x).map(|x| ((x - dx) as usize * iw / dw as usize).min(iw - 1)).collect();
        for y in start_y..end_y {
            let src_y = ((y - dy) as usize * ih / dh as usize).min(ih - 1);
            let row = &img.pixels[src_y * iw..(src_y + 1) * iw];
            for (x, &src_x) in (start_x..end_x).zip(&src_xs) {
                s.put(x, y, row[src_x]);
            }
        }
    }

    /// The box as a dashed black-and-white outline, seen on any picture,
    /// two pixels wide; only what's inside `clip`.
    fn draw_box(s: &mut Surface, r: Rect, clip: Rect) {
        let dash = |i: i32| if (i / 4) % 2 == 0 { color::BLACK } else { color::WHITE };
        let mut dot = |x: i32, y: i32, c: u32| {
            if clip.contains(x, y) {
                s.put(x, y, c);
            }
        };
        let (x0, y0, x1, y1) = (r.x, r.y, r.right() - 1, r.bottom() - 1);
        for x in x0..=x1 {
            for (y, k) in [(y0, 0), (y0 + 1, 4), (y1, 0), (y1 - 1, 4)] {
                dot(x, y, dash(x - x0 + k));
            }
        }
        for y in y0..=y1 {
            for (x, k) in [(x0, 0), (x0 + 1, 4), (x1, 0), (x1 - 1, 4)] {
                dot(x, y, dash(y - y0 + k));
            }
        }
    }

    /// The box showing: the one being dragged, or edit mode's.
    fn box_now(&self) -> Option<Rect> {
        if let Some(b) = &self.boxing {
            let (at, size) = self.placement()?;
            return box_from_drag(b.from, b.to, at, size);
        }
        self.edit.as_ref()?.sel
    }

    /// Asks for the window to be drawn; `render` draws it.
    fn paint(&mut self) -> Res<()> {
        self.dirty = true;
        Ok(())
    }

    fn render(&mut self) -> Res<()> {
        self.prepare_smooth();
        self.dirty = false;
        let mut s = Surface::new(self.w as usize, self.h as usize);
        s.fill(Rect::new(0, 0, self.w, self.h), color::FACE);

        // Toolbar Buttons
        let has = self.media.is_some();
        if let Some(e) = &self.edit {
            let sel = e.sel.is_some();
            button(&mut s, UNDO_BTN, "Undo", !e.undo.is_empty());
            button(&mut s, SAVE_BTN, "Save As...", true);
            button(&mut s, ZOOM_OUT_BTN, "-", true);
            button(&mut s, ZOOM_IN_BTN, "+", true);
            button(&mut s, EDIT_FIT_BTN, "Fit", true);
            button(&mut s, BOX_ZOOM_BTN, "Zoom", sel);
            button(&mut s, CROP_BTN, "Crop", sel);
            button(&mut s, BLACK_BTN, "Black box", sel);
            // Edit held down, as a toggle is on Windows 95.
            s.button(EDIT_BTN, true);
            s.text(EDIT_BTN.x + (EDIT_BTN.w - text_width("Edit")) / 2 + 1, EDIT_BTN.y + (EDIT_BTN.h - GLYPH_H) / 2 + 1, "Edit", color::BLACK);
        } else {
            let pages = self.comic.as_ref().is_some_and(|c| c.pages.len() > 1);
            button(&mut s, PREV_BTN, "< Prev", self.idx > 0 || self.files.len() > 1 || pages);
            button(&mut s, NEXT_BTN, "Next >", self.idx + 1 < self.files.len() || self.files.len() > 1 || pages);
            button(&mut s, ZOOM_OUT_BTN, "-", has);
            button(&mut s, ZOOM_IN_BTN, "+", has);
            button(&mut s, ACTUAL_BTN, "1:1", has);
            button(&mut s, FIT_BTN, "Fit", has);
            button(&mut s, SLIDESHOW_BTN, if self.slideshow { "Stop" } else { "Slideshow" }, self.files.len() > 1 || pages);
            button(&mut s, DELETE_BTN, "Delete", !self.files.is_empty());
            button(&mut s, EDIT_BTN, "Edit", has);
        }

        // Viewport Frame
        let vp = self.viewport_rect();
        s.fill(vp, 0x00202020);
        s.bevel(vp, Bevel::Sunken);

        let mut zoom_str = "Fit".to_string();
        if let Some(frame) = self.shown() {
            let (scale, dx, dy, dw, dh) = self.compute_scale_and_pos(vp, frame.width, frame.height);
            zoom_str = match self.zoom {
                ZoomMode::Fit => format!("Fit ({:.0}%)", scale * 100.0),
                ZoomMode::Scale(s) => format!("{:.0}%", s * 100.0),
            };
            let key = (frame.pixels.as_ptr() as usize, frame.width, frame.height, dw, dh);
            let drawn = match &self.smoothed {
                Some((k, img)) if *k == key => img,
                _ => frame,
            };
            Self::draw_image_scaled(&mut s, vp.inset(1), drawn, dx, dy, dw, dh);
            if !self.smooth && self.comic.is_some() {
                zoom_str.push_str(" raw");
            }
            if let Some(b) = self.box_now() {
                let on_screen = box_on_screen(b, Rect::new(dx, dy, dw, dh), (frame.width, frame.height));
                Self::draw_box(&mut s, on_screen, vp.inset(1));
            }
        } else {
            let msg = if self.files.is_empty() { "No images in folder" } else { "Failed to load image" };
            let tx = vp.x + (vp.w - text_width(msg)) / 2;
            let ty = vp.y + (vp.h - GLYPH_H) / 2;
            s.text(tx, ty, msg, color::SHADOW);
        }

        // Status Bar (Sunken 3D Panels): the name gets what's left over.
        let sb_y = self.h - STATUS_H + 2;
        let sb_h = STATUS_H - 4;

        let fixed = 130 + 72 + 90 + 130 + 4 * 6;
        let pan1 = Rect::new(4, sb_y, (self.w - fixed).max(60), sb_h);
        let pan2 = Rect::new(pan1.right() + 4, sb_y, 130, sb_h);
        let pan3 = Rect::new(pan2.right() + 4, sb_y, 72, sb_h);
        let pan4 = Rect::new(pan3.right() + 4, sb_y, 90, sb_h);
        let pan5 = Rect::new(pan4.right() + 4, sb_y, 130, sb_h);

        for p in [pan1, pan2, pan3, pan4, pan5] {
            s.bevel(p, Bevel::Field);
        }
        let room = |p: Rect| ((p.w - 10) / GLYPH_W).max(1) as usize;
        let mut put = |p: Rect, t: &str| {
            s.text(p.x + 5, p.y + (p.h - GLYPH_H) / 2, &ellipsize(t, room(p)), color::BLACK);
        };

        if let Some(p) = self.files.get(self.idx) {
            let fname = p.file_name().unwrap_or_default().to_string_lossy();
            put(pan1, &fname);

            if let Some(ref m) = self.media {
                let (w, h) = self.shown().map_or(m.dim, |f| (f.width, f.height));
                put(pan2, &format!("{w}x{h} {}", m.format_name));

                let sz_str = if m.file_size_bytes < 1024 {
                    format!("{} B", m.file_size_bytes)
                } else if m.file_size_bytes < 1024 * 1024 {
                    format!("{:.1} KB", m.file_size_bytes as f64 / 1024.0)
                } else {
                    format!("{:.1} MB", m.file_size_bytes as f64 / (1024.0 * 1024.0))
                };
                put(pan3, &sz_str);
            }

            put(pan4, &zoom_str);

            let idx_str = if self.edit.is_some() {
                match self.box_now() {
                    Some(b) => format!("Box {}x{}", b.w, b.h),
                    None => "Drag a box".into(),
                }
            } else if let Some(c) = &self.comic {
                format!("page {} of {}", c.page + 1, c.pages.len())
            } else {
                format!("{} of {}", self.idx + 1, self.files.len())
            };
            put(pan5, &idx_str);
        }

        self.draw_dialog(&mut s);
        ferro_ui::show(&self.conn, self.win, self.gc, self.depth, &s, self.w, self.h)
    }

    /// A click while a question or notice is up: only its buttons count.
    fn click_dialog(&mut self, x: i32, y: i32) -> Res<()> {
        let r = dialog_rect(self.w, self.h);
        let (yes, no) = dialog_buttons(r);
        if self.notice.is_some() {
            if no.contains(x, y) {
                self.notice = None;
                return self.paint();
            }
            return Ok(());
        }
        if self.confirm_delete.is_some() {
            if yes.contains(x, y) {
                return self.answer_delete(true);
            }
            if no.contains(x, y) {
                return self.answer_delete(false);
            }
            return Ok(());
        }
        match &mut self.ask {
            Some(Ask::Unsaved(_)) => {
                let [save, discard, cancel] = dialog_buttons3(r);
                if save.contains(x, y) {
                    return self.answer_unsaved(Some(true));
                }
                if discard.contains(x, y) {
                    return self.answer_unsaved(Some(false));
                }
                if cancel.contains(x, y) {
                    return self.answer_unsaved(None);
                }
            }
            Some(Ask::SaveAs(field, _)) => {
                let f = name_field(r);
                if f.contains(x, y) {
                    field.click(f.inset(2), x, false);
                    return self.paint();
                }
                if yes.contains(x, y) {
                    return self.save_typed();
                }
                if no.contains(x, y) {
                    self.ask = None;
                    return self.paint();
                }
            }
            Some(Ask::Replace(..)) => {
                if yes.contains(x, y) {
                    return self.answer_replace(true);
                }
                if no.contains(x, y) {
                    return self.answer_replace(false);
                }
            }
            None => {}
        }
        Ok(())
    }

    /// A toolbar button in edit mode.
    fn click_edit_toolbar(&mut self, x: i32, y: i32) -> Res<bool> {
        let Some(e) = &mut self.edit else { return Ok(false) };
        let changed = if UNDO_BTN.contains(x, y) {
            e.undo()
        } else if CROP_BTN.contains(x, y) {
            e.crop()
        } else if BLACK_BTN.contains(x, y) {
            e.black_box()
        } else if SAVE_BTN.contains(x, y) {
            self.ask_save(After::Stay)?;
            return Ok(true);
        } else if BOX_ZOOM_BTN.contains(x, y) {
            if let Some(r) = e.sel {
                self.zoom_to(r);
                self.paint()?;
            }
            return Ok(true);
        } else if EDIT_FIT_BTN.contains(x, y) {
            self.fit();
            return Ok(true);
        } else if EDIT_BTN.contains(x, y) {
            self.request_leave(After::Leave)?;
            return Ok(true);
        } else {
            return Ok(false);
        };
        if changed {
            self.after_change();
        }
        Ok(true)
    }

    /// After a crop, undo or black box: the picture may be another size.
    fn after_change(&mut self) {
        if matches!(self.zoom, ZoomMode::Fit) && self.comic.is_none() {
            self.pan_x = 0;
            self.pan_y = 0;
        }
        self.keep_in_sight();
        self.dirty = true;
    }

    fn fit(&mut self) {
        self.zoom = ZoomMode::Fit;
        self.pan_x = 0;
        self.pan_y = 0;
        self.page_top();
        self.dirty = true;
    }

    fn actual_size(&mut self) {
        self.zoom = ZoomMode::Scale(1.0);
        self.pan_x = 0;
        self.pan_y = 0;
        self.dirty = true;
    }

    fn click(&mut self, e: &ButtonPressEvent) -> Res<()> {
        let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
        let now = Instant::now();
        if self.confirm_delete.is_some() || self.notice.is_some() || self.ask.is_some() {
            return self.click_dialog(x, y);
        }

        if y < TOOLBAR_H && e.detail == 1 {
            if self.edit.is_some() {
                if self.click_edit_toolbar(x, y)? {
                    return Ok(());
                }
            } else if PREV_BTN.contains(x, y) {
                return self.prev_image();
            } else if NEXT_BTN.contains(x, y) {
                return self.next_image();
            } else if ACTUAL_BTN.contains(x, y) {
                self.actual_size();
                return Ok(());
            } else if FIT_BTN.contains(x, y) {
                self.fit();
                return Ok(());
            } else if SLIDESHOW_BTN.contains(x, y) {
                self.slideshow = !self.slideshow;
                self.last_slide_time = now;
                return self.paint();
            } else if DELETE_BTN.contains(x, y) {
                return self.ask_delete();
            } else if EDIT_BTN.contains(x, y) {
                return self.enter_edit();
            }
            if ZOOM_OUT_BTN.contains(x, y) {
                self.zoom_by(0.75, None);
                return self.paint();
            }
            if ZOOM_IN_BTN.contains(x, y) {
                self.zoom_by(1.33, None);
                return self.paint();
            }
        }

        let vp = self.viewport_rect();
        if vp.contains(x, y) {
            let ctrl = e.state.contains(KeyButMask::CONTROL);
            match e.detail {
                // Edit mode: the box. Anywhere, the right button (or Ctrl)
                // drags one to zoom to.
                1 if self.edit.is_some() || ctrl => {
                    self.boxing = Some(Boxing { from: (x, y), to: (x, y), zoom: self.edit.is_none() });
                }
                3 => self.boxing = Some(Boxing { from: (x, y), to: (x, y), zoom: true }),
                1 => {
                    // Check double click -> toggle fit / 100%
                    let is_double = now.duration_since(self.last_click.0).as_millis() < 400
                        && (x - self.last_click.1).abs() < 5
                        && (y - self.last_click.2).abs() < 5;

                    self.last_click = (now, x, y);
                    if is_double {
                        self.zoom = match self.zoom {
                            ZoomMode::Fit => ZoomMode::Scale(1.0),
                            ZoomMode::Scale(_) => ZoomMode::Fit,
                        };
                        self.pan_x = 0;
                        self.pan_y = 0;
                        return self.paint();
                    }
                    self.start_pan(x, y);
                }
                // The middle button moves the picture, in edit mode too.
                2 => self.start_pan(x, y),
                // In a comic the wheel reads on (Ctrl+wheel still zooms).
                4 | 5 if self.comic.is_some() && !e.state.contains(KeyButMask::CONTROL) => {
                    return self.wheel_read(e.detail == 5);
                }
                // The wheel: toward what's under the pointer.
                4 => {
                    self.zoom_by(1.2, Some((x, y)));
                    return self.paint();
                }
                5 => {
                    self.zoom_by(0.8, Some((x, y)));
                    return self.paint();
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn start_pan(&mut self, x: i32, y: i32) {
        self.dragging = true;
        self.drag_start = (x, y);
        self.pan_start = (self.pan_x, self.pan_y);
    }

    fn motion(&mut self, e: &MotionNotifyEvent) -> Res<()> {
        let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
        if let Some(b) = &mut self.boxing {
            b.to = (x, y);
            return self.paint();
        }
        if self.dragging {
            let dx = x - self.drag_start.0;
            let dy = y - self.drag_start.1;
            self.pan_x = self.pan_start.0 + dx;
            self.pan_y = self.pan_start.1 + dy;
            self.keep_in_sight();
            self.paint()?;
        }
        Ok(())
    }

    fn release(&mut self, e: &ButtonReleaseEvent) -> Res<()> {
        self.dragging = false;
        let Some(mut b) = self.boxing.take() else { return Ok(()) };
        b.to = (i32::from(e.event_x), i32::from(e.event_y));
        let r = self.placement().and_then(|(at, size)| box_from_drag(b.from, b.to, at, size));
        if b.zoom {
            if let Some(r) = r {
                self.zoom_to(r);
            }
        } else if let Some(ed) = &mut self.edit {
            // A click (no box) clears it.
            ed.sel = r;
        }
        self.paint()
    }

    /// Keys while a question or notice is up.
    fn key_dialog(&mut self, e: &KeyPressEvent) -> Res<()> {
        if self.notice.take().is_some() {
            return self.paint();
        }
        if self.confirm_delete.is_some() {
            return self.answer_delete(matches!(e.detail, KEY_Y | KEY_RETURN | KEY_KP_ENTER));
        }
        match &mut self.ask {
            Some(Ask::Unsaved(_)) => match e.detail {
                KEY_RETURN | KEY_KP_ENTER | KEY_S | KEY_Y => self.answer_unsaved(Some(true)),
                KEY_D | KEY_N => self.answer_unsaved(Some(false)),
                KEY_ESCAPE => self.answer_unsaved(None),
                _ => Ok(()),
            },
            Some(Ask::SaveAs(field, _)) => {
                let key = self.keymap.typed(e);
                match field.key(key, e.state.contains(KeyButMask::CONTROL), e.state.contains(KeyButMask::SHIFT)) {
                    Edit::Done => self.save_typed(),
                    Edit::Cancel => {
                        self.ask = None;
                        self.paint()
                    }
                    Edit::Changed => self.paint(),
                    Edit::Ignored => Ok(()),
                }
            }
            Some(Ask::Replace(..)) => self.answer_replace(matches!(e.detail, KEY_Y | KEY_RETURN | KEY_KP_ENTER)),
            None => Ok(()),
        }
    }

    /// Keys in edit mode: C crops, B blacks out the box, Z zooms to it,
    /// Ctrl+Z undoes, Ctrl+S saves as; the arrows move the picture; E or
    /// Esc (with no box) leave.
    fn key_edit(&mut self, e: &KeyPressEvent) -> Res<()> {
        let ctrl = e.state.contains(KeyButMask::CONTROL);
        let Some(ed) = &mut self.edit else { return Ok(()) };
        let changed = match e.detail {
            KEY_Z if ctrl => ed.undo(),
            KEY_S if ctrl => return self.ask_save(After::Stay),
            KEY_C => ed.crop(),
            KEY_B => ed.black_box(),
            KEY_Z => {
                if let Some(r) = ed.sel {
                    self.zoom_to(r);
                }
                return self.paint();
            }
            KEY_ESCAPE if ed.sel.is_some() => {
                ed.sel = None;
                return self.paint();
            }
            KEY_ESCAPE | KEY_E => return self.request_leave(After::Leave),
            KEY_Q => return self.request_leave(After::Quit),
            KEY_PLUS | KEY_KP_ADD => {
                self.zoom_by(1.25, None);
                return self.paint();
            }
            KEY_MINUS | KEY_KP_SUB => {
                self.zoom_by(0.8, None);
                return self.paint();
            }
            KEY_F => {
                self.fit();
                return Ok(());
            }
            KEY_1 => {
                self.actual_size();
                return Ok(());
            }
            KEY_LEFT | KEY_RIGHT | KEY_UP | KEY_DOWN => {
                let (dx, dy) = match e.detail {
                    KEY_LEFT => (30, 0),
                    KEY_RIGHT => (-30, 0),
                    KEY_UP => (0, 30),
                    _ => (0, -30),
                };
                self.pan_x += dx;
                self.pan_y += dy;
                self.keep_in_sight();
                return self.paint();
            }
            _ => false,
        };
        if changed {
            self.after_change();
        }
        Ok(())
    }

    fn key(&mut self, e: &KeyPressEvent) -> Res<()> {
        if self.confirm_delete.is_some() || self.notice.is_some() || self.ask.is_some() {
            return self.key_dialog(e);
        }
        if self.edit.is_some() {
            return self.key_edit(e);
        }
        match e.detail {
            KEY_ESCAPE | KEY_Q => std::process::exit(0),
            KEY_LEFT | KEY_BACKSPACE | KEY_PAGE_UP => self.prev_image()?,
            KEY_RIGHT | KEY_SPACE | KEY_RETURN | KEY_PAGE_DOWN => self.next_image()?,
            // A comic's first and last page.
            KEY_HOME if self.comic.is_some() => self.show_page(0)?,
            KEY_END if self.comic.is_some() => self.show_page(usize::MAX)?,
            KEY_PLUS | KEY_KP_ADD => {
                self.zoom_by(1.25, None);
                self.paint()?;
            }
            KEY_MINUS | KEY_KP_SUB => {
                self.zoom_by(0.8, None);
                self.paint()?;
            }
            KEY_F => self.fit(),
            KEY_1 => self.actual_size(),
            KEY_S => {
                self.slideshow = !self.slideshow;
                self.last_slide_time = Instant::now();
                self.paint()?;
            }
            KEY_E => self.enter_edit()?,
            KEY_T => {
                self.smooth = !self.smooth;
                self.paint()?;
            }
            KEY_DELETE => self.ask_delete()?,
            KEY_UP => {
                self.pan_y += 30;
                self.paint()?;
            }
            KEY_DOWN => {
                self.pan_y -= 30;
                self.paint()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The window's close button: asks first if there are edits not saved.
    /// `true` to close now.
    fn close(&mut self) -> Res<bool> {
        if self.edit.as_ref().is_some_and(|e| e.unsaved()) {
            self.notice = None;
            self.confirm_delete = None;
            self.ask = Some(Ask::Unsaved(After::Quit));
            self.paint()?;
            return Ok(false);
        }
        Ok(true)
    }

    fn tick(&mut self) -> Res<()> {
        self.collect_smooth();
        let now = Instant::now();
        let mut need_paint = false;

        // An animation holds still while its frame's being edited.
        if let (Some(m), None) = (&mut self.media, &self.edit) {
            if m.advance_animation(now) {
                need_paint = true;
            }
        }

        if self.slideshow && self.edit.is_none() && now.duration_since(self.last_slide_time) >= Duration::from_millis(3200) {
            self.last_slide_time = now;
            self.next_image()?;
            need_paint = false; // next_image paints
        }

        if need_paint {
            self.paint()?;
        }
        Ok(())
    }
}

fn main() -> Res<()> {
    let args: Vec<String> = env::args().collect();
    let initial_path = args.get(1).map(PathBuf::from);

    // Pictures come from anywhere. A booby-trapped one that took the
    // viewer over still couldn't reach the internet or start a program.
    #[cfg(target_os = "linux")]
    lock_down()?;
    let (conn, screen_num) = RustConnection::connect(None)?;
    let mut app = App::new(conn, screen_num, initial_path.as_deref())?;
    app.paint()?;

    loop {
        while let Some(event) = app.conn.poll_for_event()? {
            match event {
                Event::Expose(_) => app.paint()?,
                Event::ConfigureNotify(e) => {
                    let nw = i32::from(e.width);
                    let nh = i32::from(e.height);
                    if nw != app.w || nh != app.h {
                        app.w = nw;
                        app.h = nh;
                        app.paint()?;
                    }
                }
                Event::ButtonPress(e) => app.click(&e)?,
                Event::ButtonRelease(e) => app.release(&e)?,
                Event::MotionNotify(e) => app.motion(&e)?,
                Event::KeyPress(e) => app.key(&e)?,
                // Closed: at once, unless there are edits to ask about.
                Event::ClientMessage(e) if e.data.as_data32()[0] == app.atoms.wm_delete && app.close()? => return Ok(()),
                _ => {}
            }
        }

        app.tick()?;
        if app.dirty {
            app.render()?;
        }
        app.conn.flush()?;
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// No internet, no starting programs, none of the kernel's riskier corners.
#[cfg(target_os = "linux")]
fn lock_down() -> Res<()> {
    use ferro_sandbox::linux;
    linux::lock_privileges()?;
    linux::install_seccomp()?;
    linux::install_net_gate(ferro_sandbox::NetMode::Deny)?;
    if let Err(e) = linux::restrict_exec(&[]) {
        // Only a kernel without Landlock; the rest still holds.
        if linux::landlock_abi() >= 1 {
            return Err(e.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_counts_numbers() {
        let mut names = vec!["page10.jpg", "Page2.jpg", "page1.jpg", "page02b.jpg", "cover.jpg", "page2.jpg"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["cover.jpg", "page1.jpg", "Page2.jpg", "page2.jpg", "page02b.jpg", "page10.jpg"]);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("ch1/p9", "ch1/p10"), Ordering::Less);
        assert_eq!(natural_cmp("ch2/p1", "ch10/p1"), Ordering::Less);
        assert_eq!(natural_cmp("x", "x1"), Ordering::Less);
        // A number too long for any integer type still sorts.
        assert_eq!(natural_cmp("p99999999999999999999999", "p100000000000000000000000"), Ordering::Less);
    }

    /// Both toolbars, each button's label and place.
    fn toolbars() -> [Vec<(Rect, &'static str)>; 2] {
        [
            vec![
                (PREV_BTN, "< Prev"),
                (NEXT_BTN, "Next >"),
                (ZOOM_OUT_BTN, "-"),
                (ZOOM_IN_BTN, "+"),
                (ACTUAL_BTN, "1:1"),
                (FIT_BTN, "Fit"),
                (SLIDESHOW_BTN, "Slideshow"),
                (DELETE_BTN, "Delete"),
                (EDIT_BTN, "Edit"),
            ],
            vec![
                (UNDO_BTN, "Undo"),
                (SAVE_BTN, "Save As..."),
                (ZOOM_OUT_BTN, "-"),
                (ZOOM_IN_BTN, "+"),
                (EDIT_FIT_BTN, "Fit"),
                (BOX_ZOOM_BTN, "Zoom"),
                (CROP_BTN, "Crop"),
                (BLACK_BTN, "Black box"),
                (EDIT_BTN, "Edit"),
            ],
        ]
    }

    #[test]
    fn toolbars_fit_the_narrowest_window() {
        for bar in toolbars() {
            for (i, (r, label)) in bar.iter().enumerate() {
                assert!(r.right() + 4 <= MIN_W, "{label} is past the window's edge");
                assert!(text_width(label) + 4 <= r.w, "{label} doesn't fit its button");
                assert!(bar[i + 1..].iter().all(|(o, _)| !o.overlaps(r)), "{label} overlaps another button");
            }
        }
    }

    fn picture(w: usize, h: usize) -> DecodedImage {
        DecodedImage::new(w, h, (0..(w * h) as u32).collect())
    }

    #[test]
    fn crop_keeps_just_the_box() {
        let img = picture(4, 3);
        let c = crop(&img, Rect::new(1, 1, 2, 2));
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(c.pixels, [5, 6, 9, 10]);
        // A box past the edge is cut to the picture.
        let c = crop(&img, Rect::new(2, 1, 10, 10));
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(c.pixels, [6, 7, 10, 11]);
    }

    #[test]
    fn black_box_writes_over_the_pixels() {
        let mut img = DecodedImage::new(3, 3, vec![0x00FF_FFFF; 9]);
        black_box(&mut img, Rect::new(1, 0, 5, 2));
        let w = 0x00FF_FFFF;
        assert_eq!(img.pixels, [w, 0, 0, w, 0, 0, w, w, w]);
        // And it's black in a file saved from it too.
        let png = encode(&img, false).unwrap();
        assert_eq!(DecodedImage::decode(&png).unwrap(), img);
    }

    #[test]
    fn saved_files_carry_no_metadata() {
        let img = picture(8, 8);
        for jpeg in [false, true] {
            let bytes = encode(&img, jpeg).unwrap();
            let decoded = DecodedImage::decode(&bytes).unwrap();
            assert_eq!((decoded.width, decoded.height), (8, 8));
            assert!(!bytes.windows(4).any(|w| w == b"Exif" || w == b"eXIf" || w == b"tEXt"));
        }
    }

    #[test]
    fn box_from_drag_is_in_picture_pixels() {
        // A 100x50 picture drawn at 2x, its corner at (10, 20).
        let at = Rect::new(10, 20, 200, 100);
        let r = box_from_drag((50, 60), (30, 40), at, (100, 50)).unwrap();
        assert_eq!(r, Rect::new(10, 10, 11, 11));
        // Back on the screen, it covers the pixels dragged over.
        assert_eq!(box_on_screen(r, at, (100, 50)), Rect::new(30, 40, 22, 22));
        // Panned elsewhere and zoomed out, it's the same picture box.
        let small = Rect::new(-40, 5, 50, 25);
        let s = box_on_screen(r, small, (100, 50));
        assert_eq!(box_from_drag((s.x, s.y), (s.right() - 1, s.bottom() - 1), small, (100, 50)), Some(r));
        // Past the picture's edges: cut to it.
        assert_eq!(box_from_drag((0, 0), (1000, 1000), at, (100, 50)), Some(Rect::new(0, 0, 100, 50)));
        // A click isn't a box; nor is a drag off the picture.
        assert_eq!(box_from_drag((50, 60), (51, 61), at, (100, 50)), None);
        assert_eq!(box_from_drag((300, 60), (350, 90), at, (100, 50)), None);
    }

    #[test]
    fn zoom_to_box_fills_the_window_with_it() {
        let vp = Rect::new(0, 0, 400, 200);
        let (scale, px, py) = zoom_to_box(vp, (1000, 500), Rect::new(100, 100, 100, 50));
        assert_eq!(scale, 4.0);
        // Where compute_scale_and_pos then draws the picture's corner.
        let (dx, dy) = (vp.x + (vp.w - 4000) / 2 + px, vp.y + (vp.h - 2000) / 2 + py);
        assert_eq!(box_on_screen(Rect::new(100, 100, 100, 50), Rect::new(dx, dy, 4000, 2000), (1000, 500)), vp);
    }

    #[test]
    fn undo_goes_back_step_by_step() {
        let mut e = Editing::new(picture(4, 4), "x".into(), false);
        assert!(!e.unsaved());
        e.sel = Some(Rect::new(0, 0, 2, 2));
        assert!(e.black_box());
        e.sel = Some(Rect::new(1, 1, 2, 2));
        assert!(e.crop());
        assert_eq!((e.image.width, e.image.height), (2, 2));
        assert!(e.unsaved());
        assert!(e.undo());
        assert_eq!(e.image.pixels[0], 0);
        assert_eq!(e.image.pixels[15], 15);
        assert!(e.undo());
        assert_eq!(e.image, picture(4, 4));
        assert!(!e.unsaved());
        assert!(!e.undo());
        // Saved, undone past the save and changed again: not saved.
        e.sel = Some(Rect::new(0, 0, 1, 1));
        e.black_box();
        e.saved = Some(e.undo.len());
        e.undo();
        e.sel = Some(Rect::new(2, 2, 1, 1));
        e.black_box();
        assert!(e.unsaved());
    }

    #[test]
    fn save_names() {
        assert_eq!(typed_name(" scan-edited.png ", "png").as_deref(), Some("scan-edited.png"));
        assert_eq!(typed_name("scan", "jpg").as_deref(), Some("scan.jpg"));
        assert_eq!(typed_name("scan.JPEG", "png").as_deref(), Some("scan.JPEG"));
        assert_eq!(typed_name("../scan.png", "png"), None);
        assert_eq!(typed_name("  ", "png"), None);
        let dir = std::env::temp_dir().join(format!("ferro-viewer-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_name(&dir, "a", "png"), "a-edited.png");
        fs::write(dir.join("a-edited.png"), b"").unwrap();
        assert_eq!(free_name(&dir, "a", "png"), "a-edited-2.png");
        fs::remove_dir_all(&dir).unwrap();
    }

    fn make_zip(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        use zip::write::SimpleFileOptions;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data, deflate) in files {
            let method = if *deflate { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
            if name.ends_with('/') {
                w.add_directory(*name, SimpleFileOptions::default()).unwrap();
            } else {
                w.start_file(*name, SimpleFileOptions::default().compression_method(method)).unwrap();
                w.write_all(data).unwrap();
            }
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn comic_pages_in_reading_order() {
        let png = encode(&picture(2, 2), false).unwrap();
        let zip = make_zip(&[
            ("book/", b"", false),
            ("book/page10.png", &png, true),
            ("book/page2.png", &png, false),
            ("book/notes.txt", b"hi", false),
            ("__MACOSX/book/._page2.png", b"junk", false),
            ("book/._page3.png", b"junk", false),
            ("../../evil/page1.PNG", &png, false),
        ]);
        let pages = comic_pages(&zip).unwrap();
        let names: Vec<&str> = pages.iter().map(|p| p.1.as_str()).collect();
        assert_eq!(names, ["../../evil/page1.PNG", "book/page2.png", "book/page10.png"]);
        // Read in memory, Deflate or not.
        for (index, _) in &pages {
            let data = comic_page(&zip, *index, MAX_PAGE_BYTES).unwrap();
            assert_eq!(DecodedImage::decode(&data).unwrap(), picture(2, 2));
        }
        assert!(comic_pages(b"not a zip").is_none());
    }

    #[test]
    fn comic_page_size_is_capped() {
        // 4 MB of zeros, a few KB packed: past a 1 MB cap, it isn't read.
        let big = vec![0u8; 4 << 20];
        let zip = make_zip(&[("bomb.png", &big, true), ("small.png", &[1u8; 1000], false)]);
        let pages = comic_pages(&zip).unwrap();
        assert!(zip.len() < 100_000);
        let index = |n: &str| pages.iter().find(|p| p.1 == n).unwrap().0;
        assert_eq!(comic_page(&zip, index("bomb.png"), 1 << 20), None);
        assert_eq!(comic_page(&zip, index("bomb.png"), 4 << 20).map(|d| d.len()), Some(4 << 20));
        assert_eq!(comic_page(&zip, index("small.png"), 999), None);
        assert_eq!(comic_page(&zip, index("small.png"), 1000).map(|d| d.len()), Some(1000));
    }

    #[test]
    fn comic_page_size_lie_is_caught() {
        // An entry claiming to be small that unpacks bigger: the claimed
        // size is patched down in both headers; the cap still holds.
        let mut zip = make_zip(&[("a.png", &vec![7u8; 5000], true)]);
        let patch = |zip: &mut Vec<u8>, sig: &[u8], at: usize| {
            let p = zip.windows(4).position(|w| w == sig).unwrap();
            zip[p + at..p + at + 4].copy_from_slice(&10u32.to_le_bytes());
        };
        patch(&mut zip, b"PK\x03\x04", 22);
        patch(&mut zip, b"PK\x01\x02", 24);
        let pages = comic_pages(&zip).unwrap();
        // Either refused as damaged or cut at the cap: never 5000 bytes.
        let got = comic_page(&zip, pages[0].0, 100);
        assert!(got.is_none_or(|d| d.len() <= 100));
    }
}
