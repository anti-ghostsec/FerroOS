//! What the Task Manager shows and where its parts are: a Processes tab
//! (a list that sorts by any column, and End Process) and a Performance
//! tab (meters and a minute of history for the processor and memory).
//! The look is FerroOS's own Task Manager's. Pure geometry and pixels
//! (ferro-gfx), so it's the same in tests as on screen.

use ferro_gfx::{color, ellipsize, text_width, Bevel, Rect, Scrollbar, Surface, SCROLLBAR_W};
use ferro_ui::{button, default_button};
use std::collections::VecDeque;

pub const W: i32 = 500;
pub const H: i32 = 400;
/// A minute of history, one sample a second.
pub const HISTORY_LEN: usize = 60;

const ROW_H: i32 = 14;
const HEADER_H: i32 = 18;
/// The columns and their widths; the first takes what's left.
const COLUMNS: [(&str, i32); 6] = [("Image Name", 0), ("User", 64), ("PID", 50), ("CPU", 38), ("Mem Usage", 88), ("RAM Budget", 84)];
const NCOLS: usize = COLUMNS.len();
const COL_CPU: usize = 3;
const COL_MEM: usize = 4;
#[allow(dead_code)]
pub const COL_BUDGET: usize = 5;

const GRAPH_BG: u32 = 0x000000;
const GRID: u32 = 0x008040;
const LIT: u32 = 0x00FF00;
const UNLIT: u32 = 0x006000;
const MEM_LINE: u32 = 0xFFFF00;
const BUDGET_LINE: u32 = 0xFF4040;

/// One running program.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub user: String,
    /// Share of the whole machine's processors, 0 to 100.
    pub cpu: f32,
    pub mem_kb: u64,
    /// RAM budget limit in KB, if set.
    pub mem_limit_kb: Option<u64>,
    /// Started through sandbox.
    pub sandboxed: bool,
    pub depth: usize,
    pub prefix: String,
}

/// Modal dialog for entering a process RAM budget.
#[derive(Clone, Debug, PartialEq)]
pub struct BudgetDialog {
    pub pid: u32,
    pub name: String,
    pub text: String,
    pub error: Option<String>,
}

/// Everything the window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    /// 0 = Processes, 1 = Performance.
    pub tab: usize,
    pub sort_col: usize,
    pub sort_desc: bool,
    pub scroll: usize,
    pub selected: Option<u32>,
    pub rows: Vec<Row>,
    pub cpu: Option<f32>,
    /// Used and total, in KB.
    pub mem: Option<(u64, u64)>,
    pub uptime_secs: Option<u64>,
    pub cpu_history: VecDeque<f32>,
    pub mem_history: VecDeque<f32>,
    /// A line in the status bar, e.g. why something couldn't be ended.
    pub message: &'static str,
    /// In-window modal dialog for setting RAM quota.
    pub budget_dialog: Option<BudgetDialog>,
    /// Right-click process context menu.
    pub context_menu: Option<ContextMenu>,
    /// Whether related processes are shown as sub-trees.
    pub tree_view: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessAction {
    EndProcess,
    SetBudget,
    RemoveBudget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMenuItem {
    pub label: Option<&'static str>,
    pub action: Option<ProcessAction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMenu {
    pub x: i32,
    pub y: i32,
    pub pid: u32,
    pub items: Vec<ContextMenuItem>,
}

impl ContextMenu {
    pub fn for_process(x: i32, y: i32, pid: u32) -> Self {
        Self {
            x,
            y,
            pid,
            items: vec![
                ContextMenuItem { label: Some("End Process"), action: Some(ProcessAction::EndProcess) },
                ContextMenuItem { label: None, action: None },
                ContextMenuItem { label: Some("Set RAM Budget..."), action: Some(ProcessAction::SetBudget) },
                ContextMenuItem { label: Some("Remove RAM Budget"), action: Some(ProcessAction::RemoveBudget) },
            ],
        }
    }

    pub fn rect(&self) -> Rect {
        let h = self.items.iter().map(|item| if item.label.is_some() { 20 } else { 6 }).sum::<i32>() + 6;
        Rect::new(self.x, self.y, 160, h)
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<ProcessAction> {
        let r = self.rect();
        if !r.contains(x, y) {
            return None;
        }
        let mut cur_y = r.y + 3;
        for item in &self.items {
            let h = if item.label.is_some() { 20 } else { 6 };
            if y >= cur_y && y < cur_y + h {
                return item.action;
            }
            cur_y += h;
        }
        None
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            tab: 0,
            // Biggest memory users first.
            sort_col: COL_MEM,
            sort_desc: true,
            scroll: 0,
            selected: None,
            rows: Vec::new(),
            cpu: None,
            mem: None,
            uptime_secs: None,
            cpu_history: VecDeque::with_capacity(HISTORY_LEN),
            mem_history: VecDeque::with_capacity(HISTORY_LEN),
            message: "",
            budget_dialog: None,
            context_menu: None,
            tree_view: true,
        }
    }
}

fn compare_rows(a: &Row, b: &Row, col: usize, desc: bool) -> std::cmp::Ordering {
    let o = match col {
        0 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        1 => a.user.cmp(&b.user),
        2 => a.pid.cmp(&b.pid),
        COL_CPU => a.cpu.total_cmp(&b.cpu),
        COL_MEM => a.mem_kb.cmp(&b.mem_kb),
        _ => (a.sandboxed, a.mem_limit_kb).cmp(&(b.sandboxed, b.mem_limit_kb)),
    };
    let o = if desc { o.reverse() } else { o };
    o.then(a.pid.cmp(&b.pid))
}

pub fn build_process_tree(rows: Vec<Row>, sort_col: usize, sort_desc: bool) -> Vec<Row> {
    use std::collections::{HashMap, HashSet};

    let pids: HashSet<u32> = rows.iter().map(|r| r.pid).collect();
    let mut by_ppid: HashMap<u32, Vec<Row>> = HashMap::new();
    let mut roots: Vec<Row> = Vec::new();

    for row in rows {
        if row.ppid == 0 || row.ppid == 1 || !pids.contains(&row.ppid) || row.ppid == row.pid {
            roots.push(row);
        } else {
            by_ppid.entry(row.ppid).or_default().push(row);
        }
    }

    roots.sort_by(|a, b| compare_rows(a, b, sort_col, sort_desc));
    for children in by_ppid.values_mut() {
        children.sort_by(|a, b| compare_rows(a, b, sort_col, sort_desc));
    }

    let mut result = Vec::new();

    fn traverse(
        current: Row,
        depth: usize,
        is_last: bool,
        ancestors_last: &mut Vec<bool>,
        by_ppid: &mut HashMap<u32, Vec<Row>>,
        result: &mut Vec<Row>,
    ) {
        let mut prefix = String::new();
        if depth > 0 {
            for &anc_last in ancestors_last.iter().take(depth - 1) {
                if anc_last {
                    prefix.push_str("   ");
                } else {
                    prefix.push_str("│  ");
                }
            }
            if is_last {
                prefix.push_str("└─ ");
            } else {
                prefix.push_str("├─ ");
            }
        }

        let pid = current.pid;
        let mut row = current;
        row.depth = depth;
        row.prefix = prefix;
        result.push(row);

        if let Some(children) = by_ppid.remove(&pid) {
            let total = children.len();
            ancestors_last.push(is_last);
            for (idx, child) in children.into_iter().enumerate() {
                let last = idx + 1 == total;
                traverse(child, depth + 1, last, ancestors_last, by_ppid, result);
            }
            ancestors_last.pop();
        }
    }

    let mut ancestors_last = Vec::new();
    let total_roots = roots.len();
    for (idx, root) in roots.into_iter().enumerate() {
        let is_last = idx + 1 == total_roots;
        traverse(root, 0, is_last, &mut ancestors_last, &mut by_ppid, &mut result);
    }

    for (_, mut remaining) in by_ppid {
        result.append(&mut remaining);
    }

    result
}

impl State {
    /// Takes in a fresh sample; the list keeps its order and selection.
    pub fn update(&mut self, mut rows: Vec<Row>, cpu: Option<f32>, mem: Option<(u64, u64)>, uptime_secs: Option<u64>) {
        if self.tree_view {
            self.rows = build_process_tree(rows, self.sort_col, self.sort_desc);
        } else {
            let (col, desc) = (self.sort_col, self.sort_desc);
            rows.sort_by(|a, b| compare_rows(a, b, col, desc));
            self.rows = rows;
        }
        if self.selected.is_some_and(|pid| !self.rows.iter().any(|r| r.pid == pid)) {
            self.selected = None;
            if self.budget_dialog.as_ref().is_some_and(|d| !self.rows.iter().any(|r| r.pid == d.pid)) {
                self.budget_dialog = None;
            }
        }
        (self.cpu, self.mem, self.uptime_secs) = (cpu, mem, uptime_secs);
        push(&mut self.cpu_history, cpu.unwrap_or(0.0) / 100.0);
        push(&mut self.mem_history, mem.map_or(0.0, |(used, total)| used as f32 / total.max(1) as f32));
    }

    /// A click on a column's header: sort by it, or turn the order round.
    pub fn sort_by(&mut self, col: usize) {
        if self.sort_col == col {
            self.sort_desc = !self.sort_desc;
        } else {
            self.sort_col = col;
            // Numbers biggest first, text A to Z.
            self.sort_desc = col >= COL_CPU;
        }
        let rows = std::mem::take(&mut self.rows);
        let (cpu, mem, up) = (self.cpu, self.mem, self.uptime_secs);
        // Sorting again mustn't count as a new moment in the history.
        let (ch, mh) = (self.cpu_history.clone(), self.mem_history.clone());
        self.update(rows, cpu, mem, up);
        (self.cpu_history, self.mem_history) = (ch, mh);
    }

    /// Moves the selection up or down the list, scrolling to keep it seen.
    pub fn step(&mut self, down: bool, w: i32, h: i32) {
        if self.rows.is_empty() {
            return;
        }
        let at = self.selected.and_then(|pid| self.rows.iter().position(|r| r.pid == pid));
        let next = match (at, down) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(self.rows.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
        };
        self.selected = Some(self.rows[next].pid);
        let visible = layout(w, h, self).visible;
        if next < self.scroll {
            self.scroll = next;
        } else if next >= self.scroll + visible {
            self.scroll = next + 1 - visible;
        }
    }

    pub fn scroll_by(&mut self, lines: i32, w: i32, h: i32) {
        let most = self.rows.len().saturating_sub(layout(w, h, self).visible);
        self.scroll = (self.scroll as i64 + i64::from(lines)).clamp(0, most as i64) as usize;
    }

    /// Whether End Process has something it may end. Never process 1:
    /// that's the system itself.
    pub fn can_end(&self) -> bool {
        self.tab == 0 && self.selected.is_some_and(|pid| pid > 1) && self.budget_dialog.is_none()
    }

    /// Whether Set Budget has a user process selected.
    #[allow(dead_code)]
    pub fn can_budget(&self) -> bool {
        self.tab == 0 && self.selected.is_some_and(|pid| pid > 1) && self.budget_dialog.is_none()
    }

    /// Opens the RAM budget input modal for the selected process.
    pub fn open_budget_dialog(&mut self) {
        let Some(pid) = self.selected else { return };
        let Some(row) = self.rows.iter().find(|r| r.pid == pid) else { return };
        let current_text = match row.mem_limit_kb {
            Some(kb) => {
                let mb = (kb + 512) / 1024;
                if mb >= 1024 && mb % 1024 == 0 {
                    format!("{}G", mb / 1024)
                } else {
                    format!("{mb}M")
                }
            }
            None => String::new(),
        };
        self.budget_dialog = Some(BudgetDialog { pid, name: row.name.clone(), text: current_text, error: None });
    }

    pub fn close_budget_dialog(&mut self) {
        self.budget_dialog = None;
    }
}

fn push(history: &mut VecDeque<f32>, v: f32) {
    if history.len() == HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(v);
}

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    Header(usize),
    /// A program in the list, by its process number.
    Row(u32),
    End,
    ToggleTree,
    BudgetOk,
    BudgetCancel,
    ContextMenuAction(ProcessAction),
    DismissMenu,
    ScrollUp,
    ScrollDown,
    /// The scrollbar's track above (`false`) or below (`true`) its thumb.
    Page(bool),
    Nothing,
}

struct Layout {
    tabs: [Rect; 2],
    panel: Rect,
    list: Rect,
    headers: [Rect; NCOLS],
    rows: Rect,
    visible: usize,
    scroll: usize,
    scrollbar: Scrollbar,
    tree_button: Rect,
    end_button: Rect,
    status: Rect,
    dialog_rect: Option<Rect>,
    dialog_ok: Option<Rect>,
    dialog_cancel: Option<Rect>,
}

fn layout(w: i32, h: i32, st: &State) -> Layout {
    let (w, h) = (w.max(320), h.max(200));
    let tabs = [Rect::new(6, 6, 84, 20), Rect::new(90, 6, 104, 20)];
    let status = Rect::new(2, h - 22, w - 4, 20);
    let panel = Rect::new(2, 24, w - 4, status.y - 4 - 24);
    let p = panel.inset(8);
    let tree_button = Rect::new(p.x, p.bottom() - 23, 90, 23);
    let end_button = Rect::new(p.right() - 100, p.bottom() - 23, 100, 23);
    let list = Rect::new(p.x, p.y, p.w, p.h - 31);
    let inner = list.inset(2);
    let fixed: i32 = COLUMNS.iter().map(|c| c.1).sum();
    let mut headers = [Rect::default(); NCOLS];
    let mut x = inner.x;
    for (k, (_, cw)) in COLUMNS.iter().enumerate() {
        let cw = if k == 0 { inner.w - SCROLLBAR_W - fixed } else { *cw };
        headers[k] = Rect::new(x, inner.y, cw, HEADER_H);
        x += cw;
    }
    let rows = Rect::new(inner.x, inner.y + HEADER_H, inner.w - SCROLLBAR_W, inner.h - HEADER_H);
    let visible = (rows.h / ROW_H).max(1) as usize;
    let scroll = st.scroll.min(st.rows.len().saturating_sub(visible));
    let scrollbar = Scrollbar::layout(Rect::new(rows.right(), rows.y, SCROLLBAR_W, rows.h), scroll, visible, st.rows.len());

    let (dialog_rect, dialog_ok, dialog_cancel) = if st.budget_dialog.is_some() {
        let (dw, dh) = (340, 150);
        let dr = Rect::new((w - dw) / 2, (h - dh) / 2, dw, dh);
        let ok = Rect::new(dr.right() - 165, dr.bottom() - 30, 72, 22);
        let cancel = Rect::new(dr.right() - 85, dr.bottom() - 30, 72, 22);
        (Some(dr), Some(ok), Some(cancel))
    } else {
        (None, None, None)
    };

    Layout { tabs, panel, list, headers, rows, visible, scroll, scrollbar, tree_button, end_button, status, dialog_rect, dialog_ok, dialog_cancel }
}

pub fn hit_row(w: i32, h: i32, st: &State, x: i32, y: i32) -> Option<u32> {
    if st.tab != 0 || st.budget_dialog.is_some() {
        return None;
    }
    let l = layout(w, h, st);
    if l.rows.contains(x, y) {
        let i = l.scroll + ((y - l.rows.y) / ROW_H) as usize;
        return st.rows.get(i).map(|r| r.pid);
    }
    None
}

pub fn hit(w: i32, h: i32, st: &State, x: i32, y: i32) -> Hit {
    let l = layout(w, h, st);
    if let Some(menu) = &st.context_menu {
        if let Some(act) = menu.hit(x, y) {
            return Hit::ContextMenuAction(act);
        }
        return Hit::DismissMenu;
    }
    if st.budget_dialog.is_some() {
        if let Some(ok) = l.dialog_ok {
            if ok.contains(x, y) {
                return Hit::BudgetOk;
            }
        }
        if let Some(cancel) = l.dialog_cancel {
            if cancel.contains(x, y) {
                return Hit::BudgetCancel;
            }
        }
        return Hit::Nothing;
    }
    if let Some(i) = l.tabs.iter().position(|t| t.contains(x, y)) {
        return Hit::Tab(i);
    }
    if st.tab != 0 {
        return Hit::Nothing;
    }
    if let Some(i) = l.headers.iter().position(|r| r.contains(x, y)) {
        return Hit::Header(i);
    }
    if l.tree_button.contains(x, y) {
        return Hit::ToggleTree;
    }
    if l.end_button.contains(x, y) {
        return Hit::End;
    }
    if l.scrollbar.up.contains(x, y) {
        return Hit::ScrollUp;
    }
    if l.scrollbar.down.contains(x, y) {
        return Hit::ScrollDown;
    }
    if l.scrollbar.track.contains(x, y) {
        return Hit::Page(y >= l.scrollbar.thumb.y);
    }
    if l.rows.contains(x, y) {
        let i = l.scroll + ((y - l.rows.y) / ROW_H) as usize;
        return st.rows.get(i).map_or(Hit::Nothing, |r| Hit::Row(r.pid));
    }
    Hit::Nothing
}

/// How many list rows a page of the scrollbar moves.
pub fn page(w: i32, h: i32, st: &State) -> i32 {
    layout(w, h, st).visible as i32
}

/// `1234567` -> `1,234,567`.
pub fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn mb(kb: u64) -> u64 {
    (kb + 512) / 1024
}

/// Draws the whole window, `w` x `h`.
pub fn draw(s: &mut Surface, w: i32, h: i32, st: &State) {
    use color::{BLACK, FACE, SHADOW, WHITE};
    s.fill(Rect::new(0, 0, w, h), FACE);
    let l = layout(w, h, st);

    // The selected tab is taller and joins the panel under it.
    s.bevel(l.panel, Bevel::Window);
    for (k, (tab, label)) in l.tabs.iter().zip(["Processes", "Performance"]).enumerate() {
        let r = if st.tab == k { Rect::new(tab.x - 2, tab.y - 2, tab.w + 4, tab.h + 3) } else { *tab };
        s.fill(r, FACE);
        s.hline(r.x + 1, r.y, r.w - 2, WHITE);
        s.vline(r.x, r.y + 1, r.h - 1, WHITE);
        s.vline(r.right() - 1, r.y + 1, r.h - 1, BLACK);
        s.vline(r.right() - 2, r.y + 1, r.h - 1, SHADOW);
        s.text(r.x + (r.w - text_width(label)) / 2, r.y + 7, label, BLACK);
    }

    if st.tab == 0 {
        draw_processes(s, st, &l);
    } else {
        draw_performance(s, st, l.panel.inset(10));
    }

    // Modal budget dialog if active
    if st.budget_dialog.is_some() {
        draw_budget_dialog(s, st, &l);
    }

    // The status bar: three sunken fields.
    let cpu = st.cpu.map_or("--".into(), |c| format!("{c:.0}%"));
    let mem = st.mem.map_or("n/a".into(), |(u, t)| format!("{}M / {}M", mb(u), mb(t)));
    let first = if st.message.is_empty() { format!("Processes: {}", st.rows.len()) } else { st.message.to_owned() };
    // A message gets the room of two fields.
    let widths = if st.message.is_empty() { [1, 1, 1] } else { [2, 0, 1] };
    let unit = l.status.w / 3;
    let mut x = l.status.x;
    for (text, span) in [first, format!("CPU: {cpu}"), format!("Mem: {mem}")].iter().zip(widths) {
        if span == 0 {
            continue;
        }
        let r = Rect::new(x, l.status.y, unit * span - 2, l.status.h);
        s.bevel(r, Bevel::Field);
        s.text(r.x + 4, r.y + 6, &ellipsize(text, ((r.w - 8) / 8).max(0) as usize), BLACK);
        x += unit * span;
    }

    // Right-click context menu if active
    if st.context_menu.is_some() {
        draw_context_menu(s, st, w, h);
    }
}

fn draw_context_menu(s: &mut Surface, st: &State, w: i32, h: i32) {
    let Some(menu) = &st.context_menu else { return };
    let r = menu.rect();
    let mx = r.x.clamp(2, (w - r.w - 4).max(2));
    let my = r.y.clamp(2, (h - r.h - 4).max(2));
    let mr = Rect::new(mx, my, r.w, r.h);

    s.fill(mr, color::FACE);
    s.bevel(mr, Bevel::Window);

    let mut item_y = mr.y + 3;
    for item in &menu.items {
        if let Some(label) = item.label {
            let item_rect = Rect::new(mr.x + 3, item_y, mr.w - 6, 20);
            s.text(item_rect.x + 8, item_rect.y + 4, label, color::BLACK);
            item_y += 20;
        } else {
            s.etched_hline(mr.x + 4, item_y + 2, mr.w - 8);
            item_y += 6;
        }
    }
}

fn draw_processes(s: &mut Surface, st: &State, l: &Layout) {
    use color::{BLACK, NAVY, WHITE};
    s.fill(l.list, WHITE);
    s.bevel(l.list, Bevel::Sunken);
    for (k, (r, (label, _))) in l.headers.iter().zip(COLUMNS).enumerate() {
        s.button(*r, false);
        let shown = ellipsize(label, ((r.w - 8) / 8).max(0) as usize);
        if st.sort_col == k {
            s.text_bold(r.x + 4, r.y + 5, &shown, BLACK);
        } else {
            s.text(r.x + 4, r.y + 5, &shown, BLACK);
        }
    }
    // The blank header above the scrollbar.
    s.button(Rect::new(l.rows.right(), l.headers[0].y, SCROLLBAR_W, HEADER_H), false);

    for (k, p) in st.rows.iter().skip(l.scroll).take(l.visible).enumerate() {
        let y = l.rows.y + k as i32 * ROW_H;
        let selected = st.selected == Some(p.pid);
        let fg = if selected { WHITE } else { BLACK };
        if selected {
            s.fill(Rect::new(l.rows.x, y, l.rows.w, ROW_H), NAVY);
        }
        let room = |col: usize| ((l.headers[col].w - 8) / 8).max(0) as usize;
        let disp_name = if p.prefix.is_empty() { p.name.clone() } else { format!("{}{}", p.prefix, p.name) };
        s.text(l.headers[0].x + 4, y + 3, &ellipsize(&disp_name, room(0)), fg);
        s.text(l.headers[1].x + 4, y + 3, &ellipsize(&p.user, room(1)), fg);
        let budget_str = match (p.mem_limit_kb, p.sandboxed) {
            (Some(kb), _) => format!("{} MB", (kb + 512) / 1024),
            (None, true) => "none".into(),
            (None, false) => "-".into(),
        };
        let numbers = [p.pid.to_string(), format!("{:02.0}", p.cpu.min(99.0)), format!("{} K", group_digits(p.mem_kb)), budget_str];
        for (r, text) in l.headers[2..].iter().zip(numbers) {
            s.text(r.right() - 6 - text_width(&text), y + 3, &text, fg);
        }
    }
    s.scrollbar(&l.scrollbar, false, false);
    button(s, l.tree_button, if st.tree_view { "Flat View" } else { "Tree View" }, true);
    button(s, l.end_button, "End Process", st.can_end());
}

fn draw_budget_dialog(s: &mut Surface, st: &State, l: &Layout) {
    let Some(d) = &st.budget_dialog else { return };
    let Some(rect) = l.dialog_rect else { return };
    use color::{BLACK, FACE, RED, WHITE};

    s.fill(rect, FACE);
    s.bevel(rect, Bevel::Window);

    let title_bar = Rect::new(rect.x + 3, rect.y + 3, rect.w - 6, 18);
    s.fill(title_bar, color::NAVY);
    s.text(title_bar.x + 5, title_bar.y + 3, "RAM Budget", WHITE);

    let app_label = format!("Process: {} (PID {})", ellipsize(&d.name, 22), d.pid);
    s.text(rect.x + 12, rect.y + 28, &app_label, BLACK);
    s.text(rect.x + 12, rect.y + 44, "Memory limit (e.g. 512M, 2G, or none):", BLACK);

    let field = Rect::new(rect.x + 12, rect.y + 62, rect.w - 24, 22);
    s.fill(field, WHITE);
    s.bevel(field, Bevel::Sunken);
    s.text(field.x + 5, field.y + 5, &d.text, BLACK);
    let caret_x = field.x + 5 + text_width(&d.text);
    s.vline(caret_x, field.y + 4, 14, BLACK);

    if let Some(err) = &d.error {
        s.text(rect.x + 12, rect.y + 90, err, RED);
    }

    if let Some(ok_r) = l.dialog_ok {
        default_button(s, ok_r, "OK", true);
    }
    if let Some(cancel_r) = l.dialog_cancel {
        button(s, cancel_r, "Cancel", true);
    }
}

fn draw_performance(s: &mut Surface, st: &State, p: Rect) {
    let row_h = (p.h - 44) / 2;
    let cpu = st.cpu.map(|c| c / 100.0);
    for (k, (meter, history)) in [("CPU Usage", "CPU Usage History"), ("MEM Usage", "Memory Usage History")].iter().enumerate() {
        let y = p.y + 6 + k as i32 * (row_h + 8);
        let meter_box = Rect::new(p.x, y, 92, row_h);
        let graph_box = Rect::new(p.x + 100, y, p.w - 100, row_h);
        s.group_box(meter_box, meter);
        s.group_box(graph_box, history);
        let (frac, label) = if k == 0 {
            (cpu, cpu.map(|c| format!("{:.0} %", c * 100.0)))
        } else {
            (st.mem.map(|(u, t)| u as f32 / t.max(1) as f32), st.mem.map(|(u, _)| format!("{} MB", mb(u))))
        };
        draw_meter(s, meter_box.inset(10), frac.unwrap_or(0.0), label.as_deref().unwrap_or("n/a"));
        let (points, line) = if k == 0 { (&st.cpu_history, LIT) } else { (&st.mem_history, MEM_LINE) };
        let budget_frac = if k == 1 { st.mem.map(|(_, t)| ferro_sys::HEAVY_IDLE_BUDGET_KB as f32 / t.max(1) as f32) } else { None };
        draw_graph(s, graph_box.inset(10), points, line, budget_frac);
    }
    let ty = p.bottom() - 30;
    let uptime = st.uptime_secs.map_or("n/a".into(), |u| format!("{}:{:02}:{:02}", u / 3600, u / 60 % 60, u % 60));
    s.text(p.x, ty, &format!("Processes: {}    Up time: {uptime}", st.rows.len()), color::BLACK);
    let mem = st.mem.map_or("Memory: n/a".into(), |(u, t)| {
        format!("Memory: {} of {} MB used (idle budget: {} MB)", mb(u), mb(t), ferro_sys::HEAVY_IDLE_BUDGET_KB / 1024)
    });
    s.text(p.x, ty + 14, &ellipsize(&mem, (p.w / 8).max(0) as usize), color::BLACK);
}

/// An LED bar meter: two columns of segments lit from the bottom.
fn draw_meter(s: &mut Surface, r: Rect, frac: f32, label: &str) {
    s.fill(r, GRAPH_BG);
    let bars = Rect::new(r.x + r.w / 2 - 17, r.y + 4, 34, r.h - 20);
    let segments = (bars.h / 3).max(1);
    let lit = (segments as f32 * frac.clamp(0.0, 1.0)).round() as i32;
    for k in 0..segments {
        let y = bars.bottom() - (k + 1) * 3;
        let c = if k < lit { LIT } else { UNLIT };
        s.fill(Rect::new(bars.x, y, 16, 2), c);
        s.fill(Rect::new(bars.x + 18, y, 16, 2), c);
    }
    s.text(r.x + (r.w - text_width(label)) / 2, r.bottom() - 12, label, LIT);
}

/// A scrolling line graph, the newest sample at the right edge.
fn draw_graph(s: &mut Surface, r: Rect, history: &VecDeque<f32>, line: u32, budget_frac: Option<f32>) {
    s.fill(r, GRAPH_BG);
    for gx in (r.x..r.right()).step_by(12) {
        s.vline(gx, r.y, r.h, GRID);
    }
    for gy in (r.y..r.bottom()).step_by(12) {
        s.hline(r.x, gy, r.w, GRID);
    }
    if let Some(bf) = budget_frac {
        if bf > 0.0 && bf <= 1.0 {
            let by = r.bottom() - 1 - (bf * (r.h - 1) as f32) as i32;
            for bx in (r.x..r.right()).step_by(6) {
                s.hline(bx, by, 3, BUDGET_LINE);
            }
        }
    }
    let y_of = |v: f32| r.bottom() - 1 - (v.clamp(0.0, 1.0) * (r.h - 1) as f32) as i32;
    let step = (r.w - 1) as f32 / (HISTORY_LEN - 1) as f32;
    let first = HISTORY_LEN - history.len();
    let pts: Vec<(i32, i32)> = history.iter().enumerate().map(|(k, v)| (r.x + ((first + k) as f32 * step) as i32, y_of(*v))).collect();
    for pair in pts.windows(2) {
        s.line(pair[0].0, pair[0].1, pair[1].0, pair[1].1, line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, name: &str, cpu: f32, mem_kb: u64) -> Row {
        Row {
            pid,
            ppid: 1,
            name: name.into(),
            user: if pid < 100 { "root".into() } else { "ferro".into() },
            cpu,
            mem_kb,
            mem_limit_kb: if pid == 812 { Some(512 * 1024) } else { None },
            sandboxed: pid > 100,
            depth: 0,
            prefix: String::new(),
        }
    }

    fn sample() -> Vec<Row> {
        vec![row(1, "systemd", 0.0, 12_000), row(640, "Xorg", 3.0, 61_000), row(700, "ferro-wm", 1.0, 2_400), row(812, "steam", 40.0, 912_345)]
    }

    fn state() -> State {
        let mut st = State::default();
        st.update(sample(), Some(44.0), Some((2_000_000, 8_000_000)), Some(3725));
        st
    }

    fn names(st: &State) -> Vec<&str> {
        st.rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn sorts_by_any_column() {
        let mut st = state();
        assert_eq!(names(&st), ["steam", "Xorg", "systemd", "ferro-wm"], "most memory first");
        st.sort_by(0);
        assert_eq!(names(&st), ["ferro-wm", "steam", "systemd", "Xorg"], "by name, capitals not first");
        st.sort_by(0);
        assert_eq!(names(&st), ["Xorg", "systemd", "steam", "ferro-wm"]);
        st.sort_by(COL_CPU);
        assert_eq!(names(&st)[0], "steam");
        st.sort_by(COL_BUDGET);
        assert_eq!(names(&st)[0], "steam");
        // Sorting isn't a new second on the graphs.
        assert_eq!(st.cpu_history.len(), 1);
    }

    #[test]
    fn keeps_a_minute_of_history() {
        let mut st = State::default();
        for i in 0..100 {
            st.update(Vec::new(), Some(i as f32), Some((1, 2)), None);
        }
        assert_eq!(st.cpu_history.len(), HISTORY_LEN);
        assert_eq!(st.cpu_history.back(), Some(&0.99));
        assert_eq!(st.mem_history.back(), Some(&0.5));
    }

    #[test]
    fn the_selection_follows_the_program() {
        let mut st = state();
        st.selected = Some(812);
        st.update(sample(), None, None, None);
        assert_eq!(st.selected, Some(812));
        // It ended: nothing is selected any more.
        st.update(sample().into_iter().filter(|r| r.pid != 812).collect(), None, None, None);
        assert_eq!(st.selected, None);
    }

    #[test]
    fn end_process_spares_the_system() {
        let mut st = state();
        assert!(!st.can_end());
        st.selected = Some(1);
        assert!(!st.can_end());
        st.selected = Some(812);
        assert!(st.can_end());
        st.tab = 1;
        assert!(!st.can_end());
    }

    #[test]
    fn finds_what_was_clicked() {
        let mut st = state();
        let l = layout(W, H, &st);
        assert_eq!(hit(W, H, &st, l.tabs[1].x + 3, l.tabs[1].y + 3), Hit::Tab(1));
        assert_eq!(hit(W, H, &st, l.headers[2].x + 3, l.headers[2].y + 3), Hit::Header(2));
        // The second row holds Xorg.
        assert_eq!(hit(W, H, &st, l.rows.x + 5, l.rows.y + ROW_H + 2), Hit::Row(640));
        assert_eq!(hit(W, H, &st, l.rows.x + 5, l.rows.y + 10 * ROW_H), Hit::Nothing);
        assert_eq!(hit(W, H, &st, l.end_button.x + 3, l.end_button.y + 3), Hit::End);
        assert_eq!(hit(W, H, &st, l.scrollbar.up.x + 3, l.scrollbar.up.y + 3), Hit::ScrollUp);
        // The columns fill the list exactly, and nothing covers the buttons.
        assert_eq!(l.headers[NCOLS - 1].right(), l.rows.right());
        assert!(!l.end_button.overlaps(&l.list) && l.end_button.bottom() < l.status.y);
        st.tab = 1;
        assert_eq!(hit(W, H, &st, l.end_button.x + 3, l.end_button.y + 3), Hit::Nothing);
    }

    #[test]
    fn budget_dialog_modality_and_hits() {
        let mut st = state();
        st.selected = Some(812);
        assert!(st.can_budget());
        st.open_budget_dialog();
        assert!(st.budget_dialog.is_some());
        assert!(!st.can_budget());
        assert!(!st.can_end());

        let l = layout(W, H, &st);
        let ok = l.dialog_ok.unwrap();
        let cancel = l.dialog_cancel.unwrap();
        assert_eq!(hit(W, H, &st, ok.x + 2, ok.y + 2), Hit::BudgetOk);
        assert_eq!(hit(W, H, &st, cancel.x + 2, cancel.y + 2), Hit::BudgetCancel);
        // Clicks on underlying buttons are blocked while dialog is open:
        assert_eq!(hit(W, H, &st, l.end_button.x + 3, l.end_button.y + 3), Hit::Nothing);

        st.close_budget_dialog();
        assert!(st.budget_dialog.is_none());
        assert!(st.can_budget());
    }

    #[test]
    fn arrows_walk_the_list_and_scroll_it() {
        let mut st = State::default();
        st.update((0..100).map(|i| row(100 + i, "p", 0.0, 1000 - u64::from(i))).collect(), None, None, None);
        let visible = layout(W, H, &st).visible;
        st.step(true, W, H);
        assert_eq!(st.selected, Some(100));
        for _ in 0..visible {
            st.step(true, W, H);
        }
        assert_eq!(st.selected, Some(100 + visible as u32));
        assert_eq!(st.scroll, 1, "scrolled just enough to show it");
        st.scroll_by(1000, W, H);
        assert_eq!(st.scroll, 100 - visible);
        st.scroll_by(-1000, W, H);
        assert_eq!(st.scroll, 0);
    }

    #[test]
    fn draws_at_any_size() {
        let mut st = state();
        for (w, h) in [(W, H), (200, 120), (900, 700)] {
            for tab in [0, 1] {
                st.tab = tab;
                let mut s = Surface::new(w as usize, h as usize);
                draw(&mut s, w, h, &st);
            }
        }
        assert_eq!(group_digits(912_345), "912,345");
        assert_eq!(group_digits(7), "7");
    }

    #[test]
    fn context_menu_hits_and_actions() {
        let mut st = state();
        st.context_menu = Some(ContextMenu::for_process(50, 50, 812));
        assert_eq!(hit(W, H, &st, 60, 58), Hit::ContextMenuAction(ProcessAction::EndProcess));
        assert_eq!(hit(W, H, &st, 60, 88), Hit::ContextMenuAction(ProcessAction::SetBudget));
        assert_eq!(hit(W, H, &st, 60, 108), Hit::ContextMenuAction(ProcessAction::RemoveBudget));
        assert_eq!(hit(W, H, &st, 400, 300), Hit::DismissMenu);
    }

    #[test]
    fn performance_tab_and_dialogs_never_hit_rows() {
        let mut st = state();
        // On Processes tab, hitting row 0 returns its PID.
        assert_eq!(hit_row(W, H, &st, 20, 60), Some(812));

        // On Performance tab, hit_row must return None.
        st.tab = 1;
        assert_eq!(hit_row(W, H, &st, 20, 60), None);

        // When budget dialog is open, hit_row must also return None.
        st.tab = 0;
        st.selected = Some(812);
        st.open_budget_dialog();
        assert!(st.budget_dialog.is_some());
        assert_eq!(hit_row(W, H, &st, 20, 60), None);
    }

    #[test]
    fn test_tree_view_nesting() {
        let mut r1 = row(1, "systemd", 0.0, 1000);
        r1.ppid = 0;
        let mut r2 = row(100, "ferro-wm", 1.0, 5000);
        r2.ppid = 1;
        let mut r3 = row(101, "ferro-files", 2.0, 8000);
        r3.ppid = 100;
        let mut r4 = row(102, "geany", 0.5, 4000);
        r4.ppid = 100;

        let mut st = State { tree_view: true, ..State::default() };
        st.update(vec![r1, r2, r3, r4], None, None, None);

        assert_eq!(st.rows.len(), 4);
        assert_eq!(st.rows[0].name, "ferro-wm");
        assert!(st.rows[1].name == "ferro-files" || st.rows[1].name == "geany");
        assert!(st.rows[1].prefix.contains("├─") || st.rows[1].prefix.contains("└─"));
        assert!(st.rows[2].name == "ferro-files" || st.rows[2].name == "geany");
        assert!(st.rows[2].prefix.contains("├─") || st.rows[2].prefix.contains("└─"));
        assert_eq!(st.rows[3].name, "systemd");
    }
}
