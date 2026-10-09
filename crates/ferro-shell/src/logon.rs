//! "Welcome to FerroOS": the password dialog that unlocks (or creates) the
//! encrypted vault before the desktop appears, Win95-logon style.

use super::*;
use ferro_system::valid_password;

pub use ferro_system::VaultState;

/// What the desktop needs from the privileged side (ferro-system on
/// FerroOS, an in-process stand-in in the preview).
pub trait SystemLink {
    fn vault_state(&mut self) -> VaultState;
    fn create_vault(&mut self, password: &str) -> Result<(), String>;
    fn unlock_vault(&mut self, password: &str) -> Result<(), String>;
    /// Don't save anything this session.
    fn amnesic(&mut self);
    fn apply_switches(&mut self, switches: ferro_sandbox::store::Switches);
    fn limit_memory(&mut self, pid: u32, bytes: Option<u64>) -> Result<(), String>;
    /// Disks FerroOS Setup may install to.
    fn list_disks(&mut self) -> Result<Vec<crate::DiskInfo>, String> {
        Err("Setup isn't available here.".into())
    }
    /// Starts installing on `disk` (erasing it).
    fn install(&mut self, disk: &str) -> Result<(), String> {
        let _ = disk;
        Err("Setup isn't available here.".into())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Create,
    Unlock,
}

pub(crate) struct Logon {
    mode: Mode,
    fields: [String; 2],
    focus: usize,
    error: Option<String>,
}

pub(crate) struct Layout {
    dialog: Rect,
    pub(crate) fields: [Rect; 2],
    ok: Rect,
    pub(crate) other: Rect,
}

const FIELD_MAX: usize = 64;

impl Shell {
    pub fn set_system_link(&mut self, link: Box<dyn SystemLink>) {
        self.link = Some(link);
    }

    /// Shows the logon dialog if there's a vault to create or unlock.
    pub fn begin_logon(&mut self) {
        let Some(link) = self.link.as_mut() else { return };
        let mode = match link.vault_state() {
            VaultState::New => Mode::Create,
            VaultState::Locked => Mode::Unlock,
            // Nothing to unlock (a live session): the session starts now.
            _ => {
                self.push_switches();
                return;
            }
        };
        self.logon = Some(Logon { mode, fields: Default::default(), focus: 0, error: None });
    }

    pub fn logon_active(&self) -> bool {
        self.logon.is_some()
    }

    pub(crate) fn logon_layout(&self, mode: Mode) -> Layout {
        let h = if mode == Mode::Create { 262 } else { 220 };
        let d = Rect::new((self.w - 460) / 2, (self.h - h) / 2, 460, h);
        let fy = d.y + 118;
        let field = |i: i32| Rect::new(d.x + 150, fy + i * 30, 286, 22);
        let by = d.bottom() - 36;
        Layout {
            dialog: d,
            fields: [field(0), field(1)],
            ok: Rect::new(d.right() - 230, by, 100, 23),
            other: Rect::new(d.right() - 122, by, 110, 23),
        }
    }

    pub(crate) fn draw_logon(&self, s: &mut Surface, logon: &Logon) {
        s.clear(DESKTOP);
        let l = self.logon_layout(logon.mode);
        let d = l.dialog;
        s.fill(d, FACE);
        s.bevel(d, Bevel::Window);
        let t = title_rect(d);
        s.fill(t, NAVY);
        s.sprite(t.x + 1, t.y + 1, icons::FERRO, 1);
        s.text_bold(t.x + 20, t.y + 5, "Welcome to FerroOS", WHITE);
        s.sprite(d.x + 16, d.y + 38, icons::FERRO, 2);
        let intro = match logon.mode {
            Mode::Create => "Choose a password. It encrypts your settings, choices and files on disk. If you forget it, they can't be recovered.",
            Mode::Unlock => "Type your password to unlock your saved settings, choices and files.",
        };
        for (i, line) in wrap(intro, 44).iter().enumerate() {
            s.text(d.x + 70, d.y + 40 + i as i32 * 14, line, BLACK);
        }
        let labels = ["Password:", "Confirm:"];
        let count = if logon.mode == Mode::Create { 2 } else { 1 };
        for (i, label) in labels.iter().enumerate().take(count) {
            let f = l.fields[i];
            s.text(d.x + 70, f.y + 7, label, BLACK);
            s.fill(f, WHITE);
            s.bevel(f, Bevel::Sunken);
            // As many stars as fit: a long password mustn't spill out of the box.
            let fit = ((f.w - 12) / text_width("*")).max(1) as usize;
            let masked = "*".repeat(logon.fields[i].chars().count().min(fit));
            s.text(f.x + 5, f.y + 7, &masked, BLACK);
            if logon.focus == i {
                s.vline(f.x + 5 + text_width(&masked), f.y + 5, 12, BLACK);
            }
        }
        if let Some(e) = &logon.error {
            s.text(d.x + 70, l.fields[count - 1].bottom() + 8, e, RED);
        }
        let other = if logon.mode == Mode::Create { "Don't Save" } else { "Skip" };
        for (r, label) in [(l.ok, "OK"), (l.other, other)] {
            s.button(r, false);
            s.text(r.x + (r.w - text_width(label)) / 2, r.y + 8, label, BLACK);
        }
        s.sprite(self.cursor.0, self.cursor.1, icons::CURSOR, 1);
    }

    pub(crate) fn logon_event(&mut self, ev: Event) -> Option<Action> {
        let mode = self.logon.as_ref()?.mode;
        match ev {
            Event::MouseMove { x, y } => self.cursor = (x, y),
            Event::MouseDown { x, y, button: MouseButton::Left } => {
                self.cursor = (x, y);
                let l = self.logon_layout(mode);
                if l.ok.contains(x, y) {
                    return self.submit_logon();
                }
                if l.other.contains(x, y) {
                    self.skip_logon();
                    return None;
                }
                let count = if mode == Mode::Create { 2 } else { 1 };
                if let Some(i) = (0..count).find(|&i| l.fields[i].contains(x, y)) {
                    self.logon.as_mut()?.focus = i;
                }
            }
            Event::Key { key, mods } => {
                let logon = self.logon.as_mut()?;
                let count = if mode == Mode::Create { 2 } else { 1 };
                match key {
                    Key::Enter if logon.focus + 1 < count => logon.focus += 1,
                    Key::Enter => return self.submit_logon(),
                    Key::Tab => logon.focus = (logon.focus + 1) % count,
                    Key::Backspace => {
                        logon.fields[logon.focus].pop();
                    }
                    Key::Char(c) if !mods.ctrl && !mods.alt && !c.is_control() => {
                        let f = &mut logon.fields[logon.focus];
                        if f.chars().count() < FIELD_MAX {
                            f.push(c);
                            logon.error = None;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }

    fn submit_logon(&mut self) -> Option<Action> {
        let logon = self.logon.as_mut()?;
        let password = logon.fields[0].clone();
        let result = match logon.mode {
            Mode::Create => {
                if let Err(e) = valid_password(&password) {
                    Err(e.to_owned())
                } else if logon.fields[1] != password {
                    Err("The passwords don't match.".to_owned())
                } else {
                    self.link.as_mut().map_or(Err("No system service.".into()), |l| l.create_vault(&password))
                }
            }
            Mode::Unlock => self.link.as_mut().map_or(Err("No system service.".into()), |l| l.unlock_vault(&password)),
        };
        match result {
            Ok(()) => {
                self.logon = None;
                // Saved choices (kill switches included) were just restored.
                self.store = Store::load(self.store.path.clone());
                self.push_switches();
                Some(Action::SwitchesChanged)
            }
            Err(e) => {
                let logon = self.logon.as_mut()?;
                logon.error = Some(e);
                logon.fields = Default::default();
                logon.focus = 0;
                None
            }
        }
    }

    fn skip_logon(&mut self) {
        if let Some(link) = self.link.as_mut() {
            link.amnesic();
        }
        self.logon = None;
        self.push_switches();
    }

    /// Sends the kill-switch states to the privileged side to enforce.
    pub(crate) fn push_switches(&mut self) {
        let s = self.store.switches;
        if let Some(link) = self.link.as_mut() {
            link.apply_switches(s);
        }
    }
}
