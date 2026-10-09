//! Task Manager: a Processes tab (sortable list, End Process) and a
//! Performance tab (CPU/memory meters and 60-second history graphs).
//!
//! Memory cost is deliberately tiny: the process list is only gathered by the
//! backend while this window is visible, and the graphs are two fixed
//! 60-sample ring buffers owned by the shell.

use super::*;
use ferro_gfx::{Scrollbar, SCROLLBAR_W};

const ROW_H: i32 = 14;
const HEADER_H: i32 = 18;
const COLUMNS: [(&str, i32); 5] = [("Image Name", 0), ("PID", 48), ("CPU", 40), ("Mem Usage", 88), ("RAM Budget", 96)];
const NCOLS: usize = COLUMNS.len();

// Item indices returned by `tm_items`. Hidden items get an empty rect so
// indices stay stable across tabs.
const TAB0: usize = 0;
const HEADER0: usize = 2;
pub(crate) const END_BUTTON: usize = HEADER0 + NCOLS;
const SCROLL_UP: usize = END_BUTTON + 1;
const SCROLL_DOWN: usize = END_BUTTON + 2;
const SCROLL_TRACK: usize = END_BUTTON + 3;
pub(crate) const ROW0: usize = END_BUTTON + 4;

const GRAPH_BG: u32 = 0x000000;
const GRID: u32 = 0x008040;
const LIT: u32 = 0x00FF00;
const UNLIT: u32 = 0x006000;
const MEM_LINE: u32 = 0xFFFF00;
const BUDGET_LINE: u32 = 0xFF4040;

#[derive(Clone, Debug)]
pub(crate) struct TaskState {
    tab: usize,
    sort_col: usize,
    sort_desc: bool,
    scroll: usize,
    selected_pid: Option<u32>,
}

impl Default for TaskState {
    fn default() -> Self {
        // Biggest memory users first: the view that matters for a 50 MB OS.
        Self { tab: 0, sort_col: 3, sort_desc: true, scroll: 0, selected_pid: None }
    }
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
    end_button: Rect,
    status: Rect,
}

impl Shell {
    pub fn open_task_manager(&mut self) {
        if !self.focus_existing(|c| matches!(c, Content::TaskManager(_))) {
            let content = Content::TaskManager(TaskState::default());
            self.open("FerroOS Task Manager".into(), icons::TASKMGR, (520, 420), content);
        }
    }

    /// Switches the Task Manager to tab 0 (Processes) or 1 (Performance).
    pub fn task_manager_tab(&mut self, tab: usize) {
        if let Some(st) = self.windows.iter_mut().find_map(|w| match &mut w.content {
            Content::TaskManager(st) => Some(st),
            _ => None,
        }) {
            st.tab = tab.min(1);
        }
    }

    pub(crate) fn tm_rows(&self, st: &TaskState) -> Vec<&ProcInfo> {
        let Some(procs) = &self.info.processes else { return Vec::new() };
        let mut rows: Vec<&ProcInfo> = procs.iter().collect();
        rows.sort_by(|a, b| {
            let o = match st.sort_col {
                0 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                1 => a.pid.cmp(&b.pid),
                2 => a.cpu_percent.total_cmp(&b.cpu_percent),
                3 => a.rss_kb.cmp(&b.rss_kb),
                _ => (a.sandboxed, a.mem_limit_kb).cmp(&(b.sandboxed, b.mem_limit_kb)),
            };
            if st.sort_desc {
                o.reverse()
            } else {
                o
            }
        });
        rows
    }

    fn tm_layout(&self, win: &Window, st: &TaskState) -> Layout {
        let c = client_rect(win.rect);
        let tabs = [Rect::new(c.x + 4, c.y + 2, 84, 20), Rect::new(c.x + 88, c.y + 2, 104, 20)];
        let status = Rect::new(c.x, c.bottom() - 20, c.w, 20);
        let panel = Rect::new(c.x + 2, c.y + 20, c.w - 4, status.y - 4 - (c.y + 20));
        let p = panel.inset(8);
        let end_button = Rect::new(p.right() - 100, p.bottom() - 23, 100, 23);
        let list = Rect::new(p.x, p.y, p.w, p.h - 31);
        let inner = list.inset(2);

        let fixed: i32 = COLUMNS.iter().map(|c| c.1).sum();
        let mut headers = [Rect::default(); NCOLS];
        let mut x = inner.x;
        for (k, (_, w)) in COLUMNS.iter().enumerate() {
            let w = if k == 0 { inner.w - SCROLLBAR_W - fixed } else { *w };
            headers[k] = Rect::new(x, inner.y, w, HEADER_H);
            x += w;
        }
        let rows = Rect::new(inner.x, inner.y + HEADER_H, inner.w - SCROLLBAR_W, inner.h - HEADER_H);
        let visible = (rows.h / ROW_H).max(1) as usize;
        let total = self.info.processes.as_ref().map_or(0, Vec::len);
        let scroll = st.scroll.min(total.saturating_sub(visible));
        let sb_rect = Rect::new(rows.right(), rows.y, SCROLLBAR_W, rows.h);
        let scrollbar = Scrollbar::layout(sb_rect, scroll, visible, total);
        Layout { tabs, panel, list, headers, rows, visible, scroll, scrollbar, end_button, status }
    }

    pub(crate) fn tm_items(&self, win: &Window, st: &TaskState) -> Vec<Rect> {
        let l = self.tm_layout(win, st);
        let mut v = vec![l.tabs[0], l.tabs[1]];
        if st.tab != 0 {
            v.resize(ROW0, Rect::default());
            return v;
        }
        v.extend(l.headers);
        v.extend([l.end_button, l.scrollbar.up, l.scrollbar.down, l.scrollbar.track]);
        let n = self.info.processes.as_ref().map_or(0, Vec::len).saturating_sub(l.scroll).min(l.visible);
        v.extend((0..n as i32).map(|k| Rect::new(l.rows.x, l.rows.y + k * ROW_H, l.rows.w, ROW_H)));
        v
    }

    fn tm_state(&self, id: u32) -> Option<&TaskState> {
        match &self.window(id)?.content {
            Content::TaskManager(st) => Some(st),
            _ => None,
        }
    }

    fn tm_state_mut(&mut self, id: u32) -> Option<&mut TaskState> {
        match &mut self.window_mut(id)?.content {
            Content::TaskManager(st) => Some(st),
            _ => None,
        }
    }

    /// PID shown in list row item `i` of Task Manager window `id`.
    pub(crate) fn tm_pid_at(&self, id: u32, i: usize) -> Option<u32> {
        let st = self.tm_state(id)?;
        if st.tab != 0 || i < ROW0 {
            return None;
        }
        let l = self.tm_layout(self.window(id)?, st);
        self.tm_rows(st).get(l.scroll + i - ROW0).map(|p| p.pid)
    }

    pub(crate) fn tm_select(&mut self, id: u32, pid: u32) {
        if let Some(st) = self.tm_state_mut(id) {
            st.selected_pid = Some(pid);
        }
    }

    pub(crate) fn tm_press(&mut self, id: u32, i: usize) {
        let pid = self.tm_pid_at(id, i);
        let (cursor_y, total) = (self.cursor.1, self.info.processes.as_ref().map_or(0, Vec::len));
        let Some(win) = self.window(id) else { return };
        let Content::TaskManager(st) = &win.content else { return };
        let l = self.tm_layout(win, st);
        let Some(st) = self.tm_state_mut(id) else { return };
        let max_scroll = total.saturating_sub(l.visible);
        st.scroll = l.scroll;
        match i {
            TAB0 | 1 => st.tab = i,
            i if (HEADER0..END_BUTTON).contains(&i) => {
                let col = i - HEADER0;
                if st.sort_col == col {
                    st.sort_desc = !st.sort_desc;
                } else {
                    st.sort_col = col;
                    st.sort_desc = col >= 2; // numbers biggest-first, text A-Z
                }
            }
            SCROLL_UP => st.scroll = st.scroll.saturating_sub(1),
            SCROLL_DOWN => st.scroll = (st.scroll + 1).min(max_scroll),
            SCROLL_TRACK => {
                st.scroll = if cursor_y < l.scrollbar.thumb.y { st.scroll.saturating_sub(l.visible) } else { (st.scroll + l.visible).min(max_scroll) }
            }
            _ => {
                if let Some(pid) = pid {
                    st.selected_pid = Some(pid);
                }
            }
        }
    }

    pub(crate) fn tm_click(&mut self, id: u32, i: usize) -> Option<Action> {
        let st = self.tm_state(id)?;
        match (i, st.selected_pid) {
            (END_BUTTON, Some(pid)) if pid > 1 && st.tab == 0 => Some(Action::EndProcess(pid)),
            _ => None,
        }
    }

    pub(crate) fn draw_task_manager(&self, s: &mut Surface, win: &Window, st: &TaskState, items: &[Rect]) {
        let l = self.tm_layout(win, st);

        // Tab control: the selected tab is taller and merges into the panel.
        s.fill(l.panel, FACE);
        s.bevel(l.panel, Bevel::Window);
        for (k, (tab, label)) in l.tabs.iter().zip(["Processes", "Performance"]).enumerate() {
            let on = st.tab == k;
            let r = if on { Rect::new(tab.x - 2, tab.y - 2, tab.w + 4, tab.h + 3) } else { *tab };
            s.fill(r, FACE);
            s.hline(r.x + 1, r.y, r.w - 2, WHITE);
            s.vline(r.x, r.y + 1, r.h - 1, WHITE);
            s.vline(r.right() - 1, r.y + 1, r.h - 1, BLACK);
            s.vline(r.right() - 2, r.y + 1, r.h - 1, SHADOW);
            s.text(r.x + (r.w - text_width(label)) / 2, r.y + 7, label, BLACK);
        }

        if st.tab == 0 {
            self.draw_process_tab(s, win, st, &l, items);
        } else {
            self.draw_performance_tab(s, l.panel.inset(10));
        }

        // Status bar: three sunken fields, like NT's.
        let procs = self.info.processes.as_ref().map(Vec::len);
        let cpu = self.info.cpu_percent.map_or("--".into(), |c| format!("{:.0}%", c));
        let mem = match (self.info.mem_used_kb, self.info.mem_total_kb) {
            (Some(u), Some(t)) => format!("{}M / {}M", mb(u), mb(t)),
            _ => "n/a".into(),
        };
        let fields = [format!("Processes: {}", procs.map_or("--".into(), |n| n.to_string())), format!("CPU: {cpu}"), format!("Mem: {mem}")];
        let fw = l.status.w / 3;
        for (k, text) in fields.iter().enumerate() {
            let r = Rect::new(l.status.x + k as i32 * fw, l.status.y, fw - 2, l.status.h);
            s.bevel(r, Bevel::Field);
            s.text(r.x + 4, r.y + 6, &ellipsize(text, ((r.w - 8) / 8) as usize), BLACK);
        }
    }

    fn draw_process_tab(&self, s: &mut Surface, win: &Window, st: &TaskState, l: &Layout, items: &[Rect]) {
        s.fill(l.list, WHITE);
        s.bevel(l.list, Bevel::Sunken);
        for (k, (h, (label, _))) in l.headers.iter().zip(COLUMNS).enumerate() {
            s.button(*h, self.is_held(Hit::Item(win.id, HEADER0 + k), *h));
            let shown = ellipsize(label, ((h.w - 8) / 8).max(0) as usize);
            if st.sort_col == k {
                s.text_bold(h.x + 4, h.y + 5, &shown, BLACK);
            } else {
                s.text(h.x + 4, h.y + 5, &shown, BLACK);
            }
        }
        // Blank header cell above the scrollbar.
        s.button(Rect::new(l.rows.right(), l.headers[0].y, SCROLLBAR_W, HEADER_H), false);

        if self.info.processes.is_none() {
            s.text(l.rows.x + 6, l.rows.y + 8, "Process list needs /proc", SHADOW);
        }
        let rows = self.tm_rows(st);
        for (k, p) in rows.iter().skip(l.scroll).take(l.visible).enumerate() {
            let y = l.rows.y + k as i32 * ROW_H;
            let selected = st.selected_pid == Some(p.pid);
            let fg = if selected { WHITE } else { BLACK };
            if selected {
                s.fill(Rect::new(l.rows.x, y, l.rows.w, ROW_H), NAVY);
            }
            let name_w = ((l.headers[0].w - 8) / 8) as usize;
            s.text(l.headers[0].x + 4, y + 3, &ellipsize(&p.name, name_w), fg);
            let cells = [
                p.pid.to_string(),
                format!("{:02.0}", p.cpu_percent.min(99.0)),
                format!("{} K", group_digits(p.rss_kb)),
                // Sandboxed apps always show their budget; "none" means
                // sandboxed but unlimited; "-" means not sandboxed.
                match (p.mem_limit_kb, p.sandboxed) {
                    (Some(kb), _) => format!("{} MB", kb / 1024),
                    (None, true) => "none".into(),
                    (None, false) => "-".into(),
                },
            ];
            for (h, text) in l.headers[1..].iter().zip(cells) {
                s.text(h.right() - 6 - text_width(&text), y + 3, &text, fg);
            }
        }

        s.scrollbar(
            &l.scrollbar,
            self.is_held(Hit::Item(win.id, SCROLL_UP), l.scrollbar.up),
            self.is_held(Hit::Item(win.id, SCROLL_DOWN), l.scrollbar.down),
        );

        let enabled = st.selected_pid.is_some_and(|p| p > 1);
        let b = items[END_BUTTON];
        s.button(b, enabled && self.is_held(Hit::Item(win.id, END_BUTTON), b));
        let (tx, ty) = (b.x + (b.w - text_width("End Process")) / 2, b.y + 8);
        if enabled {
            s.text(tx, ty, "End Process", BLACK);
        } else {
            s.text_disabled(tx, ty, "End Process");
        }
    }

    fn draw_performance_tab(&self, s: &mut Surface, p: Rect) {
        let row_h = (p.h - 44) / 2;
        let cpu = self.info.cpu_percent.map(|c| c / 100.0);
        let mem = match (self.info.mem_used_kb, self.info.mem_total_kb) {
            (Some(u), Some(t)) => Some((u, t)),
            _ => None,
        };
        let budget = mem.map(|(_, t)| IDLE_BUDGET_KB as f32 / t.max(1) as f32);

        for (k, title) in [("CPU Usage", "CPU Usage History"), ("MEM Usage", "Memory Usage History")].iter().enumerate() {
            let y = p.y + 6 + k as i32 * (row_h + 8);
            let meter_box = Rect::new(p.x, y, 92, row_h);
            let graph_box = Rect::new(p.x + 100, y, p.w - 100, row_h);
            s.group_box(meter_box, title.0);
            s.group_box(graph_box, title.1);
            let (frac, label) = if k == 0 {
                (cpu, cpu.map(|c| format!("{:.0} %", c * 100.0)))
            } else {
                (mem.map(|(u, t)| u as f32 / t.max(1) as f32), mem.map(|(u, _)| format!("{} MB", mb(u))))
            };
            draw_meter(s, meter_box.inset(10), frac.unwrap_or(0.0), label.as_deref().unwrap_or("n/a"));
            let (history, color, line) = if k == 0 { (&self.cpu_history, LIT, None) } else { (&self.mem_history, MEM_LINE, budget) };
            draw_graph(s, graph_box.inset(10), history, color, line);
        }

        let ty = p.bottom() - 30;
        let uptime = self.info.uptime_secs.map_or("n/a".into(), |u| format!("{}:{:02}:{:02}", u / 3600, u / 60 % 60, u % 60));
        let procs = self.info.processes.as_ref().map_or("--".into(), |p| p.len().to_string());
        s.text(p.x, ty, &format!("Processes: {procs}    Up time: {uptime}"), BLACK);
        let mem_line = match mem {
            Some((u, t)) => format!("Physical: {} of {} MB used, budget {} MB", mb(u), mb(t), IDLE_BUDGET_KB / 1024),
            None => "Physical memory: n/a on this host".into(),
        };
        s.text(p.x, ty + 14, &ellipsize(&mem_line, (p.w / 8) as usize), BLACK);
    }
}

/// Kilobytes to whole megabytes, rounded the same way as the taskbar tray.
fn mb(kb: u64) -> u64 {
    (kb + 512) / 1024
}

/// NT-style LED bar meter: two columns of segments lit from the bottom.
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

/// Scrolling line graph, newest sample at the right edge.
fn draw_graph(s: &mut Surface, r: Rect, history: &VecDeque<f32>, color: u32, marker: Option<f32>) {
    s.fill(r, GRAPH_BG);
    for gx in (r.x..r.right()).step_by(12) {
        s.vline(gx, r.y, r.h, GRID);
    }
    for gy in (r.y..r.bottom()).step_by(12) {
        s.hline(r.x, gy, r.w, GRID);
    }
    let y_of = |v: f32| r.bottom() - 1 - (v.clamp(0.0, 1.0) * (r.h - 1) as f32) as i32;
    if let Some(m) = marker.filter(|m| *m <= 1.0) {
        let y = y_of(m);
        for x in (r.x..r.right()).step_by(6) {
            s.hline(x, y, 3, BUDGET_LINE);
        }
    }
    let step = (r.w - 1) as f32 / (HISTORY_LEN - 1) as f32;
    let first = HISTORY_LEN - history.len();
    let pts: Vec<(i32, i32)> = history.iter().enumerate().map(|(k, v)| (r.x + ((first + k) as f32 * step) as i32, y_of(*v))).collect();
    for w in pts.windows(2) {
        s.line(w[0].0, w[0].1, w[1].0, w[1].1, color);
    }
}
