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
        let tip_w = text_width(&self.label) + 12;
        let w = ICON_W.max(tip_w);
        let x = self.x - (w - ICON_W) / 2;
        Rect::new(x - 4, self.y - 2, w + 8, ICON_H + 28)
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
                    "sh -c 'ferro-terminal || lxterminal || xterm'".into()
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
    pub text: String,
    pub cursor: usize,
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
}

fn config_path() -> PathBuf {
    #[cfg(test)]
    {
        return std::env::temp_dir().join("ferro_test_desktop.conf");
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
        };
        dm.refresh(screen_w, screen_h);
        dm
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
                kind: ItemKind::Shortcut { name: "Command Prompt".into(), exec: "sh -c 'ferro-terminal || lxterminal || xterm'".into() },
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
                    new_items.push(DesktopItem { id, label: name.to_string(), kind: ItemKind::File(path.clone()), x, y, icon: icons::FILE });
                }
            }
        }

        self.items = new_items;
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
        for (i, item) in self.items.iter().enumerate().rev() {
            let mut hit_r = item.rect();
            if self.hovered == Some(i) {
                hit_r.h += GLYPH_H + 12;
            }
            if hit_r.contains(x, y) {
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

    /// Called on mouse release after moving an item.
    pub fn finish_move_item(&mut self, idx: usize, screen_h: i32) {
        if self.auto_arrange {
            self.realign_grid(screen_h);
        } else if self.align_to_grid {
            if let Some(item) = self.items.get_mut(idx) {
                let col = ((item.x - GRID_X + GRID_STEP_X / 2) / GRID_STEP_X).max(0);
                let row = ((item.y - GRID_Y + GRID_STEP_Y / 2) / GRID_STEP_Y).max(0);
                item.x = GRID_X + col * GRID_STEP_X;
                item.y = GRID_Y + row * GRID_STEP_Y;
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
            let cursor = initial_text.len();
            self.renaming = Some(InlineRename { item_idx: idx, text: initial_text, cursor });
        }
    }

    pub fn commit_rename(&mut self) -> Option<Rect> {
        let ren = self.renaming.take()?;
        let item = self.items.get(ren.item_idx)?;
        let old_rect = item.visual_rect();
        let new_name = ren.text.trim();
        if !new_name.is_empty() && new_name != item.label {
            let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/home/ferro"));
            match &item.kind {
                ItemKind::Folder(p) | ItemKind::File(p) => {
                    if let Some(parent) = p.parent() {
                        let target = parent.join(new_name);
                        let _ = fs::rename(p, &target);
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
                        }
                    }
                }
                _ => {}
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
            draw_item(s, item, selected, hovered, ren);
        }
        if let Some(rb) = self.rubberband {
            focus_rect(s, rb);
        }
    }
}

pub fn focus_rect(s: &mut Surface, r: Rect) {
    for x in (r.x..r.right()).step_by(2) {
        s.hline(x, r.y, 1, color::BLACK);
        s.hline(x, r.bottom() - 1, 1, color::BLACK);
    }
    for y in (r.y..r.bottom()).step_by(2) {
        s.vline(r.x, y, 1, color::BLACK);
        s.vline(r.right() - 1, y, 1, color::BLACK);
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

pub fn draw_item(s: &mut Surface, item: &DesktopItem, selected: bool, hovered: bool, renaming: Option<&InlineRename>) {
    let r = item.rect();

    // Hover outline / soft highlight when moused over
    if hovered && !selected && renaming.is_none() {
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

    if let Some(ren) = renaming {
        let tw = text_width(&ren.text);
        let box_w = (tw + 16).clamp(64, 160);
        let box_x = (r.x + (r.w - box_w) / 2).clamp(2, s.width as i32 - box_w - 2);
        let box_y = r.y + ICON_SIZE + 4;
        let box_h = GLYPH_H + 4;
        let box_rect = Rect::new(box_x, box_y, box_w, box_h);
        s.fill(box_rect, color::WHITE);
        s.hline(box_x, box_y, box_w, color::BLACK);
        s.hline(box_x, box_y + box_h - 1, box_w, color::BLACK);
        s.vline(box_x, box_y, box_h, color::BLACK);
        s.vline(box_x + box_w - 1, box_y, box_h, color::BLACK);
        s.text(box_x + 3, box_y + 2, &ren.text, color::BLACK);
        let cur_x = box_x + 3 + text_width(&ren.text[..ren.cursor]);
        s.vline(cur_x, box_y + 2, GLYPH_H, color::BLACK);
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

    // If hovered and label was truncated or multi-word, show full text tooltip
    let is_truncated = lines.join(" ") != item.label;
    if hovered && is_truncated && renaming.is_none() {
        let tip_w = text_width(&item.label) + 8;
        let tip_h = GLYPH_H + 6;
        let tip_x = (r.x + (r.w - tip_w) / 2).clamp(2, (s.width as i32 - tip_w - 2).max(2));
        let tip_y = r.y + ICON_H + 2;
        let tip_rect = Rect::new(tip_x, tip_y, tip_w, tip_h);
        s.fill(tip_rect, 0xFFFFE1); // classic light yellow tooltip
        s.hline(tip_x, tip_y, tip_w, color::BLACK);
        s.hline(tip_x, tip_y + tip_h - 1, tip_w, color::BLACK);
        s.vline(tip_x, tip_y, tip_h, color::BLACK);
        s.vline(tip_x + tip_w - 1, tip_y, tip_h, color::BLACK);
        s.text(tip_x + 4, tip_y + 3, &item.label, color::BLACK);
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
            items: vec![
                DesktopItem { id: "Files".into(), label: "Files".into(), kind: ItemKind::Files, x: 250, y: 350, icon: icons::FOLDER },
            ],
            align_to_grid: false,
            auto_arrange: false,
            ..DesktopManager::default()
        };
        dm.finish_move_item(0, 768);
        assert_eq!(dm.items[0].x, 250);
        assert_eq!(dm.items[0].y, 350);
    }
}
