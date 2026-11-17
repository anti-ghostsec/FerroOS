//! The USB Kill page (ferro-settings --usbkill): whether the computer powers
//! off when its USB ports change, which changes do it, and which devices
//! may come and go. The rules are the system's (/etc/ferro/usbkill.conf), so
//! saving them asks for the password; the service (ferro-usbkill watch)
//! picks them up at once.

use ferro_gfx::{color, ellipsize, wrap, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{Key, Keymap, Res};
use ferro_usbkill::{Action, Config, Device, Rule, When};
use std::path::Path;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 620;
const H: i32 = 492;
const ROWS: usize = 7;
const ROW_H: i32 = 20;
const LIST_Y: i32 = 222;
const TOOL: &str = "/usr/local/sbin/ferro-usbkill";

/// A device in the list: one plugged in now, or one on the lists that isn't.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub rule: Rule,
    pub present: bool,
    pub allowed: bool,
    pub tether: bool,
}

/// The list: what's plugged in (each with the rule that covers it, or one
/// for exactly it), then what the rules name that isn't plugged in.
pub fn entries(c: &Config, now: &[Device]) -> Vec<Entry> {
    let mut list: Vec<Entry> = Vec::new();
    for d in now {
        let tether = c.tether.iter().find(|r| r.matches(d));
        let allow = c.allow.iter().find(|r| r.matches(d));
        let rule = tether.or(allow).cloned().unwrap_or_else(|| Rule::for_device(d));
        if list.iter().any(|e| e.rule == rule) {
            continue;
        }
        list.push(Entry { name: d.name.clone(), rule, present: true, allowed: allow.is_some() || tether.is_some(), tether: tether.is_some() });
    }
    for (r, tether) in c.tether.iter().map(|r| (r, true)).chain(c.allow.iter().map(|r| (r, false))) {
        if !list.iter().any(|e| e.rule == *r) {
            list.push(Entry { name: "Not plugged in".into(), rule: r.clone(), present: false, allowed: true, tether });
        }
    }
    list
}

/// The lists, back from the ticks.
pub fn lists(c: &mut Config, list: &[Entry]) {
    c.tether = list.iter().filter(|e| e.tether).map(|e| e.rule.clone()).collect();
    c.allow = list.iter().filter(|e| e.allowed && !e.tether).map(|e| e.rule.clone()).collect();
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Ctl {
    Enable,
    When(When),
    OnNew,
    OnRemoved,
    Act(Action),
    Sync,
    Allow(usize),
    Tether(usize),
    Edit,
    Ok,
    Cancel,
}

fn row(i: usize) -> Rect {
    Rect::new(20, LIST_Y + i as i32 * ROW_H, W - 40, ROW_H)
}

fn controls(n: usize) -> Vec<(Rect, Ctl)> {
    let mut v = vec![
        (Rect::new(24, 60, 300, 18), Ctl::Enable),
        (Rect::new(100, 82, 120, 18), Ctl::When(When::Always)),
        (Rect::new(230, 82, 300, 18), Ctl::When(When::Locked)),
        (Rect::new(24, 104, 500, 18), Ctl::OnNew),
        (Rect::new(24, 126, 500, 18), Ctl::OnRemoved),
        (Rect::new(100, 148, 100, 18), Ctl::Act(Action::PowerOff)),
        (Rect::new(210, 148, 90, 18), Ctl::Act(Action::Restart)),
        (Rect::new(310, 148, 260, 18), Ctl::Act(Action::Note)),
        (Rect::new(24, 170, 500, 18), Ctl::Sync),
    ];
    for i in 0..n.min(ROWS) {
        let r = row(i);
        v.push((Rect::new(r.right() - 160, r.y + 1, 80, 18), Ctl::Allow(i)));
        v.push((Rect::new(r.right() - 76, r.y + 1, 72, 18), Ctl::Tether(i)));
    }
    v.push((Rect::new(W - 160, 398, 140, 24), Ctl::Edit));
    v.push((Rect::new(W - 210, H - 36, 90, 26), Ctl::Ok));
    v.push((Rect::new(W - 110, H - 36, 90, 26), Ctl::Cancel));
    v
}

fn checkbox(s: &mut Surface, r: Rect, label: &str, on: bool, enabled: bool) {
    let b = Rect::new(r.x, r.y + (r.h - 13) / 2, 13, 13);
    s.fill(b, if enabled { color::WHITE } else { color::FACE });
    s.bevel(b, Bevel::Sunken);
    if on {
        s.text(b.x + 3, b.y + 3, "x", if enabled { color::BLACK } else { color::SHADOW });
    }
    let y = r.y + (r.h - GLYPH_H) / 2;
    if enabled {
        s.text(r.x + 20, y, label, color::BLACK);
    } else {
        s.text_disabled(r.x + 20, y, label);
    }
}

fn radio(s: &mut Surface, r: Rect, label: &str, on: bool) {
    let well = Rect::new(r.x, r.y + 3, 12, 12);
    s.fill(well, color::WHITE);
    s.bevel(well, Bevel::Sunken);
    if on {
        s.fill(Rect::new(well.x + 4, well.y + 4, 4, 4), color::BLACK);
    }
    s.text(r.x + 18, r.y + (r.h - GLYPH_H) / 2, label, color::BLACK);
}

struct Page {
    c: Config,
    list: Vec<Entry>,
    /// The file's lines that made no sense, as the service left them out.
    bad: usize,
    last: String,
    said: String,
    changed: bool,
    /// The rules as they were read: while those are on and watching,
    /// saving sets it off, so Save asks to be clicked again.
    was: Config,
    sure: bool,
}

fn read_page() -> Page {
    let (c, bad) = Config::parse(&std::fs::read_to_string(ferro_usbkill::CONFIG).unwrap_or_default());
    let now = ferro_usbkill::scan(Path::new(ferro_usbkill::BUSES));
    let last = std::fs::read_to_string(ferro_usbkill::LAST).unwrap_or_default().trim().to_owned();
    Page { list: entries(&c, &now), was: c.clone(), c, bad: bad.len(), last, said: String::new(), changed: false, sure: false }
}

fn draw(s: &mut Surface, p: &Page) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    let (head, c) = if p.c.enabled { ("USB Kill is on", 0x008000) } else { ("USB Kill is off", color::RED) };
    s.text_bold(14, 12, head, c);
    s.text(14, 26, "Powers off when USB, Thunderbolt or FireWire devices change.", color::BLACK);
    let ctl = controls(p.list.len());
    let at = |c: Ctl| ctl.iter().find(|(_, k)| *k == c).map(|(r, _)| *r).unwrap_or_default();
    s.group_box(Rect::new(10, 44, W - 20, 152), "Rules");
    checkbox(s, at(Ctl::Enable), "Turn on USB Kill", p.c.enabled, true);
    s.text(24, 82 + 5, "Watch:", color::BLACK);
    radio(s, at(Ctl::When(When::Always)), "Always", p.c.when == When::Always);
    radio(s, at(Ctl::When(When::Locked)), "Only while the screen is locked", p.c.when == When::Locked);
    checkbox(s, at(Ctl::OnNew), "Plugging in a device that isn't allowed sets it off", p.c.on_new, true);
    checkbox(s, at(Ctl::OnRemoved), "Taking out a device that isn't allowed sets it off", p.c.on_removed, true);
    s.text(24, 148 + 5, "Then:", color::BLACK);
    radio(s, at(Ctl::Act(Action::PowerOff)), "Power off", p.c.action == Action::PowerOff);
    radio(s, at(Ctl::Act(Action::Restart)), "Restart", p.c.action == Action::Restart);
    radio(s, at(Ctl::Act(Action::Note)), "Only note it (to try it out)", p.c.action == Action::Note);
    checkbox(s, at(Ctl::Sync), "Write everything to the disks first (slower)", p.c.sync, true);

    s.group_box(Rect::new(10, 204, W - 20, 186), "Devices");
    let list = Rect::new(20, LIST_Y, W - 40, ROWS as i32 * ROW_H);
    s.fill(list, color::WHITE);
    s.bevel(list, Bevel::Sunken);
    if p.list.is_empty() {
        s.text(28, LIST_Y + 6, "No devices are plugged in.", color::SHADOW);
    }
    for (i, e) in p.list.iter().take(ROWS).enumerate() {
        let r = row(i);
        let label = format!("{}  {}", e.name, e.rule);
        let room = ((r.w - 172) / GLYPH_W) as usize;
        let y = r.y + (r.h - GLYPH_H) / 2;
        if e.present {
            s.text(r.x + 6, y, &ellipsize(&label, room), color::BLACK);
        } else {
            s.text(r.x + 6, y, &ellipsize(&label, room), color::SHADOW);
        }
        checkbox(s, at(Ctl::Allow(i)), "Allowed", e.allowed || e.tether, !e.tether);
        checkbox(s, at(Ctl::Tether(i)), "Tether", e.tether, true);
    }
    let more = p.list.len().saturating_sub(ROWS);
    let note =
        if more > 0 { format!("And {more} more: ferro-usbkill devices lists them all.") } else { "Tether: taking it out always sets it off.".into() };
    s.text(20, LIST_Y + ROWS as i32 * ROW_H + 6, &note, color::SHADOW);

    let extra = format!("Commands to run first: {}. Files to wipe: {}.", p.c.run.len(), p.c.wipe.len());
    s.text(14, 400, &extra, color::BLACK);
    s.text(14, 412, "They're set in the rules file.", color::BLACK);
    ferro_ui::button(s, at(Ctl::Edit), "Edit rules file...", true);
    let mut y = 432;
    if p.bad > 0 {
        s.text(14, y, &format!("{} lines of the rules file make no sense; they're left out.", p.bad), color::RED);
        y += 12;
    }
    if !p.last.is_empty() {
        for l in wrap(&format!("Last set off: {}", p.last), ((W - 28) / GLYPH_W) as usize).iter().take(2) {
            s.text(14, y, l, color::BLACK);
            y += 12;
        }
    }
    if !p.said.is_empty() {
        s.text(14, H - 52, &ellipsize(&p.said, ((W - 28) / GLYPH_W) as usize), color::RED);
    }
    ferro_ui::default_button(s, at(Ctl::Ok), "Save", p.changed);
    ferro_ui::button(s, at(Ctl::Cancel), "Close", true);
}

/// Saves the rules through ferro-usbkill (asks for the password). None: done.
fn save(p: &Page) -> Option<String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    let file = dir.join(format!("ferro-usbkill-{}.conf", std::process::id()));
    if std::fs::write(&file, p.c.format()).is_err() {
        return Some("The rules couldn't be written down to save.".into());
    }
    let ok = ferro_ui::admin::run(TOOL, &["apply", &file.to_string_lossy()], Some("Changing USB Kill"));
    let _ = std::fs::remove_file(&file);
    (!ok).then(|| "Not saved: the password wasn't given, or the rules weren't taken.".into())
}

/// Saving while the rules are on and watching (this window is only seen
/// unlocked, so: watching always) is a change to them, which sets it off
/// as any other would: said first, then a second Save goes ahead.
fn needs_sure(p: &Page) -> Option<String> {
    let on = p.was.enabled && p.was.when == When::Always && p.was.action != Action::Note;
    let then = if p.was.action == Action::Restart { "restarts" } else { "powers off" };
    (on && !p.sure).then(|| format!("It's on: saving is a change, so the computer {then}. Save again to go ahead."))
}

/// A tick or a choice; true if anything changed.
fn click(p: &mut Page, c: Ctl) -> bool {
    match c {
        Ctl::Enable => {
            p.c.enabled = !p.c.enabled;
            // Turned on with nothing allowed: what's plugged in now is, so
            // it doesn't power off the moment the mouse is moved to another port.
            if p.c.enabled && !p.list.iter().any(|e| e.allowed || e.tether) {
                for e in p.list.iter_mut().filter(|e| e.present) {
                    e.allowed = true;
                }
            }
        }
        Ctl::When(w) => p.c.when = w,
        Ctl::OnNew => p.c.on_new = !p.c.on_new,
        Ctl::OnRemoved => p.c.on_removed = !p.c.on_removed,
        Ctl::Act(a) => p.c.action = a,
        Ctl::Sync => p.c.sync = !p.c.sync,
        Ctl::Allow(i) => {
            let Some(e) = p.list.get_mut(i).filter(|e| !e.tether) else { return false };
            e.allowed = !e.allowed;
        }
        Ctl::Tether(i) => {
            let Some(e) = p.list.get_mut(i) else { return false };
            e.tether = !e.tether;
        }
        Ctl::Edit | Ctl::Ok | Ctl::Cancel => return false,
    }
    let list = p.list.clone();
    lists(&mut p.c, &list);
    true
}

pub fn run() -> Res<()> {
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let mut page = read_page();
    let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let mask = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::FOCUS_CHANGE;
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        ((sw - W) / 2) as i16,
        ((sh - H) / 2) as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().background_pixel(color::FACE).event_mask(mask),
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let title = "USB Kill";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-settings\0ferro-settings\0")?;
    ferro_ui::fixed_size(&conn, win, W, H)?;
    conn.map_window(win)?;
    conn.flush()?;
    let keymap = Keymap::load(&conn)?;
    let paint = |p: &Page| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, p);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };
    let save_now = |page: &mut Page| {
        if let Some(warning) = needs_sure(page) {
            page.said = warning;
            page.sure = true;
            return;
        }
        match save(page) {
            None => *page = read_page(),
            Some(why) => page.said = why,
        }
    };
    loop {
        match conn.wait_for_event()? {
            Event::Expose(e) if e.count == 0 => paint(&page)?,
            // Back from the rules file, or a device plugged in: as it is now
            // (unless there's something not yet saved).
            Event::FocusIn(_) if !page.changed => {
                page = read_page();
                paint(&page)?;
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == wm_delete => return Ok(()),
            Event::ButtonPress(b) if b.detail == 1 => {
                let (x, y) = (i32::from(b.event_x), i32::from(b.event_y));
                let Some(c) = controls(page.list.len()).into_iter().find(|(r, _)| r.contains(x, y)).map(|(_, c)| c) else { continue };
                match c {
                    Ctl::Cancel => return Ok(()),
                    Ctl::Ok if page.changed => save_now(&mut page),
                    Ctl::Edit => super::launch(&format!("ferro-terminal -e sudoedit {}", ferro_usbkill::CONFIG)),
                    c => {
                        if click(&mut page, c) {
                            page.changed = true;
                            page.said.clear();
                            page.sure = false;
                        }
                    }
                }
                paint(&page)?;
            }
            Event::KeyPress(k) => match keymap.typed(&k) {
                Key::Escape => return Ok(()),
                Key::Enter if page.changed => {
                    save_now(&mut page);
                    paint(&page)?;
                }
                _ => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(port: &str, id: &str, serial: Option<&str>) -> Device {
        Device { port: port.into(), devnum: 2, id: id.into(), serial: serial.map(Into::into), name: format!("Dev {port}") }
    }

    #[test]
    fn the_list_is_whats_plugged_in_and_whats_named() {
        let (c, _) = Config::parse("allow = 1c4f:0002\ntether = 0781:5583 serial=S1\nallow = dead:beef\n");
        let now = [dev("3-5", "1c4f:0002", None), dev("3-6", "1c4f:0002", None), dev("3-7", "0b05:19af", Some("9"))];
        let l = entries(&c, &now);
        let got: Vec<(String, bool, bool, bool)> = l.iter().map(|e| (e.rule.to_string(), e.present, e.allowed, e.tether)).collect();
        assert_eq!(
            got,
            [
                ("1c4f:0002".into(), true, true, false),
                ("0b05:19af serial=9".into(), true, false, false),
                ("0781:5583 serial=S1".into(), false, true, true),
                ("dead:beef".into(), false, true, false),
            ]
        );
        let mut c2 = c.clone();
        lists(&mut c2, &l);
        assert_eq!((c2.allow.len(), c2.tether.len()), (2, 1));
    }

    #[test]
    fn turning_it_on_allows_whats_plugged_in() {
        let now = [dev("3-5", "1c4f:0002", None)];
        let mut p = Page {
            c: Config::default(),
            list: entries(&Config::default(), &now),
            bad: 0,
            last: String::new(),
            said: String::new(),
            changed: false,
            was: Config::default(),
            sure: false,
        };
        assert!(click(&mut p, Ctl::Enable));
        assert!(p.c.enabled);
        assert_eq!(p.c.allow, [Rule::parse("1c4f:0002").unwrap()]);
        // A tether is allowed too, and Allowed can't be unticked under it.
        click(&mut p, Ctl::Tether(0));
        assert!(!click(&mut p, Ctl::Allow(0)));
        assert_eq!((p.c.allow.len(), p.c.tether.len()), (0, 1));
    }

    #[test]
    fn saving_while_on_asks_to_be_sure() {
        let (on, _) = Config::parse("enabled = true\n");
        let mut p = Page { c: on.clone(), list: Vec::new(), bad: 0, last: String::new(), said: String::new(), changed: true, was: on, sure: false };
        assert!(needs_sure(&p).is_some_and(|w| w.contains("powers off")));
        p.sure = true;
        assert!(needs_sure(&p).is_none());
        p.sure = false;
        p.was.action = Action::Note;
        assert!(needs_sure(&p).is_none(), "only noting: saved at once");
        p.was = Config::default();
        assert!(needs_sure(&p).is_none(), "off: saved at once");
    }

    #[test]
    fn controls_dont_overlap() {
        let c = controls(ROWS);
        for (i, (a, _)) in c.iter().enumerate() {
            for (b, _) in &c[i + 1..] {
                assert!(!a.overlaps(b), "{a:?} {b:?}");
            }
            assert!(a.right() <= W && a.y + a.h <= H);
        }
    }
}
