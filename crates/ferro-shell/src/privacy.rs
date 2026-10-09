//! Privacy controls: the tray kill switches, the RAM budget dialog, and
//! the App Permissions window where remembered choices can be erased.
//!
//! All choices live in one plain-text store (`ferro_sandbox::store`). It's
//! shared with ferro-run, which reads it when starting apps.

use super::*;
use ferro_sandbox::store::{NetChoice, Store, Switches};

const SWITCH_ICONS: [(Icon, &str); 3] = [(icons::NETWORK, "Network"), (icons::MICROPHONE, "Microphone"), (icons::CAMERA, "Camera")];
pub(crate) const TRAY_ICON_W: i32 = 20;

// App Permissions item indices.
const PERM_FORGET: usize = 0;
const PERM_FORGET_ALL: usize = 1;
const PERM_CLOSE: usize = 2;
const PERM_ROW0: usize = 3;
const PERM_ROW_H: i32 = 16;

// RAM budget dialog item indices.
const IN_FIELD: usize = 0;
const IN_REMEMBER: usize = 1;
const IN_OK: usize = 2;
const IN_CANCEL: usize = 3;

pub(crate) struct BudgetInput {
    pid: u32,
    app: String,
    text: String,
    remember: bool,
    error: Option<String>,
}

impl BudgetInput {
    #[cfg(test)]
    pub(crate) fn error_text(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

impl Shell {
    // ---- store -------------------------------------------------------------------

    /// Where remembered choices live; loads them (kill switches included).
    pub fn set_store_path(&mut self, path: impl Into<std::path::PathBuf>) {
        self.store = Store::load(path);
    }

    /// Current kill-switch states, for the backend to enforce.
    pub fn switches(&self) -> Switches {
        self.store.switches
    }

    pub(crate) fn save_store(&mut self) {
        if let Err(e) = self.store.save() {
            self.show_error("Settings", &format!("Couldn't save your choices: {e}"));
        }
    }

    // ---- tray kill switches --------------------------------------------------------

    pub(crate) fn tray_icon_rects(&self) -> [Rect; 3] {
        let tray = self.tray_rect();
        std::array::from_fn(|i| Rect::new(tray.x + 4 + i as i32 * TRAY_ICON_W, tray.y + 3, 16, 16))
    }

    pub(crate) fn switch_state(&self, i: usize) -> bool {
        let s = self.store.switches;
        [s.network, s.microphone, s.camera][i]
    }

    pub(crate) fn toggle_switch(&mut self, i: usize) -> Option<Action> {
        let s = &mut self.store.switches;
        let slot = [&mut s.network, &mut s.microphone, &mut s.camera].into_iter().nth(i)?;
        *slot = !*slot;
        self.save_store();
        self.push_switches();
        Some(Action::SwitchesChanged)
    }

    pub(crate) fn draw_tray_icons(&self, s: &mut Surface) {
        for (i, (r, (icon, _))) in self.tray_icon_rects().iter().zip(SWITCH_ICONS).enumerate() {
            s.sprite(r.x, r.y, icon, 1);
            if !self.switch_state(i) {
                // Off: a bold red slash, like a physical kill switch.
                for d in 0..2 {
                    s.line(r.x + 1 + d, r.bottom() - 2, r.right() - 2 + d, r.y + 1, RED);
                }
            }
        }
    }

    // ---- RAM budget dialog -----------------------------------------------------------

    /// Asks for a typed budget for process `pid` (shown as `app`).
    pub(crate) fn open_budget_dialog(&mut self, pid: u32, app: &str) {
        let current = self.store.app(app).memory.flatten();
        let text =
            current.map_or(
                String::new(),
                |b| {
                    if b >= 1 << 30 && b.is_multiple_of(1 << 30) {
                        format!("{}G", b >> 30)
                    } else {
                        format!("{}M", b >> 20)
                    }
                },
            );
        let input = BudgetInput { pid, app: app.to_owned(), text, remember: true, error: None };
        let id = self.next_id;
        self.open(format!("RAM Budget - {app}"), icons::TASKMGR, (400, 196), Content::Input(Box::new(input)));
        let (sw, sh) = (self.w, self.h - TASKBAR_H);
        if let Some(w) = self.window_mut(id) {
            w.rect.x = (sw - w.rect.w) / 2;
            w.rect.y = (sh - w.rect.h) / 2;
        }
    }

    pub(crate) fn input_items(&self, win: &Window) -> Vec<Rect> {
        let c = client_rect(win.rect);
        vec![
            Rect::new(c.x + 12, c.y + 44, c.w - 24, 22),
            Rect::new(c.x + 12, c.y + 96, c.w - 24, 16),
            Rect::new(c.right() - 170, c.bottom() - 33, 75, 23),
            Rect::new(c.right() - 87, c.bottom() - 33, 75, 23),
        ]
    }

    pub(crate) fn draw_input(&self, s: &mut Surface, win: &Window, input: &BudgetInput, items: &[Rect]) {
        let c = client_rect(win.rect);
        s.text(c.x + 12, c.y + 10, "Most RAM this app may use before it's stopped.", BLACK);
        s.text(c.x + 12, c.y + 24, "Examples: 5G, 512M, 50 MB, or none", SHADOW);
        let field = items[IN_FIELD];
        s.fill(field, WHITE);
        s.bevel(field, Bevel::Sunken);
        s.text(field.x + 5, field.y + 7, &input.text, BLACK);
        let caret_x = field.x + 5 + text_width(&input.text);
        s.vline(caret_x, field.y + 5, 12, BLACK);
        if let Some(e) = &input.error {
            s.text(c.x + 12, c.y + 74, e, RED);
        }
        draw_checkbox(s, items[IN_REMEMBER], input.remember, &format!("Always use this budget for {}", ellipsize(&input.app, 20)));
        self.draw_push_button(s, items[IN_OK], Hit::Item(win.id, IN_OK), "OK");
        self.draw_push_button(s, items[IN_CANCEL], Hit::Item(win.id, IN_CANCEL), "Cancel");
    }

    fn input_mut(&mut self, id: u32) -> Option<&mut BudgetInput> {
        match &mut self.window_mut(id)?.content {
            Content::Input(i) => Some(i),
            _ => None,
        }
    }

    /// Keys typed while the budget dialog is focused.
    pub(crate) fn input_key(&mut self, id: u32, key: Key, mods: Mods) -> Option<Action> {
        match key {
            Key::Enter => return self.input_click(id, IN_OK),
            Key::Escape => self.close(id),
            Key::Backspace => {
                self.input_mut(id)?.text.pop();
            }
            // Ctrl/Alt combinations are shortcuts, not text.
            Key::Char(c) if !mods.ctrl && !mods.alt && (c.is_ascii_alphanumeric() || c == '.' || c == ' ') => {
                let i = self.input_mut(id)?;
                if i.text.len() < 16 {
                    i.text.push(c);
                    i.error = None;
                }
            }
            _ => {}
        }
        None
    }

    pub(crate) fn input_press(&mut self, id: u32, item: usize) {
        if item == IN_REMEMBER {
            if let Some(i) = self.input_mut(id) {
                i.remember = !i.remember;
            }
        }
    }

    pub(crate) fn input_click(&mut self, id: u32, item: usize) -> Option<Action> {
        match item {
            IN_CANCEL => self.close(id),
            IN_OK => {
                let input = self.input_mut(id)?;
                let Some(bytes) = ferro_sandbox::parse_budget(&input.text) else {
                    input.error = Some("Type a size like 5G, 512M or 50 MB.".into());
                    return None;
                };
                if bytes.is_some_and(|b| b < 4 << 20) {
                    input.error = Some("That's too small to run anything (min 4 MB).".into());
                    return None;
                }
                let (pid, app, remember) = (input.pid, input.app.clone(), input.remember);
                self.close(id);
                if remember {
                    self.store.set_memory(&app, Some(bytes));
                    self.save_store();
                }
                return self.apply_limit(pid, bytes);
            }
            _ => {}
        }
        None
    }

    /// Applies a RAM budget through the privileged side.
    pub(crate) fn apply_limit(&mut self, pid: u32, bytes: Option<u64>) -> Option<Action> {
        if let Some(Err(e)) = self.link.as_mut().map(|l| l.limit_memory(pid, bytes)) {
            self.show_error("RAM Budget", &format!("Couldn't apply it: {e}"));
        }
        Some(Action::SetMemoryLimit { pid, bytes })
    }

    // ---- App Permissions window ------------------------------------------------------

    pub fn open_app_permissions(&mut self) {
        if !self.focus_existing(|c| matches!(c, Content::AppPermissions { .. })) {
            self.open("App Permissions".into(), icons::FERRO, (470, 320), Content::AppPermissions { selected: None });
        }
    }

    fn perm_rows(&self) -> Vec<(&String, String, String)> {
        self.store
            .apps
            .iter()
            .map(|(name, c)| {
                let net = match c.network {
                    Some(NetChoice::Allow) => "Always allow",
                    Some(NetChoice::Deny) => "Never allow",
                    None => "Ask",
                };
                let mem = match c.memory {
                    Some(Some(b)) if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
                    Some(Some(b)) => format!("{} MB", b >> 20),
                    Some(None) => "No limit".into(),
                    None => "Default".into(),
                };
                (name, net.to_owned(), mem)
            })
            .collect()
    }

    fn perm_list_rect(win: &Window) -> Rect {
        let c = client_rect(win.rect);
        Rect::new(c.x + 8, c.y + 28, c.w - 16, c.h - 28 - 40)
    }

    pub(crate) fn perm_items(&self, win: &Window) -> Vec<Rect> {
        let c = client_rect(win.rect);
        let list = Self::perm_list_rect(win);
        let by = c.bottom() - 32;
        let mut v = vec![Rect::new(c.x + 8, by, 80, 23), Rect::new(c.x + 94, by, 90, 23), Rect::new(c.right() - 83, by, 75, 23)];
        let rows = ((list.h - 4 - PERM_ROW_H) / PERM_ROW_H).max(0) as usize;
        let n = self.store.apps.len().min(rows);
        v.extend((0..n as i32).map(|k| Rect::new(list.x + 2, list.y + 2 + PERM_ROW_H * (k + 1), list.w - 4, PERM_ROW_H)));
        v
    }

    pub(crate) fn draw_permissions(&self, s: &mut Surface, win: &Window, selected: Option<usize>, items: &[Rect]) {
        let c = client_rect(win.rect);
        s.text(c.x + 8, c.y + 10, "Choices FerroOS remembers. Forget them any time.", BLACK);
        let list = Self::perm_list_rect(win);
        s.fill(list, WHITE);
        s.bevel(list, Bevel::Sunken);
        let cols = [list.x + 6, list.x + 200, list.x + 320];
        let header_y = list.y + 2;
        s.fill(Rect::new(list.x + 2, header_y, list.w - 4, PERM_ROW_H), FACE);
        for (x, h) in cols.iter().zip(["App", "Network", "RAM Budget"]) {
            s.text_bold(*x, header_y + 4, h, BLACK);
        }
        let rows = self.perm_rows();
        if rows.is_empty() {
            s.text(list.x + 6, list.y + 28, "Nothing remembered. Apps ask before using the network.", SHADOW);
        }
        for (k, ((name, net, mem), r)) in rows.iter().zip(items.iter().skip(PERM_ROW0)).enumerate() {
            let on = selected == Some(k);
            if on {
                s.fill(*r, NAVY);
            }
            let fg = if on { WHITE } else { BLACK };
            s.text(cols[0], r.y + 4, &ellipsize(name, 23), fg);
            s.text(cols[1], r.y + 4, net, fg);
            s.text(cols[2], r.y + 4, mem, fg);
        }
        let has_sel = selected.is_some_and(|k| k < rows.len());
        let b = items[PERM_FORGET];
        s.button(b, has_sel && self.is_held(Hit::Item(win.id, PERM_FORGET), b));
        if has_sel {
            s.text(b.x + (b.w - text_width("Forget")) / 2, b.y + 8, "Forget", BLACK);
        } else {
            s.text_disabled(b.x + (b.w - text_width("Forget")) / 2, b.y + 8, "Forget");
        }
        self.draw_push_button(s, items[PERM_FORGET_ALL], Hit::Item(win.id, PERM_FORGET_ALL), "Forget All");
        self.draw_push_button(s, items[PERM_CLOSE], Hit::Item(win.id, PERM_CLOSE), "Close");
    }

    pub(crate) fn perm_press(&mut self, id: u32, item: usize) {
        if item >= PERM_ROW0 {
            if let Some(Window { content: Content::AppPermissions { selected }, .. }) = self.window_mut(id) {
                *selected = Some(item - PERM_ROW0);
            }
        }
    }

    pub(crate) fn perm_click(&mut self, id: u32, item: usize) {
        match item {
            PERM_CLOSE => self.close(id),
            PERM_FORGET_ALL => {
                self.store.forget_all();
                self.save_store();
            }
            PERM_FORGET => {
                let sel = match &self.window(id).map(|w| &w.content) {
                    Some(Content::AppPermissions { selected }) => *selected,
                    _ => None,
                };
                if let Some(name) = sel.and_then(|k| self.store.apps.keys().nth(k).cloned()) {
                    self.store.forget(&name);
                    self.save_store();
                }
            }
            _ => return,
        }
        if let Some(Window { content: Content::AppPermissions { selected }, .. }) = self.window_mut(id) {
            *selected = None;
        }
    }
}

pub(crate) fn draw_checkbox(s: &mut Surface, r: Rect, on: bool, label: &str) {
    let b = Rect::new(r.x, r.y + 1, 13, 13);
    s.fill(b, WHITE);
    s.bevel(b, Bevel::Sunken);
    if on {
        s.sprite_mono(b.x + 3, b.y + 3, icons::GLYPH_CHECK, BLACK);
    }
    s.text(r.x + 20, r.y + 4, label, BLACK);
}
