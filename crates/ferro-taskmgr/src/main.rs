//! ferro-taskmgr: FerroOS Heavy's Task Manager. What's running, how much
//! of the processor and memory each program takes, the last minute of both
//! as graphs, and End Process.
//!
//! It reads /proc once a second while its window is open (ferro-sys does
//! the reading, the same code FerroOS's own Task Manager uses) and nothing
//! when it isn't. Ending a process asks it to stop (SIGTERM), and works on
//! your own programs; the system's need `sudo kill` in a Command Prompt.
//!
//! Keys: the arrows walk the list, Tab changes tab, Esc closes.
//!
//! Like ferro-wm it speaks the X11 protocol itself: one static binary.

mod view;

use ferro_gfx::Surface;
use ferro_sys::{MemInfo, Sampler};
use ferro_ui::{Key, Keymap, Res};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use view::{ContextMenu, Hit, ProcessAction, Row, State};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// Account names by number, from the system's list of accounts.
fn user_names() -> HashMap<u32, String> {
    let text = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    parse_passwd(&text)
}

fn parse_passwd(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split(':');
            let name = f.next()?;
            Some((f.nth(1)?.parse().ok()?, name.to_owned()))
        })
        .collect()
}

/// Asks a process to stop. Says why not when it can't.
fn end_process(pid: u32) -> &'static str {
    #[cfg(unix)]
    {
        // SAFETY: kill takes two integers and touches no memory of ours.
        if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0 {
            return "";
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EPERM) => "That one belongs to the system or another account.",
            _ => "It had already ended.",
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        "Ending processes needs Linux."
    }
}

/// Sets a RAM budget on a process via ferro-system broker (or direct cgroup fallback).
fn apply_budget(pid: u32, bytes: Option<u64>) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};
        let b_str = bytes.map_or("none".to_string(), |b| b.to_string());
        if let Ok(mut stream) = std::os::unix::net::UnixStream::connect("/run/ferro/system.sock") {
            if writeln!(stream, "LIMIT {pid} {b_str}").is_ok() {
                let mut reply = String::new();
                if BufReader::new(stream).read_line(&mut reply).is_ok() {
                    let trimmed = reply.trim();
                    if trimmed == "OK" {
                        return Ok(());
                    } else if let Some(err) = trimmed.strip_prefix("ERR ") {
                        return Err(err.to_string());
                    }
                }
            }
        }
        ferro_sandbox::linux::limit_process(pid, bytes).map_err(|e| format!("Could not apply: {e}"))
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, bytes);
        Err("RAM budgets need Linux.".into())
    }
}

struct Window {
    conn: RustConnection,
    win: u32,
    gc: u32,
    depth: u8,
    w: i32,
    h: i32,
    state: State,
    sampler: Sampler,
    users: HashMap<u32, String>,
}

impl Window {
    fn paint(&self) -> Res<()> {
        let mut s = Surface::new(self.w as usize, self.h as usize);
        view::draw(&mut s, self.w, self.h, &self.state);
        ferro_ui::show(&self.conn, self.win, self.gc, self.depth, &s, self.w, self.h)
    }

    /// Reads what's running now.
    fn sample(&mut self) {
        let (cpu, procs) = self.sampler.sample(true);
        let rows = procs
            .unwrap_or_default()
            .into_iter()
            .map(|p| Row {
                pid: p.pid,
                user: self.users.get(&p.uid).cloned().unwrap_or_else(|| p.uid.to_string()),
                name: p.name,
                cpu: p.cpu_percent,
                mem_kb: p.rss_kb,
                mem_limit_kb: p.mem_limit_kb,
                sandboxed: p.sandboxed,
            })
            .collect();
        let mem = MemInfo::read().map(|m| (m.footprint_kb(), m.total_kb));
        self.state.update(rows, cpu, mem, ferro_sys::uptime_secs());
    }

    fn end_selected(&mut self) {
        if let (true, Some(pid)) = (self.state.can_end(), self.state.selected) {
            self.state.message = end_process(pid);
        }
    }

    fn submit_budget(&mut self) {
        let Some(dialog) = &self.state.budget_dialog else { return };
        let text = dialog.text.trim();
        let Some(parsed) = ferro_sandbox::parse_budget(text) else {
            if let Some(d) = &mut self.state.budget_dialog {
                d.error = Some("Use e.g. 512M, 2G, or none".into());
            }
            return;
        };
        if parsed.is_some_and(|b| b < 4 << 20) {
            if let Some(d) = &mut self.state.budget_dialog {
                d.error = Some("Minimum is 4 MB".into());
            }
            return;
        }
        let pid = dialog.pid;
        match apply_budget(pid, parsed) {
            Ok(()) => {
                self.state.budget_dialog = None;
                self.state.message = "RAM budget updated.";
                self.sample();
            }
            Err(e) => {
                if let Some(d) = &mut self.state.budget_dialog {
                    d.error = Some(e);
                }
            }
        }
    }

    fn click(&mut self, e: &ButtonPressEvent) {
        let (w, h) = (self.w, self.h);
        let st = &mut self.state;
        match e.detail {
            // The wheel.
            4 => st.scroll_by(-3, w, h),
            5 => st.scroll_by(3, w, h),
            1 => {
                if st.context_menu.is_some() {
                    match view::hit(w, h, st, i32::from(e.event_x), i32::from(e.event_y)) {
                        Hit::ContextMenuAction(action) => {
                            st.context_menu = None;
                            match action {
                                ProcessAction::EndProcess => self.end_selected(),
                                ProcessAction::SetBudget => st.open_budget_dialog(),
                                ProcessAction::RemoveBudget => {
                                    if let Some(pid) = st.selected {
                                        let _ = apply_budget(pid, None);
                                        st.message = "RAM budget removed.";
                                        self.sample();
                                    }
                                }
                            }
                        }
                        _ => {
                            st.context_menu = None;
                        }
                    }
                    return;
                }
                match view::hit(w, h, st, i32::from(e.event_x), i32::from(e.event_y)) {
                    Hit::Tab(i) => st.tab = i,
                    Hit::Header(col) => st.sort_by(col),
                    Hit::Row(pid) => {
                        st.selected = Some(pid);
                        st.message = "";
                    }
                    Hit::BudgetOk => self.submit_budget(),
                    Hit::BudgetCancel => st.close_budget_dialog(),
                    Hit::End => self.end_selected(),
                    Hit::ScrollUp => st.scroll_by(-1, w, h),
                    Hit::ScrollDown => st.scroll_by(1, w, h),
                    Hit::Page(down) => {
                        let page = view::page(w, h, st);
                        st.scroll_by(if down { page } else { -page }, w, h);
                    }
                    _ => {}
                }
            }
            3 => {
                let (ex, ey) = (i32::from(e.event_x), i32::from(e.event_y));
                if let Some(pid) = view::hit_row(w, h, st, ex, ey) {
                    st.selected = Some(pid);
                    st.context_menu = Some(ContextMenu::for_process(ex, ey, pid));
                } else {
                    st.context_menu = None;
                }
            }
            _ => {}
        }
    }

    /// `false` means the window is finished with.
    fn key(&mut self, key: Key) -> bool {
        let (w, h) = (self.w, self.h);
        if self.state.context_menu.is_some() && key == Key::Escape {
            self.state.context_menu = None;
            return true;
        }
        if self.state.budget_dialog.is_some() {
            match key {
                Key::Escape => self.state.close_budget_dialog(),
                Key::Enter => self.submit_budget(),
                Key::Backspace => {
                    if let Some(d) = &mut self.state.budget_dialog {
                        d.text.pop();
                        d.error = None;
                    }
                }
                Key::Char(c) if c.is_ascii_alphanumeric() || c == '.' || c == ' ' => {
                    if let Some(d) = &mut self.state.budget_dialog {
                        if d.text.len() < 16 {
                            d.text.push(c);
                            d.error = None;
                        }
                    }
                }
                _ => {}
            }
            return true;
        }
        match key {
            Key::Escape => return false,
            Key::Next => self.state.tab = 1 - self.state.tab.min(1),
            Key::Up => self.state.step(false, w, h),
            Key::Down => self.state.step(true, w, h),
            Key::Char('b') | Key::Char('B') => self.state.open_budget_dialog(),
            Key::Char('e') | Key::Char('E') => self.end_selected(),
            _ => {}
        }
        true
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ferro-taskmgr: {e}");
        std::process::exit(1);
    }
}

fn run() -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = conn.setup().roots[screen_num].clone();
    let win = conn.generate_id()?;
    let (w, h) = (view::W, view::H);
    let events = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY;
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        0,
        0,
        w as u16,
        h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().event_mask(events),
    )?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &str| -> Res<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom("WM_PROTOCOLS")?, atom("WM_DELETE_WINDOW")?);
    let (net_wm_name, utf8_string) = (atom("_NET_WM_NAME")?, atom("UTF8_STRING")?);
    // The window manager's Close button asks us to close, instead of
    // cutting the connection.
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    let title = "Task Manager";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, net_wm_name, utf8_string, title.as_bytes())?;
    let keys = Keymap::load(&conn)?;
    conn.map_window(win)?;

    let mut window =
        Window { conn, win, gc, depth: screen.root_depth, w, h, state: State::default(), sampler: Sampler::default(), users: user_names() };
    window.sample();
    window.paint()?;
    let mut sampled = Instant::now();
    loop {
        let Some(event) = window.conn.poll_for_event()? else {
            if sampled.elapsed() >= Duration::from_secs(1) {
                sampled = Instant::now();
                window.sample();
                window.paint()?;
            }
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        match event {
            Event::Expose(e) if e.count == 0 => {}
            Event::ConfigureNotify(e) => {
                let size = (i32::from(e.width).max(1), i32::from(e.height).max(1));
                if size == (window.w, window.h) {
                    continue;
                }
                (window.w, window.h) = size;
            }
            Event::ButtonPress(e) => window.click(&e),
            Event::KeyPress(e) => {
                if !window.key(keys.typed(&e)) {
                    return Ok(());
                }
            }
            Event::ClientMessage(e) if e.type_ == wm_protocols && e.data.as_data32()[0] == wm_delete => return Ok(()),
            _ => continue,
        }
        window.paint()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_account_names() {
        let users = parse_passwd("root:x:0:0:root:/root:/bin/bash\nferro:x:1000:1000::/home/ferro:/bin/bash\nbroken line\n");
        assert_eq!(users.get(&0).map(String::as_str), Some("root"));
        assert_eq!(users.get(&1000).map(String::as_str), Some("ferro"));
        assert_eq!(users.len(), 2);
    }
}
