//! ferro-viewer: Fast, retro Windows 95 photo and animation viewer for FerroOS.
//!
//! Inspired by Enclave's built-in memory-only media viewer and authentic Windows 95
//! aesthetics. Supports PNG, JPEG, BMP, WebP, and looping animated GIFs with full
//! zoom, pan, slideshow, directory browsing, and delete capabilities.

use ferro_gfx::image::{DecodedImage, ImageAnimation};
use ferro_gfx::{color, ellipsize, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, Res};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const TOOLBAR_H: i32 = 32;
const STATUS_H: i32 = 22;

const PREV_BTN: Rect = Rect::new(4, 4, 68, 24);
const NEXT_BTN: Rect = Rect::new(76, 4, 68, 24);
const ZOOM_OUT_BTN: Rect = Rect::new(154, 4, 32, 24);
const ZOOM_IN_BTN: Rect = Rect::new(190, 4, 32, 24);
const ACTUAL_BTN: Rect = Rect::new(226, 4, 44, 24);
const FIT_BTN: Rect = Rect::new(274, 4, 44, 24);
const SLIDESHOW_BTN: Rect = Rect::new(328, 4, 76, 24);
const DELETE_BTN: Rect = Rect::new(414, 4, 68, 24);

const KEY_ESCAPE: u8 = 9;
const KEY_SPACE: u8 = 65;
const KEY_RETURN: u8 = 36;
const KEY_BACKSPACE: u8 = 22;
const KEY_DELETE: u8 = 119;
const KEY_LEFT: u8 = 113;
const KEY_RIGHT: u8 = 114;
const KEY_UP: u8 = 111;
const KEY_DOWN: u8 = 116;
const KEY_PLUS: u8 = 21;
const KEY_KP_ADD: u8 = 86;
const KEY_MINUS: u8 = 20;
const KEY_KP_SUB: u8 = 82;
const KEY_F: u8 = 41;
const KEY_1: u8 = 10;
const KEY_S: u8 = 39;
const KEY_Q: u8 = 24;
const KEY_Y: u8 = 29;

fn dialog_rect(w: i32, h: i32) -> Rect {
    let (dw, dh) = (360, 116);
    Rect::new((w - dw) / 2, (h - dh) / 2, dw, dh)
}

/// Yes and No (or the notice's OK, in No's place).
fn dialog_buttons(r: Rect) -> (Rect, Rect) {
    (Rect::new(r.right() - 170, r.bottom() - 34, 75, 24), Rect::new(r.right() - 87, r.bottom() - 34, 75, 24))
}

fn is_supported_image(path: &Path) -> bool {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif" | "pnm" | "ppm")
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
    files.sort_by(|a, b| a.file_name().unwrap_or_default().cmp(b.file_name().unwrap_or_default()));

    let current_idx = files.iter().position(|p| p == &abs).unwrap_or(0);
    if files.is_empty() {
        files.push(abs);
    }
    (files, current_idx)
}

struct LoadedMedia {
    single: Option<DecodedImage>,
    animation: Option<ImageAnimation>,
    frame_idx: usize,
    last_frame_time: Instant,
    file_size_bytes: u64,
    dim: (usize, usize),
    format_name: String,
}

impl LoadedMedia {
    fn load(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        let file_size_bytes = bytes.len() as u64;
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();

        if ext == "gif" {
            if let Some(anim) = ImageAnimation::decode_gif(&bytes) {
                if !anim.frames.is_empty() {
                    let (w, h) = (anim.frames[0].0.width, anim.frames[0].0.height);
                    return Some(Self {
                        single: None,
                        animation: Some(anim),
                        frame_idx: 0,
                        last_frame_time: Instant::now(),
                        file_size_bytes,
                        dim: (w, h),
                        format_name: "GIF".into(),
                    });
                }
            }
        }

        if let Some(img) = DecodedImage::decode(&bytes) {
            let (w, h) = (img.width, img.height);
            let fmt = ext.to_uppercase();
            return Some(Self {
                single: Some(img),
                animation: None,
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
        if let Some(ref anim) = self.animation {
            if anim.frames.len() <= 1 {
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
    zoom: ZoomMode,
    pan_x: i32,
    pan_y: i32,
    dragging: bool,
    drag_start: (i32, i32),
    pan_start: (i32, i32),
    slideshow: bool,
    last_slide_time: Instant,
    last_click: (Instant, i32, i32),
    atoms: Atoms,
    /// "Delete this picture?" is showing, for this file.
    confirm_delete: Option<PathBuf>,
    /// What went wrong, shown until the next key or click.
    notice: Option<String>,
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
        conn.map_window(win)?;

        let media = if let Some(p) = files.get(idx) { LoadedMedia::load(p) } else { None };

        let app = Self {
            conn,
            win,
            gc,
            depth: root_depth,
            w,
            h,
            files,
            idx,
            media,
            zoom: ZoomMode::Fit,
            pan_x: 0,
            pan_y: 0,
            dragging: false,
            drag_start: (0, 0),
            pan_start: (0, 0),
            slideshow: false,
            last_slide_time: Instant::now(),
            last_click: (Instant::now(), 0, 0),
            confirm_delete: None,
            notice: None,
            atoms,
        };
        app.update_title()?;
        Ok(app)
    }

    fn update_title(&self) -> Res<()> {
        let title = if let Some(p) = self.files.get(self.idx) {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            let count = self.files.len();
            let idx_str = format!("[{}/{count}]", self.idx + 1);
            if let Some(ref m) = self.media {
                format!("{idx_str} {name} ({}x{}) - FerroOS Viewer", m.dim.0, m.dim.1)
            } else {
                format!("{idx_str} {name} - FerroOS Viewer")
            }
        } else {
            "FerroOS Viewer".into()
        };

        self.conn.change_property8(PropMode::REPLACE, self.win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        self.conn.change_property8(PropMode::REPLACE, self.win, self.atoms.net_wm_name, self.atoms.utf8_string, title.as_bytes())?;
        Ok(())
    }

    fn load_current(&mut self) -> Res<()> {
        self.media = if let Some(p) = self.files.get(self.idx) { LoadedMedia::load(p) } else { None };
        self.pan_x = 0;
        self.pan_y = 0;
        self.update_title()?;
        self.paint()
    }

    fn next_image(&mut self) -> Res<()> {
        if self.files.is_empty() {
            return Ok(());
        }
        self.idx = (self.idx + 1) % self.files.len();
        self.load_current()
    }

    fn prev_image(&mut self) -> Res<()> {
        if self.files.is_empty() {
            return Ok(());
        }
        self.idx = if self.idx == 0 { self.files.len() - 1 } else { self.idx - 1 };
        self.load_current()
    }

    /// Delete (the key or the button): asks first. Only Yes (or Y) goes
    /// on; Enter, Esc, N and Delete again all leave the picture alone, so
    /// it can't go by a slip of the finger.
    fn ask_delete(&mut self) -> Res<()> {
        if let Some(p) = self.files.get(self.idx) {
            self.confirm_delete = Some(p.clone());
            self.paint()?;
        }
        Ok(())
    }

    fn answer_delete(&mut self, yes: bool) -> Res<()> {
        let Some(path) = self.confirm_delete.take() else { return Ok(()) };
        if yes {
            match ferro_associations::trash::put(&path) {
                Ok(_) => {
                    if let Some(i) = self.files.iter().position(|f| *f == path) {
                        self.files.remove(i);
                    }
                    if self.idx >= self.files.len() && !self.files.is_empty() {
                        self.idx = self.files.len() - 1;
                    }
                    return self.load_current();
                }
                Err(e) => self.notice = Some(format!("Couldn't move it to the Recycle Bin: {e}")),
            }
        }
        self.paint()
    }

    /// The question (or a notice) over the picture, with Yes and No.
    fn draw_dialog(&self, s: &mut Surface) {
        let (lines, buttons): (Vec<String>, bool) = if let Some(p) = &self.confirm_delete {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            (vec![format!("Move '{}' to the Recycle Bin?", ellipsize(&name, 34)), "Press Y for Yes, or N to keep it.".into()], true)
        } else if let Some(n) = &self.notice {
            (vec![ellipsize(n, 46)], false)
        } else {
            return;
        };
        let r = dialog_rect(self.w, self.h);
        s.fill(r, color::FACE);
        s.bevel(r, Bevel::Window);
        let title = Rect::new(r.x + 3, r.y + 3, r.w - 6, 18);
        s.fill(title, color::NAVY);
        s.text_bold(title.x + 6, title.y + 3, if buttons { "Confirm Delete" } else { "Image Viewer" }, color::WHITE);
        for (i, l) in lines.iter().enumerate() {
            s.text(r.x + 16, r.y + 34 + i as i32 * (GLYPH_H + 6), l, color::BLACK);
        }
        let (yes, no) = dialog_buttons(r);
        if buttons {
            button(s, yes, "Yes", true);
        }
        button(s, no, if buttons { "No" } else { "OK" }, true);
    }

    fn viewport_rect(&self) -> Rect {
        Rect::new(4, TOOLBAR_H + 2, (self.w - 8).max(10), (self.h - TOOLBAR_H - STATUS_H - 4).max(10))
    }

    fn compute_scale_and_pos(&self, vp: Rect, img_w: usize, img_h: usize) -> (f32, i32, i32, i32, i32) {
        let iw = img_w as f32;
        let ih = img_h as f32;
        let vw = vp.w as f32;
        let vh = vp.h as f32;

        let scale = match self.zoom {
            ZoomMode::Fit => (vw / iw).min(vh / ih).min(10.0),
            ZoomMode::Scale(s) => s.clamp(0.05, 30.0),
        };

        let draw_w = (iw * scale).round() as i32;
        let draw_h = (ih * scale).round() as i32;

        let center_x = vp.x + (vp.w - draw_w) / 2 + self.pan_x;
        let center_y = vp.y + (vp.h - draw_h) / 2 + self.pan_y;

        (scale, center_x, center_y, draw_w, draw_h)
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

        for y in start_y..end_y {
            let src_y = ((y - dy) as usize * ih / dh as usize).min(ih - 1);
            let row_offset = src_y * iw;
            for x in start_x..end_x {
                let src_x = ((x - dx) as usize * iw / dw as usize).min(iw - 1);
                let pixel = img.pixels[row_offset + src_x];
                s.put(x, y, pixel);
            }
        }
    }

    fn paint(&mut self) -> Res<()> {
        let mut s = Surface::new(self.w as usize, self.h as usize);
        s.fill(Rect::new(0, 0, self.w, self.h), color::FACE);

        // Toolbar Buttons
        button(&mut s, PREV_BTN, "< Prev", self.idx > 0 || self.files.len() > 1);
        button(&mut s, NEXT_BTN, "Next >", self.idx + 1 < self.files.len() || self.files.len() > 1);
        button(&mut s, ZOOM_OUT_BTN, "-", self.media.is_some());
        button(&mut s, ZOOM_IN_BTN, "+", self.media.is_some());
        button(&mut s, ACTUAL_BTN, "1:1", self.media.is_some());
        button(&mut s, FIT_BTN, "Fit", self.media.is_some());
        button(&mut s, SLIDESHOW_BTN, if self.slideshow { "Stop" } else { "Slideshow" }, self.files.len() > 1);
        button(&mut s, DELETE_BTN, "Delete", !self.files.is_empty());

        // Viewport Frame
        let vp = self.viewport_rect();
        s.fill(vp, 0x00202020);
        s.bevel(vp, Bevel::Sunken);

        let mut zoom_str = "Fit".to_string();
        if let Some(ref m) = self.media {
            if let Some(frame) = m.current_frame() {
                let (scale, dx, dy, dw, dh) = self.compute_scale_and_pos(vp, frame.width, frame.height);
                zoom_str = match self.zoom {
                    ZoomMode::Fit => format!("Fit ({:.0}%)", scale * 100.0),
                    ZoomMode::Scale(s) => format!("{:.0}%", s * 100.0),
                };
                Self::draw_image_scaled(&mut s, vp.inset(1), frame, dx, dy, dw, dh);
            }
        } else {
            let msg = if self.files.is_empty() { "No images in folder" } else { "Failed to load image" };
            let tx = vp.x + (vp.w - text_width(msg)) / 2;
            let ty = vp.y + (vp.h - GLYPH_H) / 2;
            s.text(tx, ty, msg, color::SHADOW);
        }

        // Status Bar (Sunken 3D Panels)
        let sb_y = self.h - STATUS_H + 2;
        let sb_h = STATUS_H - 4;

        let pan1 = Rect::new(4, sb_y, (self.w * 35 / 100).max(120), sb_h);
        let pan2 = Rect::new(pan1.right() + 4, sb_y, 160, sb_h);
        let pan3 = Rect::new(pan2.right() + 4, sb_y, 90, sb_h);
        let pan4 = Rect::new(pan3.right() + 4, sb_y, 110, sb_h);
        let pan5 = Rect::new(pan4.right() + 4, sb_y, (self.w - pan4.right() - 8).max(80), sb_h);

        for p in [pan1, pan2, pan3, pan4, pan5] {
            s.bevel(p, Bevel::Field);
        }

        if let Some(p) = self.files.get(self.idx) {
            let fname = p.file_name().unwrap_or_default().to_string_lossy();
            let room1 = ((pan1.w - 10) / GLYPH_W).max(1) as usize;
            s.text(pan1.x + 5, pan1.y + (pan1.h - GLYPH_H) / 2, &ellipsize(&fname, room1), color::BLACK);

            if let Some(ref m) = self.media {
                let dim_str = format!("{}x{} px  {}", m.dim.0, m.dim.1, m.format_name);
                s.text(pan2.x + 5, pan2.y + (pan2.h - GLYPH_H) / 2, &dim_str, color::BLACK);

                let sz_str = if m.file_size_bytes < 1024 {
                    format!("{} B", m.file_size_bytes)
                } else if m.file_size_bytes < 1024 * 1024 {
                    format!("{:.1} KB", m.file_size_bytes as f64 / 1024.0)
                } else {
                    format!("{:.1} MB", m.file_size_bytes as f64 / (1024.0 * 1024.0))
                };
                s.text(pan3.x + 5, pan3.y + (pan3.h - GLYPH_H) / 2, &sz_str, color::BLACK);
            }

            s.text(pan4.x + 5, pan4.y + (pan4.h - GLYPH_H) / 2, &zoom_str, color::BLACK);

            let idx_str = format!("{} of {}", self.idx + 1, self.files.len());
            s.text(pan5.x + 5, pan5.y + (pan5.h - GLYPH_H) / 2, &idx_str, color::BLACK);
        }

        self.draw_dialog(&mut s);
        ferro_ui::show(&self.conn, self.win, self.gc, self.depth, &s, self.w, self.h)
    }

    fn click(&mut self, e: &ButtonPressEvent) -> Res<()> {
        let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
        let now = Instant::now();
        if self.confirm_delete.is_some() || self.notice.is_some() {
            let (yes, no) = dialog_buttons(dialog_rect(self.w, self.h));
            if self.confirm_delete.is_some() && yes.contains(x, y) {
                return self.answer_delete(true);
            }
            if no.contains(x, y) {
                self.notice = None;
                return self.answer_delete(false);
            }
            return Ok(());
        }

        if PREV_BTN.contains(x, y) {
            return self.prev_image();
        }
        if NEXT_BTN.contains(x, y) {
            return self.next_image();
        }
        if ZOOM_OUT_BTN.contains(x, y) {
            let cur = match self.zoom {
                ZoomMode::Fit => 1.0,
                ZoomMode::Scale(s) => s,
            };
            self.zoom = ZoomMode::Scale((cur * 0.75).max(0.1));
            return self.paint();
        }
        if ZOOM_IN_BTN.contains(x, y) {
            let cur = match self.zoom {
                ZoomMode::Fit => 1.0,
                ZoomMode::Scale(s) => s,
            };
            self.zoom = ZoomMode::Scale((cur * 1.33).min(20.0));
            return self.paint();
        }
        if ACTUAL_BTN.contains(x, y) {
            self.zoom = ZoomMode::Scale(1.0);
            self.pan_x = 0;
            self.pan_y = 0;
            return self.paint();
        }
        if FIT_BTN.contains(x, y) {
            self.zoom = ZoomMode::Fit;
            self.pan_x = 0;
            self.pan_y = 0;
            return self.paint();
        }
        if SLIDESHOW_BTN.contains(x, y) {
            self.slideshow = !self.slideshow;
            self.last_slide_time = now;
            return self.paint();
        }
        if DELETE_BTN.contains(x, y) {
            return self.ask_delete();
        }

        let vp = self.viewport_rect();
        if vp.contains(x, y) {
            if e.detail == 1 {
                // Check double click -> toggle fit / 100%
                let is_double =
                    now.duration_since(self.last_click.0).as_millis() < 400 && (x - self.last_click.1).abs() < 5 && (y - self.last_click.2).abs() < 5;

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

                self.dragging = true;
                self.drag_start = (x, y);
                self.pan_start = (self.pan_x, self.pan_y);
            } else if e.detail == 4 {
                // Wheel Up -> Zoom in
                let cur = match self.zoom {
                    ZoomMode::Fit => 1.0,
                    ZoomMode::Scale(s) => s,
                };
                self.zoom = ZoomMode::Scale((cur * 1.2).min(25.0));
                return self.paint();
            } else if e.detail == 5 {
                // Wheel Down -> Zoom out
                let cur = match self.zoom {
                    ZoomMode::Fit => 1.0,
                    ZoomMode::Scale(s) => s,
                };
                self.zoom = ZoomMode::Scale((cur * 0.8).max(0.1));
                return self.paint();
            }
        }

        Ok(())
    }

    fn motion(&mut self, e: &MotionNotifyEvent) -> Res<()> {
        if self.dragging {
            let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
            let dx = x - self.drag_start.0;
            let dy = y - self.drag_start.1;
            self.pan_x = self.pan_start.0 + dx;
            self.pan_y = self.pan_start.1 + dy;
            self.paint()?;
        }
        Ok(())
    }

    fn release(&mut self) -> Res<()> {
        self.dragging = false;
        Ok(())
    }

    fn key(&mut self, e: &KeyPressEvent) -> Res<()> {
        if self.confirm_delete.is_some() {
            return self.answer_delete(e.detail == KEY_Y);
        }
        if self.notice.take().is_some() {
            return self.paint();
        }
        match e.detail {
            KEY_ESCAPE | KEY_Q => std::process::exit(0),
            KEY_LEFT | KEY_BACKSPACE => self.prev_image()?,
            KEY_RIGHT | KEY_SPACE | KEY_RETURN => self.next_image()?,
            KEY_PLUS | KEY_KP_ADD => {
                let cur = match self.zoom {
                    ZoomMode::Fit => 1.0,
                    ZoomMode::Scale(s) => s,
                };
                self.zoom = ZoomMode::Scale((cur * 1.25).min(25.0));
                self.paint()?;
            }
            KEY_MINUS | KEY_KP_SUB => {
                let cur = match self.zoom {
                    ZoomMode::Fit => 1.0,
                    ZoomMode::Scale(s) => s,
                };
                self.zoom = ZoomMode::Scale((cur * 0.8).max(0.1));
                self.paint()?;
            }
            KEY_F => {
                self.zoom = ZoomMode::Fit;
                self.pan_x = 0;
                self.pan_y = 0;
                self.paint()?;
            }
            KEY_1 => {
                self.zoom = ZoomMode::Scale(1.0);
                self.pan_x = 0;
                self.pan_y = 0;
                self.paint()?;
            }
            KEY_S => {
                self.slideshow = !self.slideshow;
                self.last_slide_time = Instant::now();
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

    fn tick(&mut self) -> Res<()> {
        let now = Instant::now();
        let mut need_paint = false;

        if let Some(ref mut m) = self.media {
            if m.advance_animation(now) {
                need_paint = true;
            }
        }

        if self.slideshow && now.duration_since(self.last_slide_time) >= Duration::from_millis(3200) {
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
                Event::ButtonRelease(_) => app.release()?,
                Event::MotionNotify(e) => app.motion(&e)?,
                Event::KeyPress(e) => app.key(&e)?,
                Event::ClientMessage(e) if e.data.as_data32()[0] == app.atoms.wm_delete => {
                    return Ok(());
                }
                _ => {}
            }
        }

        app.tick()?;
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
