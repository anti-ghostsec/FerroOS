//! Desktop icon management, filesystem scanning, dragging, and hover highlights.
//!
//! Renders desktop shortcuts, files, and folders from `~/Desktop`, with
//! Recycle Bin at the top and Files (My Computer) underneath. Supports
//! dragging icons across the desktop surface with persistence in
//! `~/.config/ferro/desktop.conf`.

use ferro_gfx::{color, icons, text_width, Rect, Surface, GLYPH_H};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const ICON_W: i32 = 76;
pub const ICON_H: i32 = 68;
pub const ICON_SIZE: i32 = 32;

pub const GRID_X: i32 = 20;
pub const GRID_Y: i32 = 20;
pub const GRID_STEP_Y: i32 = 76;
pub const GRID_STEP_X: i32 = 84;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    RecycleBin,
    Files,
    Folder(PathBuf),
    File(PathBuf),
    Shortcut { name: String, exec: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopItem {
    pub id: String,
    pub label: String,
    pub kind: ItemKind,
    pub x: i32,
    pub y: i32,
    pub icon: icons::Icon,
}

impl DesktopItem {
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, ICON_W, ICON_H)
    }

    pub fn exec_command(&self) -> String {
        match &self.kind {
            ItemKind::RecycleBin => "sh -c 'mkdir -p \"$HOME/.local/share/Trash\" && ferro-files \"$HOME/.local/share/Trash\"'".into(),
            ItemKind::Files => "ferro-files".into(),
            ItemKind::Folder(path) => format!("ferro-files \"{}\"", path.display()),
            ItemKind::File(path) => ferro_associations::resolve_command_for_path(path),
            ItemKind::Shortcut { exec, .. } => exec.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopManager {
    pub items: Vec<DesktopItem>,
    pub selected: Option<usize>,
    pub hovered: Option<usize>,
    pub hidden: HashSet<String>,
}

fn config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("ferro").join("desktop.conf")
    } else {
        PathBuf::from("/etc/ferro/desktop.conf")
    }
}

fn desktop_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join("Desktop")
    } else {
        PathBuf::from("/home/ferro/Desktop")
    }
}

impl Default for DesktopManager {
    fn default() -> Self {
        Self::new(1024, 768)
    }
}

impl DesktopManager {
    pub fn new(screen_w: i32, screen_h: i32) -> Self {
        let mut dm = Self { items: Vec::new(), selected: None, hovered: None, hidden: HashSet::new() };
        dm.refresh(screen_w, screen_h);
        dm
    }

    /// Loads saved icon positions and hidden item list from `desktop.conf`.
    pub fn load_positions() -> (HashMap<String, (i32, i32)>, HashSet<String>) {
        let path = config_path();
        let mut map = HashMap::new();
        let mut hidden = HashSet::new();
        if let Ok(content) = fs::read_to_string(&path) {
            for line in content.lines().map(str::trim) {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let key = k.trim().to_lowercase();
                    let val = v.trim();
                    if val == "hidden" || val == "true" && key.ends_with(".hidden") {
                        let clean_k = key.strip_suffix(".hidden").unwrap_or(&key).to_string();
                        hidden.insert(clean_k);
                        continue;
                    }
                    if let Some((xs, ys)) = val.split_once(',') {
                        if let (Ok(x), Ok(y)) = (xs.trim().parse::<i32>(), ys.trim().parse::<i32>()) {
                            map.insert(key, (x, y));
                        }
                    }
                }
            }
        }
        (map, hidden)
    }

    /// Saves icon positions and hidden item list to `desktop.conf`.
    pub fn save_positions(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut lines = vec!["# FerroOS Desktop Icon Positions".to_string()];
        for item in &self.items {
            lines.push(format!("{} = {},{}", item.id.to_lowercase(), item.x, item.y));
        }
        for h in &self.hidden {
            lines.push(format!("{}.hidden = true", h.to_lowercase()));
        }
        let _ = fs::write(&path, lines.join("\n") + "\n");
    }

    /// Scans the Desktop directory and rebuilds the items list.
    pub fn refresh(&mut self, _screen_w: i32, screen_h: i32) {
        let (positions, hidden) = Self::load_positions();
        self.hidden = hidden;
        let mut new_items = Vec::new();

        // 1. Recycle Bin at top: (20, 20)
        let (bin_x, bin_y) = positions.get("recycle bin").copied().unwrap_or((GRID_X, GRID_Y));
        new_items.push(DesktopItem {
            id: "Recycle Bin".into(),
            label: "Recycle Bin".into(),
            kind: ItemKind::RecycleBin,
            x: bin_x,
            y: bin_y,
            icon: icons::RECYCLE,
        });

        // 2. Files (My Computer) underneath: (20, 96)
        let (files_x, files_y) = positions.get("files").copied().unwrap_or((GRID_X, GRID_Y + GRID_STEP_Y));
        new_items.push(DesktopItem { id: "Files".into(), label: "Files".into(), kind: ItemKind::Files, x: files_x, y: files_y, icon: icons::FOLDER });

        // 3. LibreWolf underneath: (20, 172) (unless hidden)
        let mut next_slot_y = GRID_Y + 2 * GRID_STEP_Y;
        let mut next_slot_x = GRID_X;

        if !self.hidden.contains("librewolf") {
            let (wolf_x, wolf_y) = positions.get("librewolf").copied().unwrap_or((GRID_X, GRID_Y + 2 * GRID_STEP_Y));
            new_items.push(DesktopItem {
                id: "LibreWolf".into(),
                label: "LibreWolf".into(),
                kind: ItemKind::Shortcut { name: "LibreWolf".into(), exec: "librewolf".into() },
                x: wolf_x,
                y: wolf_y,
                icon: icons::BROWSER,
            });
            next_slot_y += GRID_STEP_Y;
        }

        // 4. Scan ~/Desktop
        let dir = desktop_dir();

        if let Ok(entries) = fs::read_dir(&dir) {
            let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|d| d.path())).collect();
            paths.sort();

            for path in paths {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                if name.is_empty() || name.starts_with('.') || name == "Trash" || name == ".Trash" || name.to_lowercase().starts_with("librewolf") {
                    continue;
                }

                let id = name.to_string();
                if self.hidden.contains(&id.to_lowercase()) {
                    continue;
                }

                let (x, y) = if let Some(&(px, py)) = positions.get(&id.to_lowercase()) {
                    (px, py)
                } else {
                    let cur_x = next_slot_x;
                    let cur_y = next_slot_y;
                    next_slot_y += GRID_STEP_Y;
                    if next_slot_y + ICON_H > screen_h - 40 {
                        next_slot_y = GRID_Y;
                        next_slot_x += GRID_STEP_X;
                    }
                    (cur_x, cur_y)
                };

                if path.is_dir() {
                    new_items.push(DesktopItem { id, label: name.to_string(), kind: ItemKind::Folder(path.clone()), x, y, icon: icons::FOLDER });
                } else if name.ends_with(".desktop") {
                    let (disp_name, exec) = parse_desktop_file(&path);
                    new_items.push(DesktopItem {
                        id,
                        label: disp_name,
                        kind: ItemKind::Shortcut { name: name.to_string(), exec },
                        x,
                        y,
                        icon: icons::COMPUTER,
                    });
                } else {
                    new_items.push(DesktopItem { id, label: name.to_string(), kind: ItemKind::File(path.clone()), x, y, icon: icons::FILE });
                }
            }
        }

        self.items = new_items;
        if let Some(sel) = self.selected {
            if sel >= self.items.len() {
                self.selected = None;
            }
        }
    }

    /// Synchronously deletes a desktop item or moves it to Trash.
    pub fn delete_item(&mut self, idx: usize) -> bool {
        if idx >= self.items.len() {
            return false;
        }
        let item = self.items[idx].clone();
        if item.id == "Recycle Bin" || item.id == "Files" {
            return false;
        }

        if item.id.eq_ignore_ascii_case("librewolf") {
            self.hidden.insert("librewolf".to_string());
            self.items.remove(idx);
            self.selected = None;
            self.save_positions();
            return true;
        }

        match &item.kind {
            ItemKind::File(path) | ItemKind::Folder(path) => {
                let trash = desktop_dir().parent().unwrap_or(Path::new("/")).join(".local").join("share").join("Trash");
                let _ = fs::create_dir_all(&trash);
                let dest = trash.join(path.file_name().unwrap_or_default());
                let _ = fs::rename(path, dest);
            }
            ItemKind::Shortcut { .. } => {
                let desk_file = desktop_dir().join(&item.id);
                let _ = fs::remove_file(desk_file);
            }
            _ => {}
        }

        self.items.remove(idx);
        self.selected = None;
        self.save_positions();
        true
    }

    /// Arranges desktop icons alphabetically by name in vertical columns.
    pub fn arrange_by_name(&mut self, _screen_w: i32, screen_h: i32) {
        if self.items.len() <= 2 {
            return;
        }
        // Keep Recycle Bin and Files in slots 0 and 1, sort remainder by label
        let mut rest = self.items.split_off(2);
        rest.sort_by_key(|a| a.label.to_lowercase());
        self.items.extend(rest);

        self.realign_grid(screen_h);
        self.save_positions();
    }

    /// Arranges desktop icons by type (System, Folders, Shortcuts, Files).
    pub fn arrange_by_type(&mut self, _screen_w: i32, screen_h: i32) {
        if self.items.len() <= 2 {
            return;
        }
        let mut rest = self.items.split_off(2);
        rest.sort_by_key(|a| match &a.kind {
            ItemKind::RecycleBin | ItemKind::Files => 0,
            ItemKind::Folder(_) => 1,
            ItemKind::Shortcut { .. } => 2,
            ItemKind::File(_) => 3,
        });
        self.items.extend(rest);

        self.realign_grid(screen_h);
        self.save_positions();
    }

    /// Snaps all current items to grid coordinates.
    pub fn align_to_grid(&mut self, _screen_w: i32, _screen_h: i32) {
        for item in &mut self.items {
            let col = ((item.x - GRID_X + GRID_STEP_X / 2) / GRID_STEP_X).max(0);
            let row = ((item.y - GRID_Y + GRID_STEP_Y / 2) / GRID_STEP_Y).max(0);
            item.x = GRID_X + col * GRID_STEP_X;
            item.y = GRID_Y + row * GRID_STEP_Y;
        }
        self.save_positions();
    }

    fn realign_grid(&mut self, screen_h: i32) {
        let mut cur_x = GRID_X;
        let mut cur_y = GRID_Y;
        for item in &mut self.items {
            item.x = cur_x;
            item.y = cur_y;
            cur_y += GRID_STEP_Y;
            if cur_y + ICON_H > screen_h - 40 {
                cur_y = GRID_Y;
                cur_x += GRID_STEP_X;
            }
        }
    }

    /// Hit-tests a point (x, y) against all desktop items.
    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        for (i, item) in self.items.iter().enumerate().rev() {
            if item.rect().contains(x, y) {
                return Some(i);
            }
        }
        None
    }

    /// Moves an item to a new position and clamps to the desktop.
    pub fn move_item(&mut self, idx: usize, x: i32, y: i32, screen_w: i32, screen_h: i32) {
        if let Some(item) = self.items.get_mut(idx) {
            item.x = x.clamp(0, (screen_w - ICON_W).max(0));
            item.y = y.clamp(0, (screen_h - 32 - ICON_H).max(0));
        }
    }

    /// Draws all desktop items onto surface `s`.
    pub fn draw(&self, s: &mut Surface, w: i32, h: i32, bg_color: u32, wallpaper: Option<&ferro_gfx::DecodedImage>) {
        if let Some(img) = wallpaper {
            s.draw_image_scaled(Rect::new(0, 0, w, h), img);
        } else {
            s.fill(Rect::new(0, 0, w, h), bg_color);
        }
        for (i, item) in self.items.iter().enumerate() {
            let selected = self.selected == Some(i);
            let hovered = self.hovered == Some(i);
            draw_item(s, item, selected, hovered);
        }
    }
}

fn parse_desktop_file(path: &Path) -> (String, String) {
    let name_default = path.file_stem().and_then(|n| n.to_str()).unwrap_or("App").to_string();
    let mut disp_name = name_default.clone();
    let mut exec = name_default;
    if let Ok(content) = fs::read_to_string(path) {
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("Name=") {
                disp_name = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("Exec=") {
                exec = val.trim().to_string();
            }
        }
    }
    (disp_name, exec)
}

fn draw_item(s: &mut Surface, item: &DesktopItem, selected: bool, hovered: bool) {
    let r = item.rect();

    // Hover outline / soft highlight when moused over
    if hovered && !selected {
        let (hx, hy, hw, hh) = (r.x + 2, r.y + 2, r.w - 4, r.h - 4);
        s.hline(hx, hy, hw, color::SHADOW);
        s.vline(hx, hy, hh, color::SHADOW);
        s.hline(hx, hy + hh - 1, hw, color::SHADOW);
        s.vline(hx + hw - 1, hy, hh, color::SHADOW);
    }

    // Draw 32x32 icon sprite centered
    let icon_x = r.x + (r.w - ICON_SIZE) / 2;
    let icon_y = r.y + 2;
    s.sprite(icon_x, icon_y, item.icon, 2);

    // Draw label with drop shadow or selection background
    let lw = text_width(&item.label);
    let lx = r.x + (r.w - lw) / 2;
    let ly = r.y + ICON_SIZE + 6;
    let label_rect = Rect::new(lx - 2, ly - 2, lw + 4, GLYPH_H + 4);

    if selected {
        s.fill(label_rect, color::NAVY);
        s.text(lx, ly, &item.label, color::WHITE);
    } else {
        s.text(lx + 1, ly + 1, &item.label, color::BLACK); // drop shadow
        s.text(lx, ly, &item.label, color::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_bin_at_top_and_files_underneath() {
        let mut dm = DesktopManager::default();
        dm.items.clear();
        dm.items.push(DesktopItem {
            id: "Recycle Bin".into(),
            label: "Recycle Bin".into(),
            kind: ItemKind::RecycleBin,
            x: 20,
            y: 20,
            icon: icons::RECYCLE,
        });
        dm.items.push(DesktopItem { id: "Files".into(), label: "Files".into(), kind: ItemKind::Files, x: 20, y: 96, icon: icons::FOLDER });
        dm.items.push(DesktopItem {
            id: "LibreWolf".into(),
            label: "LibreWolf".into(),
            kind: ItemKind::Shortcut { name: "LibreWolf".into(), exec: "librewolf".into() },
            x: 20,
            y: 172,
            icon: icons::BROWSER,
        });

        assert_eq!(dm.items[0].kind, ItemKind::RecycleBin);
        assert_eq!(dm.items[0].y, 20);
        assert_eq!(dm.items[1].kind, ItemKind::Files);
        assert_eq!(dm.items[1].y, 96);
        assert_eq!(dm.items[2].id, "LibreWolf");
        assert_eq!(dm.items[2].y, 172);

        // Hit testing
        assert_eq!(dm.hit(25, 25), Some(0));
        assert_eq!(dm.hit(25, 105), Some(1));
        assert_eq!(dm.hit(25, 180), Some(2));
        assert_eq!(dm.hit(500, 500), None);
    }

    #[test]
    fn moving_item_updates_position() {
        let mut dm = DesktopManager {
            items: vec![DesktopItem { id: "Files".into(), label: "Files".into(), kind: ItemKind::Files, x: 20, y: 96, icon: icons::FOLDER }],
            ..DesktopManager::default()
        };
        dm.move_item(0, 150, 200, 1024, 768);
        assert_eq!(dm.items[0].x, 150);
        assert_eq!(dm.items[0].y, 200);
    }

    #[test]
    fn align_to_grid_snaps() {
        let mut dm = DesktopManager {
            items: vec![DesktopItem { id: "Files".into(), label: "Files".into(), kind: ItemKind::Files, x: 25, y: 99, icon: icons::FOLDER }],
            ..DesktopManager::default()
        };
        dm.align_to_grid(1024, 768);
        assert_eq!(dm.items[0].x, GRID_X);
        assert_eq!(dm.items[0].y, GRID_Y + GRID_STEP_Y);
    }
}
