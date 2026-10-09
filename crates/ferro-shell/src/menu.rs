//! Right-click context menus (desktop, icons, taskbar, window system menu,
//! explorer files/folders, Task Manager processes).

use super::*;

const ROW_H: i32 = 18;
const SEP_H: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cmd {
    Open,
    Properties,
    Refresh,
    NewFolder,
    NewTextDocument,
    Cascade,
    TileHorizontal,
    TileVertical,
    MinimizeAll,
    TaskManager,
    CommandPrompt,
    Explorer,
    Restore,
    Minimize,
    Maximize,
    Close,
    EndProcess,
    /// Task Manager: type a RAM budget for a process, or remove it.
    SetBudget,
    RemoveBudget,
    StripMetadata,
    AppPermissions,
    NetworkPrivacy,
}

/// What was right-clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Desktop,
    DesktopIcon(usize),
    Taskbar,
    Window(u32),
    /// A drive in My Computer or an entry in an explorer window.
    Item(u32, usize),
    /// The background of an explorer window (its current folder).
    Folder(u32),
    Process(u32),
    /// Start > Programs flyout.
    Programs,
    /// Start > Settings flyout.
    Settings,
}

pub(crate) enum Entry {
    Item { label: &'static str, cmd: Cmd, enabled: bool },
    Separator,
}

fn item(label: &'static str, cmd: Cmd, enabled: bool) -> Entry {
    Entry::Item { label, cmd, enabled }
}

pub(crate) struct ContextMenu {
    pub rect: Rect,
    target: Target,
    entries: Vec<Entry>,
    /// Drawn bold, like Windows' default verb.
    default: Option<Cmd>,
}

impl ContextMenu {
    fn new(x: i32, y: i32, screen: (i32, i32), target: Target, entries: Vec<Entry>, default: Option<Cmd>) -> Self {
        let text_w = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item { label, .. } => Some(text_width(label)),
                Entry::Separator => None,
            })
            .max()
            .unwrap_or(0);
        let w = (text_w + 44).max(120);
        let h = 6 + entries.iter().map(|e| if matches!(e, Entry::Separator) { SEP_H } else { ROW_H }).sum::<i32>();
        // Flip left/up when the menu would run off screen, as Windows does.
        let x = if x + w > screen.0 { (x - w).max(0) } else { x };
        let y = if y + h > screen.1 { (y - h).max(0) } else { y };
        Self { rect: Rect::new(x, y, w, h), target, entries, default }
    }

    fn entry_rects(&self) -> Vec<Rect> {
        let mut y = self.rect.y + 3;
        self.entries
            .iter()
            .map(|e| {
                let h = if matches!(e, Entry::Separator) { SEP_H } else { ROW_H };
                let r = Rect::new(self.rect.x + 3, y, self.rect.w - 6, h);
                y += h;
                r
            })
            .collect()
    }

    #[cfg(test)]
    pub fn target_for_test(&self) -> Target {
        self.target
    }

    /// Index of the selectable entry under (x, y), if any.
    pub fn item_at(&self, x: i32, y: i32) -> Option<usize> {
        self.entry_rects().iter().zip(&self.entries).position(|(r, e)| r.contains(x, y) && matches!(e, Entry::Item { .. }))
    }
}

impl Shell {
    /// Builds the menu for a right-click on `hit`, applying the selection
    /// change Windows makes (right-clicking an icon selects it).
    pub(crate) fn context_menu_for(&mut self, hit: Hit, x: i32, y: i32) -> Option<ContextMenu> {
        use Cmd::*;
        let (target, entries, default) = match hit {
            Hit::Nothing => {
                self.selected_icon = None;
                (Target::Desktop, vec![item("Refresh", Refresh, true), Entry::Separator, item("Properties", Properties, true)], None)
            }
            Hit::DesktopIcon(i) => {
                self.selected_icon = Some(i);
                (Target::DesktopIcon(i), vec![item("Open", Open, true), Entry::Separator, item("Properties", Properties, true)], Some(Open))
            }
            Hit::Taskbar | Hit::StartMenu | Hit::StartItem(_) => (
                Target::Taskbar,
                vec![
                    item("Cascade Windows", Cascade, true),
                    item("Tile Windows Horizontally", TileHorizontal, true),
                    item("Tile Windows Vertically", TileVertical, true),
                    Entry::Separator,
                    item("Minimize All Windows", MinimizeAll, true),
                    Entry::Separator,
                    item("Task Manager...", TaskManager, true),
                    item("Properties", Properties, true),
                ],
                None,
            ),
            Hit::Title(id) | Hit::TaskButton(id) | Hit::Close(id) | Hit::Min(id) | Hit::Max(id) => {
                let w = self.window(id)?;
                let (minmax, maxed, minimized) = (w.has_minmax(), w.restore.is_some(), w.minimized);
                (
                    Target::Window(id),
                    vec![
                        item("Restore", Restore, maxed || minimized),
                        item("Minimize", Minimize, minmax && !minimized),
                        item("Maximize", Maximize, minmax && !maxed),
                        Entry::Separator,
                        item("Close", Close, true),
                    ],
                    Some(Close),
                )
            }
            Hit::Item(id, i) => {
                self.focus(id);
                if let Some(pid) = self.tm_pid_at(id, i) {
                    self.tm_select(id, pid);
                    let mut entries = vec![item("End Process", EndProcess, pid > 1), Entry::Separator];
                    // Only your own programs (system services stay out of
                    // reach), and never the desktop itself.
                    let owner = self.info.processes.as_ref().and_then(|ps| ps.iter().find(|p| p.pid == pid)).map(|p| p.uid);
                    let mine = match (ferro_sys::current_uid(), owner) {
                        (Some(me), Some(o)) => me == o,
                        _ => true, // preview hosts: no ownership to check
                    };
                    let limitable = pid > 1 && pid != std::process::id() && mine;
                    entries.push(item("Set RAM Budget...", SetBudget, limitable));
                    entries.push(item("Remove RAM Budget", RemoveBudget, limitable));
                    return Some(ContextMenu::new(x, y, (self.w, self.h), Target::Process(pid), entries, None));
                }
                let w = self.window_mut(id)?;
                match &mut w.content {
                    Content::MyComputer => {
                        w.selected = Some(i);
                        (Target::Item(id, i), vec![item("Open", Open, true), Entry::Separator, item("Properties", Properties, true)], Some(Open))
                    }
                    Content::Explorer { entries, .. } => {
                        w.selected = Some(i);
                        let e = entries.get(i)?;
                        let mut v = vec![item("Open", Open, true)];
                        if e.name != ".." {
                            if !e.is_dir && ferro_meta::is_supported_name(&e.name) {
                                v.extend([Entry::Separator, item("Remove Metadata", StripMetadata, true)]);
                            }
                            v.extend([Entry::Separator, item("Properties", Properties, true)]);
                        }
                        (Target::Item(id, i), v, Some(Open))
                    }
                    _ => return None,
                }
            }
            Hit::Client(id) => {
                self.focus(id);
                let w = self.window_mut(id)?;
                if !matches!(w.content, Content::Explorer { .. }) {
                    return None;
                }
                w.selected = None;
                (
                    Target::Folder(id),
                    vec![
                        item("Refresh", Refresh, true),
                        Entry::Separator,
                        item("New Folder", NewFolder, true),
                        item("New Text Document", NewTextDocument, true),
                        Entry::Separator,
                        item("Properties", Properties, true),
                    ],
                    None,
                )
            }
            Hit::StartButton | Hit::MenuItem(_) | Hit::MenuFrame | Hit::TrayIcon(_) => return None,
        };
        Some(ContextMenu::new(x, y, (self.w, self.h), target, entries, default))
    }

    pub(crate) fn programs_menu(&self, x: i32, y: i32) -> ContextMenu {
        let entries = vec![
            item("Command Prompt", Cmd::CommandPrompt, true),
            item("Windows Explorer", Cmd::Explorer, true),
            item("Task Manager", Cmd::TaskManager, true),
        ];
        ContextMenu::new(x, y, (self.w, self.h), Target::Programs, entries, None)
    }

    pub(crate) fn settings_menu(&self, x: i32, y: i32) -> ContextMenu {
        let entries = vec![
            item("App Permissions...", Cmd::AppPermissions, true),
            item("VPN and Tor...", Cmd::NetworkPrivacy, true),
            item("Task Manager", Cmd::TaskManager, true),
        ];
        ContextMenu::new(x, y, (self.w, self.h), Target::Settings, entries, None)
    }

    pub(crate) fn run_menu(&mut self, i: usize) -> Option<Action> {
        let menu = self.menu.take()?;
        let Some(Entry::Item { cmd, enabled: true, .. }) = menu.entries.get(i) else { return None };
        let cmd = *cmd;
        self.start_open = false;
        match (menu.target, cmd) {
            (Target::Programs, Cmd::CommandPrompt) => self.open_terminal(),
            (Target::Programs, Cmd::Explorer) => self.open_explorer(r"C:\"),
            (Target::Programs | Target::Settings, Cmd::TaskManager) => self.open_task_manager(),
            (Target::Settings, Cmd::AppPermissions) => self.open_app_permissions(),
            (Target::Settings, Cmd::NetworkPrivacy) => self.open_network_privacy(),
            (Target::Desktop, Cmd::Properties) | (Target::Taskbar, Cmd::Properties) => self.open_about(),
            (Target::DesktopIcon(i), Cmd::Open) | (Target::DesktopIcon(i), Cmd::Properties) => match i {
                0 => self.open_my_computer(),
                1 => self.open_recycle_bin(),
                _ => self.open_about(),
            },
            (Target::Taskbar, Cmd::TaskManager) => self.open_task_manager(),
            (Target::Taskbar, c @ (Cmd::Cascade | Cmd::TileHorizontal | Cmd::TileVertical | Cmd::MinimizeAll)) => self.arrange(c),
            (Target::Window(id), Cmd::Restore) => {
                let maxed = self.window(id).is_some_and(|w| w.restore.is_some() && !w.minimized);
                self.focus(id);
                if maxed {
                    self.toggle_maximize(id);
                }
            }
            (Target::Window(id), Cmd::Minimize) => {
                if let Some(w) = self.window_mut(id) {
                    w.minimized = true;
                }
            }
            (Target::Window(id), Cmd::Maximize) => {
                self.focus(id);
                self.toggle_maximize(id);
            }
            (Target::Window(id), Cmd::Close) => self.close(id),
            (Target::Item(id, i), Cmd::Open) => self.activate_item(id, i),
            (Target::Item(id, i), Cmd::Properties) => {
                if let Some(path) = self.item_path(id, i) {
                    self.open_properties(&path);
                }
            }
            (Target::Folder(id), Cmd::Refresh) => self.refresh_explorer(id),
            (Target::Folder(id), Cmd::Properties) => {
                if let Some(path) = self.explorer_path(id) {
                    self.open_properties(&path);
                }
            }
            (Target::Folder(id), c @ (Cmd::NewFolder | Cmd::NewTextDocument)) => self.create_in_folder(id, c),
            (Target::Process(pid), Cmd::EndProcess) => return Some(Action::EndProcess(pid)),
            (Target::Process(pid), Cmd::SetBudget) => {
                let name = self.info.processes.as_ref().and_then(|ps| ps.iter().find(|p| p.pid == pid)).map(|p| p.name.clone());
                self.open_budget_dialog(pid, &name.unwrap_or_else(|| format!("pid {pid}")));
            }
            (Target::Process(pid), Cmd::RemoveBudget) => return self.apply_limit(pid, None),
            (Target::Item(id, i), Cmd::StripMetadata) => {
                if let Some(path) = self.item_path(id, i) {
                    self.remove_personal_info(&path);
                }
            }
            _ => {} // Refresh on the desktop: the next frame redraws anyway.
        }
        None
    }

    fn arrange(&mut self, how: Cmd) {
        let area = Rect::new(0, 0, self.w, self.h - TASKBAR_H);
        let visible: Vec<usize> = (0..self.windows.len()).filter(|&i| !self.windows[i].minimized).collect();
        let n = visible.len().max(1) as i32;
        for (k, &i) in visible.iter().enumerate() {
            let k = k as i32;
            let w = &mut self.windows[i];
            if how == Cmd::MinimizeAll {
                w.minimized = true;
                continue;
            }
            if let Some(r) = w.restore.take() {
                w.rect = r;
            }
            w.rect = match how {
                Cmd::Cascade => Rect::new(8 + k * 24, 8 + k * 24, w.rect.w, w.rect.h),
                Cmd::TileHorizontal => Rect::new(0, k * area.h / n, area.w, area.h / n),
                _ => Rect::new(k * area.w / n, 0, area.w / n, area.h),
            };
        }
    }

    fn explorer_path(&self, id: u32) -> Option<String> {
        match &self.window(id)?.content {
            Content::Explorer { path, .. } => Some(path.clone()),
            _ => None,
        }
    }

    /// Windows path of a My Computer drive or explorer entry.
    fn item_path(&self, id: u32, i: usize) -> Option<String> {
        match &self.window(id)?.content {
            Content::MyComputer => self.drives.drives().nth(i).map(|(l, _)| format!("{l}:\\")),
            Content::Explorer { path, entries, .. } => Some(ferro_path::join(path, &entries.get(i)?.name)),
            _ => None,
        }
    }

    fn refresh_explorer(&mut self, id: u32) {
        let Some(path) = self.explorer_path(id) else { return };
        let fresh = read_dir(&self.drives, &path);
        if let Some(Window { content: Content::Explorer { entries, error, .. }, selected, .. }) = self.window_mut(id) {
            (*entries, *error) = fresh;
            *selected = None;
        }
    }

    /// "New Folder" / "New Text Document", numbered like Windows if taken.
    fn create_in_folder(&mut self, id: u32, what: Cmd) {
        let Some(dir) = self.explorer_path(id) else { return };
        let Ok(posix_dir) = self.drives.to_posix(&dir) else { return };
        let (base, ext) = if what == Cmd::NewFolder { ("New Folder", "") } else { ("New Text Document", ".txt") };
        let name = (1..)
            .map(|n| if n == 1 { format!("{base}{ext}") } else { format!("{base} ({n}){ext}") })
            .find(|name| !Path::new(&posix_dir).join(name).exists())
            .expect("unbounded search");
        let target = Path::new(&posix_dir).join(&name);
        let result = if what == Cmd::NewFolder {
            std::fs::create_dir(&target)
        } else {
            std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map(drop)
        };
        match result {
            Ok(()) => self.refresh_explorer(id),
            Err(e) => self.show_error("Error", &format!("Cannot create {name} in {dir}: {e}")),
        }
    }

    pub(crate) fn draw_context_menu(&self, s: &mut Surface, m: &ContextMenu) {
        s.fill(m.rect, FACE);
        s.bevel(m.rect, Bevel::Window);
        let (cx, cy) = self.cursor;
        for (r, e) in m.entry_rects().into_iter().zip(&m.entries) {
            match e {
                Entry::Separator => s.etched_hline(r.x + 1, r.y + 3, r.w - 2),
                Entry::Item { label, cmd, enabled } => {
                    let hover = *enabled && r.contains(cx, cy);
                    if hover {
                        s.fill(r, NAVY);
                    }
                    let (x, y) = (r.x + 18, r.y + 5);
                    let bold = m.default == Some(*cmd);
                    match (enabled, hover, bold) {
                        (false, _, _) => {
                            s.text_disabled(x, y, label);
                        }
                        (true, h, true) => {
                            s.text_bold(x, y, label, if h { WHITE } else { BLACK });
                        }
                        (true, h, false) => {
                            s.text(x, y, label, if h { WHITE } else { BLACK });
                        }
                    }
                }
            }
        }
    }
}
