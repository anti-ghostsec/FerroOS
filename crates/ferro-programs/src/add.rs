//! Add Programs: what FerroOS Heavy can add (drivers, Steam, Wine...), each
//! with a line on what it is. Start opens the progress window for it, which
//! runs `ferro-heavy` and asks for the password there.

use crate::proto;
use crate::win::{self, Win};
use ferro_gfx::{color, ellipsize, icons, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, default_button, Edit, Key, Res, TextEdit};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::Event;

const HEAVY: &str = "/usr/local/bin/ferro-heavy";
const W: i32 = 600;
const H: i32 = 548;
const ROW_H: i32 = 32;
const LIST: Rect = Rect::new(16, 56, W - 32, 12 * ROW_H + 6);
const BAR_Y: i32 = LIST.y + LIST.h + 10;
const START_BTN: Rect = Rect::new(W - 16 - 90, BAR_Y, 90, 24);
const FIELD: Rect = Rect::new(150, BAR_Y + 1, W - 150 - 16 - 2 * 83, 22);
const OK_BTN: Rect = Rect::new(W - 16 - 75 - 83, BAR_Y, 75, 24);
const CANCEL_BTN: Rect = Rect::new(W - 16 - 75, BAR_Y, 75, 24);
const CLOSE_BTN: Rect = Rect::new(W - 91, H - 36, 75, 24);

pub struct Item {
    pub name: &'static str,
    pub about: &'static str,
    /// ferro-heavy's word for it (None: a package typed by name).
    pub verb: Option<&'static str>,
    /// What the progress window is called.
    pub doing: &'static str,
    pub root: bool,
}

pub const ITEMS: [Item; 12] = [
    Item {
        name: "Update everything",
        about: "Debian's packages and FerroOS's own programs, brought up to date.",
        verb: Some("update"),
        doing: "Updating FerroOS",
        root: true,
    },
    Item {
        name: "NVIDIA graphics driver",
        about: "NVIDIA's driver, with CUDA support for PyTorch and the like.",
        verb: Some("nvidia"),
        doing: "Installing the NVIDIA driver",
        root: true,
    },
    Item {
        name: "3D graphics and Vulkan",
        about: "Mesa 3D, Vulkan and video decoding for Intel and AMD graphics.",
        verb: Some("drivers"),
        doing: "Installing 3D graphics",
        root: true,
    },
    Item {
        name: "Steam",
        about: "Valve's Steam, and Proton's Vulkan drivers for Windows games.",
        verb: Some("steam"),
        doing: "Installing Steam",
        root: true,
    },
    Item {
        name: "LLM / AI training tools",
        about: "Python 3, venv and build tools for PyTorch and LLM training.",
        verb: Some("llm"),
        doing: "Installing LLM tools",
        root: true,
    },
    Item {
        name: "NVIDIA CUDA toolkit",
        about: "Compilers and headers, several GB. PyTorch brings its own.",
        verb: Some("cuda"),
        doing: "Installing the CUDA toolkit",
        root: true,
    },
    Item {
        name: "Sound test",
        about: "Unmutes the output chosen in Volume Control and plays a sound.",
        verb: Some("audio"),
        doing: "Sound test",
        root: false,
    },
    Item {
        name: "ScummVM",
        about: "Classic adventure and children's games (LucasArts, Sierra...).",
        verb: Some("scummvm"),
        doing: "Installing ScummVM",
        root: true,
    },
    Item { name: "Wine", about: "Runs Windows programs, and older 32-bit games.", verb: Some("wine"), doing: "Installing Wine", root: true },
    Item { name: "Tor Browser", about: "The safest way to browse over Tor.", verb: Some("torbrowser"), doing: "Installing Tor Browser", root: true },
    Item {
        name: "A Debian package by name",
        about: "Any of Debian's programs, if you know its package's name.",
        verb: None,
        doing: "Installing",
        root: true,
    },
    Item {
        name: "What's on and off",
        about: "Secure Boot, encryption, the firewall, graphics, sound.",
        verb: Some("status"),
        doing: "What's on and off",
        root: false,
    },
];

/// The progress window for `ferro-heavy VERB [ARGS]`, in a process of its
/// own (so several can run, and this window stays free).
fn open(title: &str, root: bool, verb: &str, extra: &[&str]) {
    let me = std::env::current_exe().unwrap_or_else(|_| "ferro-programs".into());
    let mut c = std::process::Command::new(me);
    c.args(["--run", title]);
    if root {
        c.arg("--root");
    }
    c.args(["--", HEAVY, "--gui", verb]).args(extra).stdin(std::process::Stdio::null());
    let _ = c.spawn().map(|mut child| std::thread::spawn(move || child.wait()));
}

fn row(i: usize) -> Rect {
    Rect::new(LIST.x + 3, LIST.y + 3 + i as i32 * ROW_H, LIST.w - 6, ROW_H)
}

fn draw(s: &mut Surface, chosen: usize, typing: Option<&TextEdit>, status: &str) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    s.sprite(16, 12, icons::FOLDER, 2);
    s.text(60, 14, "Choose what to add to this computer, then Start. Everything", color::BLACK);
    s.text(60, 28, "comes from Debian, its signatures checked, and is kept updated.", color::BLACK);
    win::list_box(s, LIST);
    let cols = ((LIST.w - 20) / GLYPH_W) as usize;
    for (i, item) in ITEMS.iter().enumerate() {
        let r = row(i);
        let (ink, sub) = if i == chosen {
            s.fill(r, color::NAVY);
            (color::WHITE, color::WHITE)
        } else {
            (color::BLACK, color::SHADOW)
        };
        s.text_bold(r.x + 8, r.y + 5, item.name, ink);
        s.text(r.x + 8, r.y + 7 + GLYPH_H + 3, &ellipsize(item.about, cols), sub);
    }
    if let Some(edit) = typing {
        s.text(16, BAR_Y + 8, "Package's name:", color::BLACK);
        edit.draw(s, FIELD);
        s.bevel(FIELD, ferro_gfx::Bevel::Sunken);
        default_button(s, OK_BTN, "Install", true);
        button(s, CANCEL_BTN, "Cancel", true);
    } else {
        s.text(16, BAR_Y + 8, &ellipsize(status, ((START_BTN.x - 24) / GLYPH_W) as usize), color::BLACK);
        default_button(s, START_BTN, "Start...", true);
    }
    button(s, CLOSE_BTN, "Close", true);
}

pub fn run() -> Res<()> {
    let w = Win::open("Add Programs", W, H)?;
    let mut s = Surface::new(W as usize, H as usize);
    let (mut chosen, mut typing, mut status) = (0usize, None::<TextEdit>, String::new());
    let mut last_click: Option<(usize, Instant)> = None;

    // The one chosen started (or, for a package by name, its box shown).
    macro_rules! go {
        () => {{
            let item = &ITEMS[chosen];
            match item.verb {
                Some(verb) => {
                    open(item.doing, item.root, verb, &[]);
                    status = format!("{} opened in a window of its own.", item.name);
                }
                None => typing = Some(TextEdit::new("")),
            }
        }};
    }
    macro_rules! install_typed {
        () => {{
            let name = typing.as_ref().map(|e| e.text().trim().to_owned()).unwrap_or_default();
            if proto::is_package_name(&name) {
                open(&format!("Installing {name}"), true, "install", &[&name]);
                status = format!("Installing {name} in a window of its own.");
                typing = None;
            } else {
                status = "That isn't a package's name (lower-case letters, digits, + - . only).".to_owned();
                typing = None;
            }
        }};
    }

    loop {
        let ev = w.conn.wait_for_event()?;
        match ev {
            Event::Expose(e) if e.count != 0 => continue,
            Event::Expose(_) => {}
            Event::MapNotify(_) => w.focus()?,
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == w.delete => return Ok(()),
            Event::ButtonPress(b) if b.detail == 1 => {
                let (x, y) = (i32::from(b.event_x), i32::from(b.event_y));
                if CLOSE_BTN.contains(x, y) {
                    return Ok(());
                }
                if typing.is_some() {
                    if FIELD.contains(x, y) {
                        if let Some(e) = typing.as_mut() {
                            e.click(FIELD, x, false);
                        }
                    } else if OK_BTN.contains(x, y) {
                        install_typed!();
                    } else if CANCEL_BTN.contains(x, y) {
                        typing = None;
                    }
                } else if let Some(i) = (0..ITEMS.len()).find(|&i| row(i).contains(x, y)) {
                    // A double click starts it, as Start does.
                    let double = last_click.is_some_and(|(j, t)| j == i && t.elapsed() < Duration::from_millis(500));
                    chosen = i;
                    last_click = Some((i, Instant::now()));
                    if double {
                        last_click = None;
                        go!();
                    }
                } else if START_BTN.contains(x, y) {
                    go!();
                }
            }
            Event::KeyPress(k) => {
                let key = w.keymap.typed(&k);
                let (ctrl, shift) = win::mods(k.state);
                if let Some(e) = typing.as_mut() {
                    match e.key(key, ctrl, shift) {
                        Edit::Done => install_typed!(),
                        Edit::Cancel => typing = None,
                        _ => {}
                    }
                } else {
                    match key {
                        Key::Up => chosen = chosen.saturating_sub(1),
                        Key::Down => chosen = (chosen + 1).min(ITEMS.len() - 1),
                        Key::Enter => go!(),
                        Key::Escape => return Ok(()),
                        _ => continue,
                    }
                }
            }
            _ => continue,
        }
        draw(&mut s, chosen, typing.as_ref(), &status);
        w.show(&s)?;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn shot(typing: bool) -> Surface<'static> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, 3, typing.then(|| TextEdit::new("vlc")).as_ref(), "Steam opened in a window of its own.");
        s
    }

    #[test]
    fn fits() {
        const { assert!(BAR_Y + 24 < CLOSE_BTN.y && FIELD.x + FIELD.w < OK_BTN.x) };
        assert!(row(ITEMS.len() - 1).bottom() <= LIST.bottom() - 3);
        for item in &ITEMS {
            assert!(item.about.len() as i32 * GLYPH_W <= LIST.w - 20, "{}", item.name);
        }
    }
}
