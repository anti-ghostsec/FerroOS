//! What the installer window shows and where its parts are, and the rules
//! its answers must pass. Pure geometry, pixels (ferro-gfx) and text, so
//! it's the same in tests as on screen.

use ferro_gfx::{color, icons, Bevel, Rect, Surface, GLYPH_H};
use ferro_ui::{button, text_field};

pub const W: i32 = 480;
pub const H: i32 = 320;
const MARGIN: i32 = 16;
const BUTTON_W: i32 = 80;
const BUTTON_H: i32 = 24;
const ROW_H: i32 = 20;
/// How many disks the list has room for.
const MAX_DISKS: usize = 6;

/// The boxes on the account page, in order.
pub const PASSPHRASE: usize = 0;
pub const PASSPHRASE_AGAIN: usize = 1;
pub const NAME: usize = 2;
pub const PASSWORD: usize = 3;
pub const PASSWORD_AGAIN: usize = 4;
pub const FIELDS: usize = 5;
const LABELS: [&str; FIELDS] = ["Passphrase for the disk:", "Passphrase again:", "Your account's name:", "Password:", "Password again:"];

/// The steps `ferro-install` announces, and how the window words them.
pub const STEPS: [(&str, &str); 5] = [
    ("== partitioning", "Preparing the disk"),
    ("== encrypting", "Encrypting it (LUKS2)"),
    ("== copying", "Copying the system"),
    ("== setting", "Setting it up to start from the disk"),
    ("== finishing", "Finishing"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Disk,
    Account,
    Confirm,
    Working,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    /// As the kernel names it: `sda`, `nvme0n1`.
    pub name: String,
    pub bytes: u64,
    pub model: String,
}

impl Disk {
    fn line(&self) -> String {
        format!("{:<9} {:>9}  {}", self.name, size_text(self.bytes), self.model)
    }
}

/// Everything the window shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub page: Page,
    pub disks: Vec<Disk>,
    pub disk: usize,
    pub fields: [String; FIELDS],
    /// Which box on the account page the typing goes into.
    pub focus: usize,
    /// The "yes, erase it" box is ticked.
    pub sure: bool,
    /// What's wrong with the answers, if anything.
    pub message: &'static str,
    /// The step being worked on.
    pub step: usize,
    /// Why installing stopped.
    pub error: String,
}

impl State {
    pub fn new(disks: Vec<Disk>) -> Self {
        Self { page: Page::Disk, disks, disk: 0, fields: Default::default(), focus: 0, sure: false, message: "", step: 0, error: String::new() }
    }

    /// What `ferro-install` asks, answered in its order. Sent to its
    /// standard input, so nothing secret is in a command line.
    pub fn answers(&self) -> String {
        let f = &self.fields;
        let disk = self.disks.get(self.disk).map_or("", |d| d.name.as_str());
        format!("{disk}\n{}\n{}\n{}\n{}\n{}\nERASE\n", f[PASSPHRASE], f[PASSPHRASE], f[NAME], f[PASSWORD], f[PASSWORD])
    }

    /// Forgets the passphrase and the password.
    pub fn forget(&mut self) {
        for i in [PASSPHRASE, PASSPHRASE_AGAIN, PASSWORD, PASSWORD_AGAIN] {
            self.fields[i].clear();
        }
    }
}

/// A size as people say it: `476.9 GB`.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["MB", "GB", "TB", "PB"];
    let mut v = bytes as f64 / (1u64 << 20) as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

/// Reads `lsblk -d -n -b -o NAME,SIZE,MODEL`. Leaves out the disk the
/// installation medium is (`medium` is its device, like `/dev/sdb1`) and
/// anything too small to install on.
pub fn parse_disks(lsblk: &str, medium: &str) -> Vec<Disk> {
    let medium = medium.trim().trim_start_matches("/dev/");
    lsblk
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?.to_owned();
            let bytes: u64 = parts.next()?.parse().ok()?;
            let model = parts.collect::<Vec<_>>().join(" ");
            let is_medium = !medium.is_empty() && medium.starts_with(&name);
            (!is_medium && bytes >= 8 << 30).then_some(Disk { name, bytes, model })
        })
        .collect()
}

/// What's wrong with the account page's answers, if anything. The same
/// rules `ferro-install` applies; here they can be said kindly.
pub fn check_account(f: &[String; FIELDS]) -> Result<(), &'static str> {
    if f[PASSPHRASE].chars().count() < 8 {
        return Err("The passphrase needs at least 8 characters.");
    }
    if f[PASSPHRASE] != f[PASSPHRASE_AGAIN] {
        return Err("The two passphrases differ.");
    }
    let name = f[NAME].as_bytes();
    let rest_ok = name.iter().skip(1).all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'));
    if !name.first().is_some_and(u8::is_ascii_lowercase) || !rest_ok || name.len() > 31 {
        return Err("The name: lower-case letters and digits, a letter first.");
    }
    if f[PASSWORD].is_empty() {
        return Err("The account needs a password.");
    }
    if f[PASSWORD] != f[PASSWORD_AGAIN] {
        return Err("The two passwords differ.");
    }
    Ok(())
}

/// Which step a line of `ferro-install`'s output starts, if it starts one.
pub fn step_of(line: &str) -> Option<usize> {
    STEPS.iter().position(|(marker, _)| line.contains(marker))
}

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Disk(usize),
    Field(usize),
    Sure,
    Back,
    Next,
    Cancel,
    Nothing,
}

/// The labels of the three buttons along the bottom on each page: back,
/// forward, and leave. `None` where a page has no such button.
fn buttons(page: Page) -> [Option<&'static str>; 3] {
    match page {
        Page::Disk => [None, Some("Next >"), Some("Cancel")],
        Page::Account => [Some("< Back"), Some("Next >"), Some("Cancel")],
        Page::Confirm => [Some("< Back"), Some("Install"), Some("Cancel")],
        Page::Working => [None, None, None],
        Page::Done => [None, Some("Restart"), Some("Close")],
        Page::Failed => [None, None, Some("Close")],
    }
}

fn button_rect(slot: i32) -> Rect {
    Rect::new(W - MARGIN - (3 - slot) * BUTTON_W - (2 - slot) * 6, H - MARGIN - BUTTON_H, BUTTON_W, BUTTON_H)
}

fn disk_list() -> Rect {
    Rect::new(MARGIN, 84, W - 2 * MARGIN, MAX_DISKS as i32 * ROW_H + 4)
}

fn disk_row(i: usize) -> Rect {
    let l = disk_list();
    Rect::new(l.x + 2, l.y + 2 + i as i32 * ROW_H, l.w - 4, ROW_H)
}

fn field(i: usize) -> Rect {
    Rect::new(220, 52 + i as i32 * 30, W - MARGIN - 220, 22)
}

fn sure_box() -> Rect {
    Rect::new(MARGIN, 150, 13, 13)
}

/// Whether going forward is possible yet.
pub fn can_go_on(s: &State) -> bool {
    match s.page {
        Page::Disk => s.disk < s.disks.len(),
        Page::Confirm => s.sure,
        _ => true,
    }
}

pub fn hit(s: &State, x: i32, y: i32) -> Hit {
    let labels = buttons(s.page);
    for (slot, what) in [Hit::Back, Hit::Next, Hit::Cancel].into_iter().enumerate() {
        if labels[slot].is_some() && button_rect(slot as i32).contains(x, y) {
            return what;
        }
    }
    match s.page {
        Page::Disk => (0..s.disks.len().min(MAX_DISKS)).find(|&i| disk_row(i).contains(x, y)).map_or(Hit::Nothing, Hit::Disk),
        Page::Account => (0..FIELDS).find(|&i| field(i).contains(x, y)).map_or(Hit::Nothing, Hit::Field),
        // The words beside the box tick it too.
        Page::Confirm if Rect::new(MARGIN, 146, 220, 21).contains(x, y) => Hit::Sure,
        _ => Hit::Nothing,
    }
}

fn lines(s: &mut Surface, y: i32, text: &[&str]) {
    for (i, line) in text.iter().enumerate() {
        s.text(MARGIN, y + i as i32 * 14, line, color::BLACK);
    }
}

/// Breaks text into lines of at most `width` characters, at spaces.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    for word in text.split_whitespace() {
        let last = out.last_mut().expect("never empty");
        if !last.is_empty() && last.chars().count() + 1 + word.chars().count() > width {
            out.push(word.to_owned());
        } else {
            if !last.is_empty() {
                last.push(' ');
            }
            last.push_str(word);
        }
    }
    out
}

/// Draws the whole window, `w` x `h` (the part beyond W x H is left plain).
pub fn draw(s: &mut Surface, w: i32, h: i32, st: &State) {
    s.fill(Rect::new(0, 0, w, h), color::FACE);
    let title = match st.page {
        Page::Disk => "Install FerroOS Heavy: the disk",
        Page::Account => "Install FerroOS Heavy: passphrase and account",
        Page::Confirm => "Install FerroOS Heavy: ready",
        Page::Working => "Installing FerroOS Heavy",
        Page::Done => "FerroOS Heavy is installed",
        Page::Failed => "FerroOS Heavy was not installed",
    };
    s.text_bold(MARGIN, 14, title, color::BLACK);
    s.etched_hline(MARGIN, 34, W - 2 * MARGIN);

    match st.page {
        Page::Disk => {
            lines(s, 48, &["Which disk should FerroOS Heavy be installed on?", "Everything on that disk is erased."]);
            let l = disk_list();
            s.fill(l, color::WHITE);
            s.bevel(l, Bevel::Sunken);
            if st.disks.is_empty() {
                s.text(l.x + 8, l.y + 10, "No disk big enough was found (8 GB at least).", color::BLACK);
            }
            for (i, d) in st.disks.iter().take(MAX_DISKS).enumerate() {
                let r = disk_row(i);
                let chosen = i == st.disk;
                if chosen {
                    s.fill(r, color::NAVY);
                }
                s.text(r.x + 6, r.y + (r.h - GLYPH_H) / 2, &d.line(), if chosen { color::WHITE } else { color::BLACK });
            }
            lines(s, l.bottom() + 12, &["The arrow keys choose; Enter goes on."]);
        }
        Page::Account => {
            for (i, label) in LABELS.iter().enumerate() {
                let f = field(i);
                s.text(MARGIN, f.y + (f.h - GLYPH_H) / 2, label, color::BLACK);
                let stars = "*".repeat(st.fields[i].chars().count());
                let text = if i == NAME { st.fields[i].as_str() } else { &stars };
                text_field(s, f, text, i == st.focus);
            }
            let y = field(FIELDS - 1).bottom() + 12;
            if st.message.is_empty() {
                lines(s, y, &["The passphrase unlocks the disk at every start.", "Nothing can recover it if it's forgotten."]);
            } else {
                s.text(MARGIN, y, st.message, color::RED);
            }
        }
        Page::Confirm => {
            let disk = st.disks.get(st.disk).map(Disk::line).unwrap_or_default();
            let account = format!("Account: {}", st.fields[NAME]);
            lines(s, 52, &["Ready to install on:", "", &disk, "", &account]);
            s.text(MARGIN, 126, "Everything on that disk is erased. This can't be undone.", color::RED);
            let b = sure_box();
            s.fill(b, color::WHITE);
            s.bevel(b, Bevel::Sunken);
            if st.sure {
                s.sprite_mono(b.x + 3, b.y + 3, icons::GLYPH_CHECK, color::BLACK);
            }
            s.text(b.right() + 8, b.y + 3, "Yes, erase this disk", color::BLACK);
            lines(s, 180, &["Space ticks the box; Enter installs."]);
        }
        Page::Working => {
            for (i, (_, label)) in STEPS.iter().enumerate() {
                let y = 56 + i as i32 * 22;
                if i < st.step {
                    s.sprite_mono(MARGIN + 2, y, icons::GLYPH_CHECK, color::BLACK);
                    s.text(MARGIN + 20, y, label, color::BLACK);
                } else if i == st.step {
                    s.sprite_mono(MARGIN + 4, y, icons::GLYPH_SUBMENU, color::BLACK);
                    s.text_bold(MARGIN + 20, y, label, color::BLACK);
                } else {
                    s.text_disabled(MARGIN + 20, y, label);
                }
            }
            lines(s, 190, &["This takes a few minutes.", "Don't turn the computer off."]);
        }
        Page::Done => {
            lines(
                s,
                56,
                &[
                    "It starts from the disk from now on.",
                    "",
                    "Take out the installation medium, then restart.",
                    "The disk's passphrase is asked first, every time.",
                ],
            );
        }
        Page::Failed => {
            lines(s, 56, &["Installing stopped:"]);
            for (i, line) in wrap(&st.error, 54).iter().take(8).enumerate() {
                s.text(MARGIN, 78 + i as i32 * 14, line, color::RED);
            }
        }
    }

    let go = can_go_on(st);
    for (slot, label) in buttons(st.page).into_iter().enumerate() {
        if let Some(label) = label {
            button(s, button_rect(slot as i32), label, slot != 1 || go);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disks() -> Vec<Disk> {
        parse_disks(
            "sda 512110190592 Samsung SSD 860\nnvme0n1 1000204886016 WD_BLACK SN850X\nsdb 15376000000 Cruzer Blade\nsdc 2000000000\n",
            "/dev/sdb1",
        )
    }

    #[test]
    fn lists_disks_but_not_the_medium_or_tiny_ones() {
        let d = disks();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0], Disk { name: "sda".into(), bytes: 512110190592, model: "Samsung SSD 860".into() });
        assert_eq!(d[1].name, "nvme0n1");
        assert_eq!(size_text(d[0].bytes), "476.9 GB");
        assert_eq!(size_text(1000204886016), "931.5 GB");
        assert_eq!(size_text(4 << 40), "4.0 TB");
        // A medium that's a whole disk (a DVD) hides nothing else.
        assert_eq!(parse_disks("sda 512110190592 X\n", "/dev/sr0").len(), 1);
        assert_eq!(parse_disks("sda 512110190592 X\n", "").len(), 1);
    }

    fn answers(a: [&str; FIELDS]) -> [String; FIELDS] {
        a.map(str::to_owned)
    }

    #[test]
    fn checks_the_answers() {
        assert_eq!(check_account(&answers(["long enough", "long enough", "atom", "pw", "pw"])), Ok(()));
        assert!(check_account(&answers(["short", "short", "atom", "pw", "pw"])).is_err());
        assert!(check_account(&answers(["long enough", "long enoug", "atom", "pw", "pw"])).is_err());
        assert!(check_account(&answers(["long enough", "long enough", "atom", "pw", "pW"])).is_err());
        assert!(check_account(&answers(["long enough", "long enough", "atom", "", ""])).is_err());
        for bad in ["", "Atom", "1atom", "at om", "atom;rm", "root\n"] {
            assert!(check_account(&answers(["long enough", "long enough", bad, "pw", "pw"])).is_err(), "{bad:?}");
        }
        assert_eq!(check_account(&answers(["long enough", "long enough", "a-b_9", "pw", "pw"])), Ok(()));
    }

    #[test]
    fn answers_in_the_installers_order() {
        let mut s = State::new(disks());
        s.disk = 1;
        s.fields = answers(["the phrase", "the phrase", "atom", " pw ", " pw "]);
        assert_eq!(s.answers(), "nvme0n1\nthe phrase\nthe phrase\natom\n pw \n pw \nERASE\n");
        s.forget();
        assert_eq!(s.fields, answers(["", "", "atom", "", ""]));
    }

    #[test]
    fn follows_the_installers_steps() {
        // The first step's line follows the questions, on one line.
        assert_eq!(step_of("Install on which disk? Again: == partitioning /dev/sda"), Some(0));
        assert_eq!(step_of("== copying the system"), Some(2));
        assert_eq!(step_of("== finishing"), Some(4));
        assert_eq!(step_of("FerroOS Heavy is installed on /dev/sda."), None);
    }

    #[test]
    fn finds_what_was_clicked() {
        let mut s = State::new(disks());
        let row = disk_row(1);
        assert_eq!(hit(&s, row.x + 3, row.y + 3), Hit::Disk(1));
        // Only rows that hold a disk.
        let empty = disk_row(2);
        assert_eq!(hit(&s, empty.x + 3, empty.y + 3), Hit::Nothing);
        let (back, next, cancel) = (button_rect(0), button_rect(1), button_rect(2));
        assert!(!back.overlaps(&next) && !next.overlaps(&cancel) && cancel.right() <= W);
        // The first page has nowhere to go back to.
        assert_eq!(hit(&s, back.x + 3, back.y + 3), Hit::Nothing);
        assert_eq!(hit(&s, next.x + 3, next.y + 3), Hit::Next);
        assert_eq!(hit(&s, cancel.x + 3, cancel.y + 3), Hit::Cancel);
        s.page = Page::Account;
        assert_eq!(hit(&s, back.x + 3, back.y + 3), Hit::Back);
        let f = field(PASSWORD);
        assert_eq!(hit(&s, f.x + 3, f.y + 3), Hit::Field(PASSWORD));
        assert!(field(FIELDS - 1).bottom() + 40 < next.y);
        s.page = Page::Confirm;
        assert_eq!(hit(&s, sure_box().x + 3, sure_box().y + 3), Hit::Sure);
        assert!(!can_go_on(&s));
        s.sure = true;
        assert!(can_go_on(&s));
        s.page = Page::Working;
        assert_eq!(hit(&s, next.x + 3, next.y + 3), Hit::Nothing);
    }

    #[test]
    fn wraps_long_errors() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
        assert_eq!(wrap("", 9), [""]);
    }

    #[test]
    fn never_draws_what_was_typed_in_secret() {
        let mut s = State::new(disks());
        s.page = Page::Account;
        s.fields = answers(["secret-one", "secret-one", "atom", "pass-a", "pass-a"]);
        let mut a = Surface::new(W as usize, H as usize);
        draw(&mut a, W, H, &s);
        // Other secrets of the same lengths look exactly the same.
        s.fields = answers(["SECRET-TWO", "SECRET-TWO", "atom", "PASS-B", "PASS-B"]);
        let mut b = Surface::new(W as usize, H as usize);
        draw(&mut b, W, H, &s);
        assert_eq!(a.pixels(), b.pixels());
    }
}
