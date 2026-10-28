//! Retro Windows 95 File Types / Associations view and hit-testing.

use crate::config::{default_associations, AssociationEntry, PRESET_APPS};
use ferro_gfx::{color, icons, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};

pub const W: i32 = 440;
pub const H: i32 = 350;

pub const LIST_RECT: Rect = Rect::new(20, 42, 280, 150);
pub const ROW_H: i32 = 18;
pub const VISIBLE_ROWS: usize = (LIST_RECT.h / ROW_H) as usize;

pub const ADD_BTN: Rect = Rect::new(315, 42, 105, 23);
pub const REMOVE_BTN: Rect = Rect::new(315, 70, 105, 23);
pub const RESTORE_BTN: Rect = Rect::new(315, 98, 105, 23);

pub const DET_GRP: Rect = Rect::new(20, 202, 400, 95);
pub const EDIT_BTN: Rect = Rect::new(170, 264, 115, 23);
pub const PRESET_BTN: Rect = Rect::new(295, 264, 115, 23);

pub const OK_BTN: Rect = Rect::new(180, 312, 75, 24);
pub const CANCEL_BTN: Rect = Rect::new(260, 312, 75, 24);
pub const APPLY_BTN: Rect = Rect::new(340, 312, 75, 24);

// Modal Dialog Geometry
pub const DLG_RECT: Rect = Rect::new(35, 60, 370, 200);
pub const DLG_EXT_INPUT: Rect = Rect::new(50, 105, 340, 22);
pub const DLG_CMD_INPUT: Rect = Rect::new(50, 152, 340, 22);
pub const DLG_EDIT_CMD_INPUT: Rect = Rect::new(50, 125, 340, 22);
pub const DLG_OK_BTN: Rect = Rect::new(215, 222, 75, 24);
pub const DLG_CANCEL_BTN: Rect = Rect::new(305, 222, 75, 24);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogState {
    AddType { ext: String, cmd: String, active_field: usize },
    EditCommand { cmd: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Select(usize),
    AddType,
    RemoveType,
    EditCommand,
    CyclePreset,
    RestoreDefaults,
    Ok,
    Cancel,
    Apply,
    DialogOk,
    DialogCancel,
    DialogFocusField(usize),
    Nothing,
}

pub struct State {
    pub entries: Vec<AssociationEntry>,
    pub selected: usize,
    pub scroll: usize,
    pub applied: bool,
    pub dialog: Option<DialogState>,
    /// Which of `CONTROLS` has the keyboard, if any (Tab moves it).
    pub focus: Option<usize>,
}

/// The buttons, in Tab order.
pub const CONTROLS: [Rect; 8] = [ADD_BTN, REMOVE_BTN, RESTORE_BTN, EDIT_BTN, PRESET_BTN, OK_BTN, CANCEL_BTN, APPLY_BTN];

impl Default for State {
    fn default() -> Self {
        Self { entries: default_associations(), selected: 0, scroll: 0, applied: true, dialog: None, focus: None }
    }
}

pub fn hit(x: i32, y: i32, state: &State) -> Hit {
    if let Some(ref dlg) = state.dialog {
        if DLG_OK_BTN.contains(x, y) {
            return Hit::DialogOk;
        }
        if DLG_CANCEL_BTN.contains(x, y) {
            return Hit::DialogCancel;
        }
        match dlg {
            DialogState::AddType { .. } => {
                if DLG_EXT_INPUT.contains(x, y) {
                    return Hit::DialogFocusField(0);
                }
                if DLG_CMD_INPUT.contains(x, y) {
                    return Hit::DialogFocusField(1);
                }
            }
            DialogState::EditCommand { .. } => {
                if DLG_EDIT_CMD_INPUT.contains(x, y) {
                    return Hit::DialogFocusField(0);
                }
            }
        }
        return Hit::Nothing;
    }

    if OK_BTN.contains(x, y) {
        return Hit::Ok;
    }
    if CANCEL_BTN.contains(x, y) {
        return Hit::Cancel;
    }
    if APPLY_BTN.contains(x, y) {
        return Hit::Apply;
    }
    if ADD_BTN.contains(x, y) {
        return Hit::AddType;
    }
    if REMOVE_BTN.contains(x, y) {
        return Hit::RemoveType;
    }
    if EDIT_BTN.contains(x, y) {
        return Hit::EditCommand;
    }
    if PRESET_BTN.contains(x, y) {
        return Hit::CyclePreset;
    }
    if RESTORE_BTN.contains(x, y) {
        return Hit::RestoreDefaults;
    }

    if LIST_RECT.contains(x, y) {
        let row = ((y - LIST_RECT.y) / ROW_H) as usize;
        let idx = state.scroll + row;
        if idx < state.entries.len() {
            return Hit::Select(idx);
        }
    }

    Hit::Nothing
}

pub fn draw(s: &mut Surface, w: i32, h: i32, state: &State) {
    s.fill(Rect::new(0, 0, w, h), color::FACE);

    // Tab Header
    let tab = Rect::new(10, 6, 80, 20);
    s.fill(tab, color::FACE);
    s.bevel(tab, Bevel::Raised);
    s.text(16, 9, "File Types", color::BLACK);

    // List header label
    s.text(20, 28, "Registered file types:", color::BLACK);

    // List Box
    s.fill(LIST_RECT, color::WHITE);
    s.bevel(LIST_RECT, Bevel::Sunken);

    for (i, entry) in state.entries.iter().skip(state.scroll).take(VISIBLE_ROWS).enumerate() {
        let abs_idx = state.scroll + i;
        let ry = LIST_RECT.y + 2 + i as i32 * ROW_H;
        let row_rect = Rect::new(LIST_RECT.x + 2, ry, LIST_RECT.w - 4, ROW_H);

        let is_sel = abs_idx == state.selected;
        if is_sel {
            s.fill(row_rect, color::NAVY);
        }

        let icon = if entry.ext == "png" || entry.ext == "jpg" || entry.ext == "bmp" {
            icons::FILE
        } else if entry.ext == "mp4" || entry.ext == "mp3" {
            icons::DRIVE
        } else {
            icons::FILE
        };
        s.sprite(row_rect.x + 2, ry + 1, icon, 1);

        let text_color = if is_sel { color::WHITE } else { color::BLACK };
        let ext_str = format!(".{}", entry.ext);
        s.text_bold(row_rect.x + 22, ry + 3, &ext_str, text_color);

        let desc_x = row_rect.x + 80;
        s.text(desc_x, ry + 3, &entry.desc, text_color);
    }

    // Right action buttons
    s.button(ADD_BTN, false);
    s.text(ADD_BTN.x + 12, ADD_BTN.y + 6, "New Type...", color::BLACK);

    s.button(REMOVE_BTN, false);
    s.text(REMOVE_BTN.x + 24, REMOVE_BTN.y + 6, "Remove", color::BLACK);

    s.button(RESTORE_BTN, false);
    s.text(RESTORE_BTN.x + 8, RESTORE_BTN.y + 6, "Restore Defaults", color::BLACK);

    // Details Group Box
    s.bevel(DET_GRP, Bevel::Sunken);
    let det_title = Rect::new(28, 200, 105, GLYPH_H + 2);
    s.fill(det_title, color::FACE);
    s.text(30, 201, "File type details", color::BLACK);

    if let Some(entry) = state.entries.get(state.selected) {
        let ext_lbl = format!("Extension: .{}", entry.ext);
        s.text(30, 218, &ext_lbl, color::BLACK);

        let cmd_lbl = format!("Opens with: {}", entry.command);
        let max_chars = ((w - 60) / GLYPH_W) as usize;
        let trunc_cmd: String = cmd_lbl.chars().take(max_chars).collect();
        s.text(30, 236, &trunc_cmd, color::NAVY);
    }

    // Detail Action buttons
    s.button(EDIT_BTN, false);
    s.text(EDIT_BTN.x + 8, EDIT_BTN.y + 6, "Edit Command...", color::BLACK);

    s.button(PRESET_BTN, false);
    s.text(PRESET_BTN.x + 12, PRESET_BTN.y + 6, "Choose App...", color::BLACK);

    // Bottom action buttons
    ferro_ui::default_button(s, OK_BTN, "OK", true);
    ferro_ui::button(s, CANCEL_BTN, "Cancel", true);
    ferro_ui::button(s, APPLY_BTN, "Apply", true);

    // OK, the default button, has its dotted outline already.
    if let Some(r) = state.focus.and_then(|i| CONTROLS.get(i)).filter(|&&r| r != OK_BTN) {
        ferro_ui::focus_rect(s, r.inset(3));
    }

    // Modal Dialog Overlay
    if let Some(ref dlg) = state.dialog {
        s.fill(DLG_RECT, color::FACE);
        s.bevel(DLG_RECT, Bevel::Raised);

        let title_bar = Rect::new(DLG_RECT.x + 3, DLG_RECT.y + 3, DLG_RECT.w - 6, 18);
        s.fill(title_bar, color::NAVY);

        match dlg {
            DialogState::AddType { ext, cmd, active_field } => {
                s.text(title_bar.x + 6, title_bar.y + 3, "Add New File Type", color::WHITE);
                s.text(50, 88, "File extension (e.g. exe):", color::BLACK);
                ferro_ui::text_field(s, DLG_EXT_INPUT, ext, *active_field == 0);

                s.text(50, 135, "Command to execute (e.g. wine %f):", color::BLACK);
                ferro_ui::text_field(s, DLG_CMD_INPUT, cmd, *active_field == 1);
            }
            DialogState::EditCommand { cmd } => {
                s.text(title_bar.x + 6, title_bar.y + 3, "Edit File Association", color::WHITE);
                s.text(50, 105, "Command to execute:", color::BLACK);
                ferro_ui::text_field(s, DLG_EDIT_CMD_INPUT, cmd, true);
            }
        }

        ferro_ui::default_button(s, DLG_OK_BTN, "OK", true);
        ferro_ui::button(s, DLG_CANCEL_BTN, "Cancel", true);
    }
}

pub fn cycle_preset(cmd: &str) -> String {
    for (i, (_, app_cmd)) in PRESET_APPS.iter().enumerate() {
        if *app_cmd == cmd {
            let next_idx = (i + 1) % PRESET_APPS.len();
            return PRESET_APPS[next_idx].1.to_string();
        }
    }
    PRESET_APPS[0].1.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_test_buttons() {
        let state = State::default();
        assert_eq!(hit(OK_BTN.x + 2, OK_BTN.y + 2, &state), Hit::Ok);
        assert_eq!(hit(CANCEL_BTN.x + 2, CANCEL_BTN.y + 2, &state), Hit::Cancel);
        assert_eq!(hit(APPLY_BTN.x + 2, APPLY_BTN.y + 2, &state), Hit::Apply);
        assert_eq!(hit(ADD_BTN.x + 2, ADD_BTN.y + 2, &state), Hit::AddType);
        assert_eq!(hit(REMOVE_BTN.x + 2, REMOVE_BTN.y + 2, &state), Hit::RemoveType);
        assert_eq!(hit(EDIT_BTN.x + 2, EDIT_BTN.y + 2, &state), Hit::EditCommand);
        assert_eq!(hit(PRESET_BTN.x + 2, PRESET_BTN.y + 2, &state), Hit::CyclePreset);
        assert_eq!(hit(RESTORE_BTN.x + 2, RESTORE_BTN.y + 2, &state), Hit::RestoreDefaults);
    }

    #[test]
    fn hit_test_dialogs() {
        let state = State { dialog: Some(DialogState::AddType { ext: "exe".into(), cmd: "wine %f".into(), active_field: 0 }), ..Default::default() };
        assert_eq!(hit(DLG_OK_BTN.x + 2, DLG_OK_BTN.y + 2, &state), Hit::DialogOk);
        assert_eq!(hit(DLG_CANCEL_BTN.x + 2, DLG_CANCEL_BTN.y + 2, &state), Hit::DialogCancel);
        assert_eq!(hit(DLG_EXT_INPUT.x + 2, DLG_EXT_INPUT.y + 2, &state), Hit::DialogFocusField(0));
        assert_eq!(hit(DLG_CMD_INPUT.x + 2, DLG_CMD_INPUT.y + 2, &state), Hit::DialogFocusField(1));
    }

    #[test]
    fn cycle_preset_advances() {
        let first = PRESET_APPS[0].1;
        let second = cycle_preset(first);
        assert_ne!(first, second);
        assert_eq!(second, PRESET_APPS[1].1);
    }
}
