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

    pub fn visual_rect(&self) -> Rect {
        let tw = text_width(&self.label);
        let tip_w = tw + 16;
        let tip_h = GLYPH_H + 8;
        let tip_x_center = self.x + (ICON_W - tip_w) / 2;
        let min_x = (self.x - 8).min(tip_x_center - 4).min(2);
        let max_x = (self.x + ICON_W + 8).max(tip_x_center + tip_w + 4).max(self.x + tip_w);
        let min_y = (self.y - 6).max(0);
        let max_y = self.y + ICON_H + tip_h + 12;
        Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
    }

    pub fn exec_command(&self) -> String {
        match &self.kind {
            ItemKind::RecycleBin => "sh -c 'mkdir -p \"$HOME/.local/share/Trash\" && ferro-files \"$HOME/.local/share/Trash\"'".into(),
            ItemKind::Files => "ferro-files \"My Computer\"".into(),
            ItemKind::Folder(path) => format!("ferro-files {}", ferro_associations::shell_quote(&path.to_string_lossy())),
            ItemKind::File(path) => ferro_associations::resolve_command_for_path(path),
            ItemKind::Shortcut { exec, .. } => {
                let e = exec.trim();
                let lower = e.to_lowercase();
                if lower == "ferro-cmd"
                    || lower == "cmd"
                    || lower == "terminal"
                    || lower == "x-terminal-emulator"
                    || (lower.contains("ferro-cmd") && !lower.contains("xterm") && !lower.contains("lxterminal"))
                {
                    "sh -c 'ferro-terminal || lxterminal'".into()
                } else if lower.contains("librewolf") || lower.contains("firefox") || lower == "browser" {
                    "sh -c 'librewolf || firefox-esr || firefox || x-www-browser'".into()
                } else {
                    exec.clone()
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineRename {
    pub item_idx: usize,
    pub edit: ferro_ui::TextEdit,
    /// The mouse held down in it: selecting as it moves.
    pub dragging: bool,
    /// When it was last clicked (a second click soon after picks a word).
    pub last_click: u32,
}

pub struct DesktopManager {
    pub items: Vec<DesktopItem>,
    pub selected: Option<usize>,
    pub selected_items: HashSet<usize>,
    pub hovered: Option<usize>,
    pub hidden: HashSet<String>,
    pub auto_arrange: bool,
    pub align_to_grid: bool,
    pub rubberband: Option<Rect>,
    pub renaming: Option<InlineRename>,
    pub thumbnails: HashMap<String, ferro_gfx::DecodedImage>,
    /// Thumbnails being made aside (pictures and videos), and what's been
    /// asked for, so a refresh doesn't ask again.
    thumbs_rx: Option<std::sync::mpsc::Receiver<(String, ferro_gfx::DecodedImage)>>,
    thumbs_asked: HashSet<String>,
    /// Files dropped here from the file manager, by name: where they go
    /// once they've arrived, and since when they've been waited for.
    pending_place: Vec<(String, i32, i32, std::time::Instant)>,
}

impl DesktopManager {
    /// Files dropped from the file manager with the pointer at `(x, y)`:
    /// the first's icon goes there, the others below it.
    pub fn place_at(&mut self, names: &[String], x: i32, y: i32) {
        let now = std::time::Instant::now();
        for (k, name) in names.iter().enumerate() {
            self.pending_place.push((name.clone(), (x - 36).max(0), (y - 20 + k as i32 * 76).max(0), now));
        }
    }

    /// If dropped files are awaited: looks again, puts those that have
    /// arrived where they were dropped. Whether any were.
    pub fn refresh_pending(&mut self, screen_w: i32, screen_h: i32) -> bool {
        self.pending_place.retain(|p| p.3.elapsed() < std::time::Duration::from_secs(20));
        if self.pending_place.is_empty() {
            return false;
        }
        self.refresh(screen_w, screen_h);
        let mut placed = false;
        let pending = std::mem::take(&mut self.pending_place);
        for p in pending {
            match self.items.iter_mut().find(|it| it.id == p.0) {
                Some(it) => {
                    (it.x, it.y) = (p.1, p.2);
                    placed = true;
                }
                None => self.pending_place.push(p),
            }
        }
        if placed {
            self.save_positions();
        }
        placed
    }

    /// Takes the thumbnails made since last time; whether there were any.
    pub fn take_thumbnails(&mut self) -> bool {
        let Some(rx) = &self.thumbs_rx else { return false };
        let mut got = false;
        loop {
            match rx.try_recv() {
                Ok((id, t)) => {
                    self.thumbnails.insert(id, t);
                    got = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.thumbs_rx = None;
                    break;
                }
            }
        }
        got
    }
}

pub fn count_trash_items() -> usize {
    ferro_associations::trash::count()
}

/// What the Recycle Bin's icon says: how many things are in it.
pub fn bin_label(count: usize) -> String {
    match count {
        0 => "Recycle Bin".into(),
        n => format!("Recycle Bin ({n})"),
    }
}

fn config_path() -> PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir().join("ferro_test_desktop.conf")
    }
    #[cfg(not(test))]
    {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".config").join("ferro").join("desktop.conf")
        } else {
            PathBuf::from("/etc/ferro/desktop.conf")
        }
    }
}

pub fn desktop_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join("Desktop")
    } else {
        PathBuf::from("/home/ferro/Desktop")
    }
}

pub fn find_free_slot_in_items(items: &[DesktopItem], target_x: i32, target_y: i32, screen_h: i32) -> (i32, i32) {
    let max_rows = ((screen_h - 40 - GRID_Y) / GRID_STEP_Y).max(1);
    let target_col = ((target_x - GRID_X + GRID_STEP_X / 2) / GRID_STEP_X).max(0);
    let target_row = ((target_y - GRID_Y + GRID_STEP_Y / 2) / GRID_STEP_Y).max(0);

    let is_occupied = |x: i32, y: i32| -> bool {
        let test_rect = Rect::new(x, y, ICON_W, ICON_H);
        items.iter().any(|it| it.rect().overlaps(&test_rect))
    };

    let sx = GRID_X + target_col * GRID_STEP_X;
    let sy = GRID_Y + target_row * GRID_STEP_Y;
    if !is_occupied(sx, sy) {
        return (sx, sy);
    }

    let mut best: Option<(i32, i32)> = None;
    let mut min_dist_sq = i64::MAX;

    for col in 0..40 {
        for row in 0..max_rows {
            let gx = GRID_X + col * GRID_STEP_X;
            let gy = GRID_Y + row * GRID_STEP_Y;
            if !is_occupied(gx, gy) {
                let dx = i64::from(gx - target_x);
                let dy = i64::from(gy - target_y);
                let dist_sq = dx * dx + dy * dy;
                if dist_sq < min_dist_sq {
                    min_dist_sq = dist_sq;
                    best = Some((gx, gy));
                }
            }
        }
        if let Some(pos) = best {
            if (col - target_col).abs() > 3 {
                return pos;
            }
        }
    }
    best.unwrap_or((sx, sy))
}

impl Default for DesktopManager {
    fn default() -> Self {
        Self::new(1024, 768)
    }
}

#[derive(Debug, Default)]
pub struct DesktopSavedConfig {
    pub positions: HashMap<String, (i32, i32)>,
    pub hidden: HashSet<String>,
    pub auto_arrange: bool,
    pub align_to_grid: bool,
}

impl DesktopManager {
    pub fn new(screen_w: i32, screen_h: i32) -> Self {
        let mut dm = Self {
            items: Vec::new(),
            selected: None,
            selected_items: HashSet::new(),
            hovered: None,
            hidden: HashSet::new(),
            auto_arrange: false,
            align_to_grid: true,
            rubberband: None,
            renaming: None,
            thumbnails: HashMap::new(),
            thumbs_rx: None,
            thumbs_asked: HashSet::new(),
            pending_place: Vec::new(),
        };
        dm.refresh(screen_w, screen_h);
        dm
    }

    #[allow(dead_code)]
    pub fn find_free_slot(&self, target_x: i32, target_y: i32, exclude_idx: Option<usize>, screen_h: i32) -> (i32, i32) {
        let max_rows = ((screen_h - 40 - GRID_Y) / GRID_STEP_Y).max(1);
        let target_col = ((target_x - GRID_X + GRID_STEP_X / 2) / GRID_STEP_X).max(0);
        let target_row = ((target_y - GRID_Y + GRID_STEP_Y / 2) / GRID_STEP_Y).max(0);

        let is_occupied = |x: i32, y: i32| -> bool {
            let test_rect = Rect::new(x, y, ICON_W, ICON_H);
            for (i, it) in self.items.iter().enumerate() {
                if Some(i) == exclude_idx {
                    continue;
                }
                if it.rect().overlaps(&test_rect) {
                    return true;
                }
            }
            false
        };

        let sx = GRID_X + target_col * GRID_STEP_X;
        let sy = GRID_Y + target_row * GRID_STEP_Y;
        if !is_occupied(sx, sy) {
            return (sx, sy);
        }

        let mut best: Option<(i32, i32)> = None;
        let mut min_dist_sq = i64::MAX;

        for col in 0..40 {
            for row in 0..max_rows {
                let gx = GRID_X + col * GRID_STEP_X;
                let gy = GRID_Y + row * GRID_STEP_Y;
                if !is_occupied(gx, gy) {
                    let dx = i64::from(gx - target_x);
                    let dy = i64::from(gy - target_y);
                    let dist_sq = dx * dx + dy * dy;
                    if dist_sq < min_dist_sq {
                        min_dist_sq = dist_sq;
                        best = Some((gx, gy));
                    }
                }
            }
            if let Some(pos) = best {
                if (col - target_col).abs() > 3 {
                    return pos;
                }
            }
        }
        best.unwrap_or((sx, sy))
    }

    /// Loads saved icon positions, hidden item list, and layout settings from `desktop.conf`.
    pub fn load_positions() -> DesktopSavedConfig {
        let path = config_path();
        let mut map = HashMap::new();
        let mut hidden = HashSet::new();
        let mut auto_arrange = false;
        let mut align_to_grid = true;
        if let Ok(content) = fs::read_to_string(&path) {
            for line in content.lines().map(str::trim) {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let key = k.trim().to_lowercase();
                    let val = v.trim();
                    if key == "auto_arrange" {
                        auto_arrange = val.eq_ignore_ascii_case("true") || val == "1";
                        continue;
                    }
                    if key == "align_to_grid" {
                        align_to_grid = val.eq_ignore_ascii_case("true") || val == "1";
                        continue;
                    }
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
        DesktopSavedConfig { positions: map, hidden, auto_arrange, align_to_grid }
    }

    /// Saves icon positions, hidden item list, and layout settings to `desktop.conf`.
    pub fn save_positions(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut lines = vec!["# FerroOS Desktop Settings and Icon Positions".to_string()];
        lines.push(format!("auto_arrange = {}", self.auto_arrange));
        lines.push(format!("align_to_grid = {}", self.align_to_grid));
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
        let config = Self::load_positions();
        let mut positions = config.positions;
        for item in &self.items {
            positions.entry(item.id.to_lowercase()).or_insert((item.x, item.y));
        }
        self.hidden = config.hidden;
        self.auto_arrange = config.auto_arrange;
        self.align_to_grid = config.align_to_grid;
        let mut new_items = Vec::new();
        let mut thumb_jobs: Vec<(String, PathBuf)> = Vec::new();

        // 1. Recycle Bin at top: (20, 20)
        let trash_count = count_trash_items();
        let bin_label = bin_label(trash_count);
        let (bin_x, bin_y) = positions.get("recycle bin").copied().unwrap_or((GRID_X, GRID_Y));
        new_items.push(DesktopItem {
            id: "Recycle Bin".into(),
            label: bin_label,
            kind: ItemKind::RecycleBin,
            x: bin_x,
            y: bin_y,
            icon: icons::RECYCLE,
        });

        // 2. My Computer underneath: (20, 96)
        let (files_x, files_y) = positions.get("my computer").or_else(|| positions.get("files")).copied().unwrap_or((GRID_X, GRID_Y + GRID_STEP_Y));
        new_items.push(DesktopItem {
            id: "My Computer".into(),
            label: "My Computer".into(),
            kind: ItemKind::Files,
            x: files_x,
            y: files_y,
            icon: icons::COMPUTER,
        });

        // 3. LibreWolf underneath: (20, 172) (unless hidden)
        let mut next_slot_y = GRID_Y + 2 * GRID_STEP_Y;
        let mut next_slot_x = GRID_X;

        if !self.hidden.contains("librewolf") {
            let (wolf_x, wolf_y) = positions.get("librewolf").copied().unwrap_or((GRID_X, next_slot_y));
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

        // 4. Command Prompt underneath: (20, 248) (unless hidden)
        if !self.hidden.contains("command prompt") && !self.hidden.contains("terminal") {
            let (term_x, term_y) = positions.get("command prompt").or_else(|| positions.get("terminal")).copied().unwrap_or((GRID_X, next_slot_y));
            new_items.push(DesktopItem {
                id: "Command Prompt".into(),
                label: "Command Prompt".into(),
                kind: ItemKind::Shortcut { name: "Command Prompt".into(), exec: "sh -c 'ferro-terminal || lxterminal'".into() },
                x: term_x,
                y: term_y,
                icon: icons::TERMINAL,
            });
            next_slot_y += GRID_STEP_Y;
        }

        // 5. Scan ~/Desktop
        let dir = desktop_dir();

        if let Ok(entries) = fs::read_dir(&dir) {
            let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|d| d.path())).collect();
            paths.sort();

            for path in paths {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                let lower = name.to_lowercase();
                if name.is_empty()
                    || name.starts_with('.')
                    || name == "Trash"
                    || name == ".Trash"
                    || lower.starts_with("librewolf")
                    || lower.starts_with("terminal")
                    || lower.starts_with("command prompt")
                {
                    continue;
                }

                let id = name.to_string();
                if self.hidden.contains(&id.to_lowercase()) {
                    continue;
                }

                let (x, y) = if let Some(&(px, py)) = positions.get(&id.to_lowercase()) {
                    let rect = Rect::new(px, py, ICON_W, ICON_H);
                    if new_items.iter().any(|it| it.rect().overlaps(&rect)) {
                        find_free_slot_in_items(&new_items, px, py, screen_h)
                    } else {
                        (px, py)
                    }
                } else {
                    let (cur_x, cur_y) = find_free_slot_in_items(&new_items, next_slot_x, next_slot_y, screen_h);
                    next_slot_y += GRID_STEP_Y;
                    if next_slot_y + ICON_H > screen_h - 40 {
                        next_slot_y = GRID_Y;
                        next_slot_x += GRID_STEP_X;
                    }
                    (cur_x, cur_y)
                };

                if path.is_dir() {
                    new_items.push(DesktopItem { id, label: name.to_string(), kind: ItemKind::Folder(path.clone()), x, y, icon: icons::FOLDER });
                } else if name.ends_with(".desktop") && ferro_associations::trusted_launcher(&path) {
                    let (disp_name, exec) = parse_desktop_file(&path);
                    new_items.push(DesktopItem {
                        id,
                        label: disp_name,
                        kind: ItemKind::Shortcut { name: name.to_string(), exec },
                        x,
                        y,
                        icon: icons::COMPUTER,
                    });
                } else if name.to_lowercase().ends_with(".appimage") {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Ok(metadata) = fs::metadata(&path) {
                            let mut perms = metadata.permissions();
                            let mode = perms.mode();
                            if mode & 0o111 == 0 {
                                perms.set_mode(mode | 0o755);
                                let _ = fs::set_permissions(&path, perms);
                            }
                        }
                    }
                    new_items.push(DesktopItem { id, label: name.to_string(), kind: ItemKind::File(path.clone()), x, y, icon: icons::COMPUTER });
                } else {
                    if ferro_associations::thumbs::has_thumbnail(&path) && self.thumbs_asked.insert(id.clone()) {
                        thumb_jobs.push((id.clone(), path.clone()));
                    }
                    new_items.push(DesktopItem { id, label: name.to_string(), kind: ItemKind::File(path.clone()), x, y, icon: icons::FILE });
                }
            }
        }

        self.items = new_items;
        if !thumb_jobs.is_empty() {
            // Made aside: a big picture or a video takes a moment, and the
            // desktop mustn't wait for it. They arrive through take_thumbnails.
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for (id, path) in thumb_jobs {
                    if let Some(t) = ferro_associations::thumbs::thumbnail(&path, 32) {
                        if tx.send((id, t)).is_err() {
                            return;
                        }
                    }
                }
            });
            self.thumbs_rx = Some(rx);
        }
        if self.auto_arrange {
            self.realign_grid(screen_h);
        }
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

        if item.id.eq_ignore_ascii_case("command prompt") || item.id.eq_ignore_ascii_case("terminal") {
            self.hidden.insert("command prompt".to_string());
            self.hidden.insert("terminal".to_string());
            self.items.remove(idx);
            self.selected = None;
            self.save_positions();
            return true;
        }

        match &item.kind {
            // On a removable drive (one mounted in the Desktop folder): it
            // can't go to the Recycle Bin, so the file manager asks whether
            // to delete it for good, as it does for its own Delete. The
            // icon goes once it's gone.
            ItemKind::File(path) | ItemKind::Folder(path) if !ferro_associations::trash::can_take(path) => {
                crate::launch(&format!("ferro-files --delete {}", ferro_associations::shell_quote(&path.to_string_lossy())));
                return false;
            }
            ItemKind::File(path) | ItemKind::Folder(path) => {
                // Into the Recycle Bin, never over something already there.
                if ferro_associations::trash::put(path).is_err() {
                    return false;
                }
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

    pub fn realign_grid(&mut self, screen_h: i32) {
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
        self.hit_except(x, y, &[])
    }

    /// What's under (x, y) leaving out `skip`: where icons being dragged
    /// are dropped, they themselves are under the pointer, on top of what
    /// they're dropped on.
    pub fn hit_except(&self, x: i32, y: i32, skip: &[usize]) -> Option<usize> {
        self.items.iter().enumerate().rev().find(|(i, item)| !skip.contains(i) && item.rect().contains(x, y)).map(|(i, _)| i)
    }

    /// Where the Recycle Bin's icon is, and its index.
    pub fn bin(&self) -> Option<(usize, Rect)> {
        self.items.iter().position(|i| i.kind == ItemKind::RecycleBin).map(|i| (i, self.items[i].rect()))
    }

    /// Moves an item to a new position and clamps to the desktop.
    pub fn move_item(&mut self, idx: usize, x: i32, y: i32, screen_w: i32, screen_h: i32) {
        if let Some(item) = self.items.get_mut(idx) {
            item.x = x.clamp(0, (screen_w - ICON_W).max(0));
            item.y = y.clamp(0, (screen_h - 32 - ICON_H).max(0));
        }
    }

    /// Called on mouse release after moving an item.
    #[allow(dead_code)]
    pub fn finish_move_item(&mut self, idx: usize, screen_h: i32) {
        if self.auto_arrange {
            self.realign_grid(screen_h);
        } else if self.align_to_grid {
            if let Some(item) = self.items.get(idx) {
                let (target_x, target_y) = (item.x, item.y);
                let (free_x, free_y) = self.find_free_slot(target_x, target_y, Some(idx), screen_h);
                if let Some(it) = self.items.get_mut(idx) {
                    it.x = free_x;
                    it.y = free_y;
                }
            }
        } else if let Some(item) = self.items.get(idx) {
            let test_rect = item.rect();
            let overlaps = self.items.iter().enumerate().any(|(i, other)| i != idx && other.rect().overlaps(&test_rect));
            if overlaps {
                let (target_x, target_y) = (item.x, item.y);
                let (free_x, free_y) = self.find_free_slot(target_x, target_y, Some(idx), screen_h);
                if let Some(it) = self.items.get_mut(idx) {
                    it.x = free_x;
                    it.y = free_y;
                }
            }
        }
        self.save_positions();
    }

    /// Called on mouse release after moving a group of items together.
    pub fn finish_move_group(&mut self, main_idx: usize, others: &[(usize, i32, i32)], screen_w: i32, screen_h: i32) {
        if self.auto_arrange {
            self.realign_grid(screen_h);
            self.save_positions();
            return;
        }

        if self.align_to_grid {
            // Align anchor item to grid; shift all others by the exact same offset to preserve cluster layout
            if let Some(main_item) = self.items.get(main_idx) {
                let target_col = ((main_item.x - GRID_X + GRID_STEP_X / 2) / GRID_STEP_X).max(0);
                let target_row = ((main_item.y - GRID_Y + GRID_STEP_Y / 2) / GRID_STEP_Y).max(0);
                let base_grid_x = GRID_X + target_col * GRID_STEP_X;
                let base_grid_y = GRID_Y + target_row * GRID_STEP_Y;
                let delta_x = base_grid_x - main_item.x;
                let delta_y = base_grid_y - main_item.y;

                if let Some(it) = self.items.get_mut(main_idx) {
                    it.x = base_grid_x.clamp(0, (screen_w - ICON_W).max(0));
                    it.y = base_grid_y.clamp(0, (screen_h - 32 - ICON_H).max(0));
                }
                for &(other_idx, _, _) in others {
                    if let Some(other_item) = self.items.get_mut(other_idx) {
                        other_item.x = (other_item.x + delta_x).clamp(0, (screen_w - ICON_W).max(0));
                        other_item.y = (other_item.y + delta_y).clamp(0, (screen_h - 32 - ICON_H).max(0));
                    }
                }
            }
        } else {
            // Clamp group items to screen without scattering
            if let Some(main_item) = self.items.get_mut(main_idx) {
                main_item.x = main_item.x.clamp(0, (screen_w - ICON_W).max(0));
                main_item.y = main_item.y.clamp(0, (screen_h - 32 - ICON_H).max(0));
            }
            for &(other_idx, _, _) in others {
                if let Some(other_item) = self.items.get_mut(other_idx) {
                    other_item.x = other_item.x.clamp(0, (screen_w - ICON_W).max(0));
                    other_item.y = other_item.y.clamp(0, (screen_h - 32 - ICON_H).max(0));
                }
            }
        }
        self.save_positions();
    }

    pub fn is_selected(&self, idx: usize) -> bool {
        self.selected == Some(idx) || self.selected_items.contains(&idx)
    }

    pub fn set_single_selected(&mut self, idx: Option<usize>) {
        self.selected = idx;
        self.selected_items.clear();
        if let Some(i) = idx {
            self.selected_items.insert(i);
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.selected_items.clear();
    }

    pub fn select_all(&mut self) {
        self.selected_items.clear();
        for i in 0..self.items.len() {
            self.selected_items.insert(i);
        }
        self.selected = self.items.first().map(|_| 0);
    }

    pub fn update_rubberband(&mut self, rb: Rect) {
        self.rubberband = Some(rb);
        self.selected_items.clear();
        for (i, item) in self.items.iter().enumerate() {
            if item.rect().overlaps(&rb) {
                self.selected_items.insert(i);
            }
        }
        self.selected = self.selected_items.iter().next().copied();
    }

    pub fn delete_selected(&mut self) -> bool {
        if self.selected_items.is_empty() {
            if let Some(s) = self.selected {
                return self.delete_item(s);
            }
            return false;
        }
        let mut indices: Vec<usize> = self.selected_items.iter().copied().collect();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        let mut any_deleted = false;
        for idx in indices {
            if self.delete_item(idx) {
                any_deleted = true;
            }
        }
        self.clear_selection();
        any_deleted
    }

    pub fn start_rename(&mut self, idx: usize) {
        if let Some(it) = self.items.get(idx) {
            let initial_text = match &it.kind {
                ItemKind::Folder(p) | ItemKind::File(p) => {
                    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| it.label.clone())
                }
                ItemKind::Shortcut { .. } => it.label.clone(),
                _ => return,
            };
            let stem_len = match &it.kind {
                ItemKind::Folder(_) => initial_text.len(),
                ItemKind::File(_) => Path::new(&initial_text).file_stem().and_then(|s| s.to_str()).map(|s| s.len()).unwrap_or(initial_text.len()),
                _ => initial_text.len(),
            };
            self.renaming = Some(InlineRename {
                item_idx: idx,
                edit: ferro_ui::TextEdit::with_selection(&initial_text, 0, stem_len),
                dragging: false,
                last_click: 0,
            });
        }
    }

    pub fn commit_rename(&mut self) -> Option<Rect> {
        let ren = self.renaming.take()?;
        let item = self.items.get_mut(ren.item_idx)?;
        let old_rect = item.visual_rect();
        let old_id = item.id.clone();
        let new_name = ren.edit.text().trim();
        if !new_name.is_empty() && new_name != item.label {
            let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/home/ferro"));
            let mut renamed_ok = false;
            let mut new_path = None;
            match &item.kind {
                ItemKind::Folder(p) | ItemKind::File(p) => {
                    if let Some(parent) = p.parent() {
                        let target = parent.join(new_name);
                        if fs::rename(p, &target).is_ok() {
                            new_path = Some(target);
                            renamed_ok = true;
                        }
                    }
                }
                ItemKind::Shortcut { .. } => {
                    let shortcut_path = home.join("Desktop").join(&item.id);
                    if shortcut_path.exists() {
                        if let Ok(content) = fs::read_to_string(&shortcut_path) {
                            let mut new_lines = Vec::new();
                            for line in content.lines() {
                                if line.starts_with("Name=") {
                                    new_lines.push(format!("Name={new_name}"));
                                } else {
                                    new_lines.push(line.to_string());
                                }
                            }
                            let _ = fs::write(&shortcut_path, new_lines.join("\n"));
                            renamed_ok = true;
                        }
                    }
                }
                _ => {}
            }
            if renamed_ok {
                item.id = new_name.to_string();
                item.label = new_name.to_string();
                if let Some(target) = new_path {
                    item.kind = match &item.kind {
                        ItemKind::Folder(_) => ItemKind::Folder(target),
                        ItemKind::File(_) => ItemKind::File(target),
                        other => other.clone(),
                    };
                }
                let mut config = Self::load_positions();
                config.positions.remove(&old_id.to_lowercase());
                config.positions.insert(new_name.to_lowercase(), (item.x, item.y));
                self.save_positions();
            }
        }
        Some(old_rect)
    }

    pub fn cancel_rename(&mut self) -> Option<Rect> {
        let ren = self.renaming.take()?;
        self.items.get(ren.item_idx).map(|it| it.visual_rect())
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/home/ferro"));
        let mut paths = Vec::new();
        let set = if !self.selected_items.is_empty() {
            self.selected_items.clone()
        } else if let Some(s) = self.selected {
            let mut h = HashSet::new();
            h.insert(s);
            h
        } else {
            HashSet::new()
        };
        for idx in set {
            if let Some(item) = self.items.get(idx) {
                match &item.kind {
                    ItemKind::Folder(p) | ItemKind::File(p) => paths.push(p.clone()),
                    ItemKind::Shortcut { .. } => paths.push(home.join("Desktop").join(&item.id)),
                    _ => {}
                }
            }
        }
        paths
    }

    pub fn copy_selected(&self) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            write_clipboard_files(&paths, false);
        }
    }

    pub fn cut_selected(&self) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            write_clipboard_files(&paths, true);
        }
    }

    /// Draws all desktop items onto surface `s`.
    #[allow(dead_code)]
    pub fn draw(&self, s: &mut Surface, w: i32, h: i32, bg_color: u32, wallpaper: Option<&ferro_gfx::DecodedImage>) {
        if let Some(img) = wallpaper {
            s.draw_image_scaled(Rect::new(0, 0, w, h), img);
        } else {
            s.fill(Rect::new(0, 0, w, h), bg_color);
        }
        for (i, item) in self.items.iter().enumerate() {
            let selected = self.is_selected(i);
            let hovered = self.hovered == Some(i);
            let ren = self.renaming.as_ref().filter(|r| r.item_idx == i);
            draw_item(s, item, selected, hovered, ren, self.thumbnails.get(&item.id));
        }
        if let Some(rb) = self.rubberband {
            focus_rect(s, rb);
        }
    }
}

pub fn focus_rect(s: &mut Surface, r: Rect) {
    if r.w <= 0 || r.h <= 0 {
        return;
    }
    for x in 0..r.w {
        let col = if (r.x + x) % 2 == 0 { color::WHITE } else { color::BLACK };
        s.put(r.x + x, r.y, col);
        s.put(r.x + x, r.bottom() - 1, col);
    }
    for y in 0..r.h {
        let col = if (r.y + y) % 2 == 0 { color::WHITE } else { color::BLACK };
        s.put(r.x, r.y + y, col);
        s.put(r.right() - 1, r.y + y, col);
    }
}

pub fn clipboard_file() -> PathBuf {
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime).join("ferro-files.clipboard")
    } else {
        std::env::temp_dir().join(format!("ferro-files-clipboard-{}", unsafe { libc::getuid() }))
    }
}

pub fn read_clipboard_files() -> Option<(Vec<PathBuf>, bool)> {
    let content = fs::read_to_string(clipboard_file()).ok()?;
    let mut lines = content.lines().map(str::trim).filter(|l| !l.is_empty());
    let mode = lines.next()?;
    let cut = mode.eq_ignore_ascii_case("CUT");
    let mut paths = Vec::new();
    for line in lines {
        let p = PathBuf::from(line);
        if p.exists() {
            paths.push(p);
        }
    }
    if paths.is_empty() {
        None
    } else {
        Some((paths, cut))
    }
}

pub fn write_clipboard_files(paths: &[PathBuf], cut: bool) {
    let mode = if cut { "CUT" } else { "COPY" };
    let mut out = format!("{mode}\n");
    for p in paths {
        out.push_str(&p.to_string_lossy());
        out.push('\n');
    }
    let _ = fs::write(clipboard_file(), out);
}

pub fn has_clipboard() -> bool {
    read_clipboard_files().is_some()
}

pub fn paste_clipboard_to_desktop() -> Option<Vec<PathBuf>> {
    let (paths, cut) = read_clipboard_files()?;
    let desk = desktop_dir();
    let mut created = Vec::new();
    for src in paths {
        let name = src.file_name()?;
        let mut target = desk.join(name);
        if target.exists() && (!cut || target != src) {
            let stem = src.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
            let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let mut i = 1;
            while target.exists() {
                target = desk.join(format!("{stem} ({i}){ext}"));
                i += 1;
            }
        }
        if cut {
            let _ = fs::rename(&src, &target);
        } else if src.is_dir() {
            let _ = copy_dir_all(&src, &target);
        } else {
            let _ = fs::copy(&src, &target);
        }
        created.push(target);
    }
    if cut {
        let _ = fs::remove_file(clipboard_file());
    }
    Some(created)
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
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

pub fn wrap_label(label: &str, max_chars_per_line: usize) -> Vec<String> {
    let words: Vec<&str> = label.split_whitespace().collect();
    if words.is_empty() {
        return vec![label.to_string()];
    }
    if words.len() == 1 {
        let w = words[0];
        if w.chars().count() > max_chars_per_line {
            return vec![ferro_gfx::ellipsize(w, max_chars_per_line)];
        }
        return vec![w.to_string()];
    }

    let mut line1 = String::new();
    let mut word_idx = 0;
    while word_idx < words.len() {
        let w = words[word_idx];
        let test = if line1.is_empty() { w.to_string() } else { format!("{line1} {w}") };
        if test.chars().count() <= max_chars_per_line {
            line1 = test;
            word_idx += 1;
        } else {
            break;
        }
    }
    if line1.is_empty() {
        line1 = ferro_gfx::ellipsize(words[0], max_chars_per_line);
        word_idx += 1;
    }

    if word_idx >= words.len() {
        return vec![line1];
    }

    let mut line2 = String::new();
    while word_idx < words.len() {
        let w = words[word_idx];
        let test = if line2.is_empty() { w.to_string() } else { format!("{line2} {w}") };
        if test.chars().count() <= max_chars_per_line {
            line2 = test;
            word_idx += 1;
        } else {
            let remaining = if line2.is_empty() { w.to_string() } else { format!("{line2} {w}") };
            line2 = ferro_gfx::ellipsize(&remaining, max_chars_per_line);
            break;
        }
    }
    if line2.is_empty() && word_idx < words.len() {
        line2 = ferro_gfx::ellipsize(words[word_idx], max_chars_per_line);
    }

    vec![line1, line2]
}

/// Makes a new folder (or an empty text document) on the desktop, named as
/// Windows names them: "New Folder", "New Folder (2)"... Its name, if made.
pub fn make_new(folder: bool) -> Option<String> {
    let dir = desktop_dir();
    let name_for = |n: u32| match (folder, n) {
        (true, 1) => "New Folder".to_string(),
        (true, n) => format!("New Folder ({n})"),
        (false, 1) => "New Text Document.txt".to_string(),
        (false, n) => format!("New Text Document ({n}).txt"),
    };
    let name = (1..1000).map(name_for).find(|n| !dir.join(n).exists())?;
    let path = dir.join(&name);
    let made = if folder { fs::create_dir_all(&path) } else { fs::write(&path, "") };
    made.ok().map(|()| name)
}

/// Where an item's new name is typed while it's renamed (`text` so far).
pub fn rename_rect(item: &DesktopItem, text: &str, screen_w: i32) -> Rect {
    let r = item.rect();
    let box_w = (text_width(text) + 16).clamp(64, 160);
    let box_x = (r.x + (r.w - box_w) / 2).clamp(2, (screen_w - box_w - 2).max(2));
    Rect::new(box_x, r.y + ICON_SIZE + 4, box_w, GLYPH_H + 6)
}

pub fn draw_item(
    s: &mut Surface,
    item: &DesktopItem,
    selected: bool,
    hovered: bool,
    renaming: Option<&InlineRename>,
    thumbnail: Option<&ferro_gfx::DecodedImage>,
) {
    let r = item.rect();

    // Hover outline / 3D highlight when moused over
    if hovered && renaming.is_none() {
        let hr = Rect::new(r.x - 2, r.y - 2, r.w + 4, r.h + 4);
        s.bevel(hr, if selected { ferro_gfx::Bevel::Sunken } else { ferro_gfx::Bevel::Raised });
    }

    // Draw 32x32 icon sprite centered or in-memory thumbnail
    let icon_x = r.x + (r.w - ICON_SIZE) / 2;
    let icon_y = r.y + 2;
    if let Some(thumb) = thumbnail {
        s.draw_image_scaled(Rect::new(icon_x, icon_y, ICON_SIZE, ICON_SIZE), thumb);
        s.bevel(Rect::new(icon_x - 1, icon_y - 1, ICON_SIZE + 2, ICON_SIZE + 2), ferro_gfx::Bevel::Sunken);
    } else {
        s.sprite(icon_x, icon_y, item.icon, 2);
    }

    if let Some(ren) = renaming {
        let r = rename_rect(item, ren.edit.text(), s.width as i32);
        s.fill(r, color::BLACK);
        ren.edit.draw(s, r.inset(1));
        return;
    }

    // Format label into up to 2 wrapped lines (max 11 chars each so 66px <= 76px)
    let lines = wrap_label(&item.label, 11);
    let mut cur_y = r.y + ICON_SIZE + 4;
    for line in &lines {
        let lw = text_width(line);
        let lx = r.x + (r.w - lw) / 2;
        let line_rect = Rect::new(lx - 2, cur_y - 1, lw + 4, GLYPH_H + 2);
        if selected {
            s.fill(line_rect, color::NAVY);
            s.text(lx, cur_y, line, color::WHITE);
        } else {
            s.text(lx + 1, cur_y + 1, line, color::BLACK); // drop shadow
            s.text(lx, cur_y, line, color::WHITE);
        }
        cur_y += GLYPH_H + 2;
    }

    // If hovered, show full text tooltip so the user gets immediate responsive feedback
    if hovered && renaming.is_none() {
        let tip_w = text_width(&item.label) + 12;
        let tip_h = GLYPH_H + 6;
        let tip_x = (r.x + (r.w - tip_w) / 2).clamp(2, (s.width as i32 - tip_w - 2).max(2));
        let tip_y = r.y + ICON_H + 2;
        let tip_rect = Rect::new(tip_x, tip_y, tip_w, tip_h);
        s.fill(tip_rect, 0xFFFFE1); // classic light yellow tooltip
        s.hline(tip_x, tip_y, tip_w, color::BLACK);
        s.hline(tip_x, tip_y + tip_h - 1, tip_w, color::BLACK);
        s.vline(tip_x, tip_y, tip_h, color::BLACK);
        s.vline(tip_x + tip_w - 1, tip_y, tip_h, color::BLACK);
        s.text(tip_x + 6, tip_y + 3, &item.label, color::BLACK);
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

    #[test]
    fn finish_move_item_snaps_to_grid_when_enabled() {
        let mut dm = DesktopManager {
            items: vec![DesktopItem { id: "Files".into(), label: "Files".into(), kind: ItemKind::Files, x: 28, y: 105, icon: icons::FOLDER }],
            align_to_grid: true,
            auto_arrange: false,
            ..DesktopManager::default()
        };
        dm.finish_move_item(0, 768);
        assert_eq!(dm.items[0].x, GRID_X);
        assert_eq!(dm.items[0].y, GRID_Y + GRID_STEP_Y);
    }

    #[test]
    fn finish_move_item_rearranges_when_auto_arrange_enabled() {
        let mut dm = DesktopManager {
            items: vec![
                DesktopItem { id: "Recycle Bin".into(), label: "Recycle Bin".into(), kind: ItemKind::RecycleBin, x: 20, y: 20, icon: icons::RECYCLE },
                DesktopItem { id: "Files".into(), label: "Files".into(), kind: ItemKind::Files, x: 300, y: 400, icon: icons::FOLDER },
            ],
            align_to_grid: true,
            auto_arrange: true,
            ..DesktopManager::default()
        };
        dm.finish_move_item(1, 768);
        assert_eq!(dm.items[0].x, GRID_X);
        assert_eq!(dm.items[0].y, GRID_Y);
        assert_eq!(dm.items[1].x, GRID_X);
        assert_eq!(dm.items[1].y, GRID_Y + GRID_STEP_Y);
    }

    #[test]
    fn finish_move_item_preserves_position_when_auto_arrange_disabled() {
        let mut dm = DesktopManager {
            items: vec![DesktopItem { id: "Files".into(), label: "Files".into(), kind: ItemKind::Files, x: 250, y: 350, icon: icons::FOLDER }],
            align_to_grid: false,
            auto_arrange: false,
            ..DesktopManager::default()
        };
        dm.finish_move_item(0, 768);
        assert_eq!(dm.items[0].x, 250);
        assert_eq!(dm.items[0].y, 350);
    }
}
