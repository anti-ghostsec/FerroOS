//! The progress window: one of FerroOS's scripts (installing, uninstalling,
//! updating) run with a window of its own instead of a terminal. What it's
//! doing, a progress bar while apt works, what it printed under Details,
//! its questions as Windows asks them (Yes/No, a list, a box to type in) and,
//! at the end, how it went. See proto.rs for what the script says to it.
//!
//!   ferro-programs --run TITLE [--root] -- PROGRAM [ARGS...]
//!
//! With --root, PROGRAM runs as root, the password asked for in FerroOS's
//! password box. Without, it runs as you: a script that needs root for a
//! part asks sudo then, through the same box (SUDO_ASKPASS).

use crate::proto::{self, Said};
use crate::win::{self, Win};
use ferro_gfx::{color, ellipsize, icons, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, default_button, text_field, Edit, Key, Res, TextEdit};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::Event;

const ASKPASS: &str = "/usr/local/bin/ferro-askpass";
const W: i32 = 560;
const H: i32 = 420;
const BAR: Rect = Rect::new(16, 62, W - 32, 22);
const LOG: Rect = Rect::new(16, 112, W - 32, 196);
const LOG_ROW: i32 = GLYPH_H + 4;
const BOTTOM_Y: i32 = LOG.y + LOG.h + 12;
const CLOSE_BTN: Rect = Rect::new(W - 91, H - 36, 75, 24);
/// Yes and No (OK and Cancel) beside Close, as a wizard's buttons are.
const YES_BTN: Rect = Rect::new(CLOSE_BTN.x - 2 * 83, CLOSE_BTN.y, 75, 24);
const NO_BTN: Rect = Rect::new(CLOSE_BTN.x - 83, CLOSE_BTN.y, 75, 24);
const FIELD: Rect = Rect::new(16, BOTTOM_Y + 16, W - 32, 22);
/// The most kept for Details (an update can say a great deal).
const KEEP: usize = 4000;

enum Ask {
    YesNo(Vec<String>),
    Choose { title: String, options: Vec<String>, chosen: usize },
    Text { prompt: String, edit: TextEdit, secret: bool },
}

struct State {
    title: String,
    step: String,
    progress: Option<f32>,
    log: Vec<String>,
    /// Lines up from the newest (0: the newest shows).
    scroll: usize,
    ask: Option<Ask>,
    /// Finished: whether it went well, and what to say.
    result: Option<(bool, String)>,
    said_done: Option<String>,
    said_fail: Option<String>,
}

fn log_rows() -> usize {
    ((LOG.h - 6) / LOG_ROW) as usize
}

fn draw(s: &mut Surface, st: &State) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    s.sprite(16, 12, icons::FERRO, 2);
    s.text_bold(60, 16, &ellipsize(&st.title, ((W - 76) / GLYPH_W) as usize), color::BLACK);
    let step = match &st.result {
        Some((true, _)) => "Finished.".to_owned(),
        Some((false, _)) => "Stopped.".to_owned(),
        None if st.ask.is_some() => "Waiting for your answer.".to_owned(),
        None if st.step.is_empty() => "Starting...".to_owned(),
        None => st.step.clone(),
    };
    s.text(60, 34, &ellipsize(&step, ((W - 76) / GLYPH_W) as usize), color::BLACK);
    let fraction = match (&st.result, st.progress) {
        (Some((true, _)), _) => 1.0,
        (_, Some(p)) => p / 100.0,
        _ => 0.0,
    };
    win::progress_bar(s, BAR, fraction);

    s.text(16, LOG.y - 14, "Details:", color::BLACK);
    win::list_box(s, LOG);
    let cols = ((LOG.w - 10) / GLYPH_W) as usize;
    if let Some(Ask::Choose { title, options, chosen }) = &st.ask {
        // The list to choose from, where Details are.
        s.text_bold(LOG.x + 6, LOG.y + 6, &ellipsize(title, cols), color::BLACK);
        for (i, o) in options.iter().enumerate() {
            let r = Rect::new(LOG.x + 3, LOG.y + 22 + i as i32 * 18, LOG.w - 6, 18);
            if r.bottom() > LOG.bottom() - 2 {
                break;
            }
            let ink = if i == *chosen {
                s.fill(r, color::NAVY);
                color::WHITE
            } else {
                color::BLACK
            };
            s.text(r.x + 6, r.y + 5, &ellipsize(o, cols - 1), ink);
        }
    } else {
        let rows = log_rows();
        let end = st.log.len().saturating_sub(st.scroll);
        let start = end.saturating_sub(rows);
        for (n, line) in st.log[start..end].iter().enumerate() {
            s.text(LOG.x + 5, LOG.y + 4 + n as i32 * LOG_ROW, &ellipsize(line, cols), color::BLACK);
        }
    }

    let words = ((W - 32) / GLYPH_W) as usize;
    match (&st.ask, &st.result) {
        (Some(Ask::YesNo(lines)), _) => {
            for (n, l) in lines.iter().take(3).enumerate() {
                s.text(16, BOTTOM_Y + n as i32 * (GLYPH_H + 4), l, color::BLACK);
            }
            default_button(s, YES_BTN, "Yes", true);
            button(s, NO_BTN, "No", true);
        }
        (Some(Ask::Choose { .. }), _) => {
            s.text(16, BOTTOM_Y + 4, "Choose one, then OK.", color::BLACK);
            default_button(s, YES_BTN, "OK", true);
            button(s, NO_BTN, "Cancel", true);
        }
        (Some(Ask::Text { prompt, edit, secret }), _) => {
            s.text(16, BOTTOM_Y + 4, &ellipsize(prompt, words), color::BLACK);
            if *secret {
                let shown: String = "*".repeat(edit.text().chars().count());
                text_field(s, FIELD, &shown, true);
            } else {
                edit.draw(s, FIELD);
                s.bevel(FIELD, ferro_gfx::Bevel::Sunken);
            }
            default_button(s, YES_BTN, "OK", true);
            button(s, NO_BTN, "Cancel", true);
        }
        (None, Some((ok, said))) => {
            for (n, l) in proto::wrap(said, words).iter().take(4).enumerate() {
                s.text_bold(16, BOTTOM_Y + n as i32 * (GLYPH_H + 4), l, if *ok { color::BLACK } else { color::RED });
            }
        }
        (None, None) => {
            s.text(16, BOTTOM_Y, "Please wait. This window closes when you say, once it's done.", color::SHADOW);
        }
    }
    button(s, CLOSE_BTN, "Close", st.result.is_some());
}

/// The program started, its output read a line at a time into `rx`.
fn start(root: bool, title: &str, argv: &[String]) -> std::io::Result<(Child, Receiver<String>)> {
    let mut cmd = if root {
        let mut c = Command::new("/usr/bin/sudo");
        c.args(["-k", "-A", "--"]).args(argv);
        c
    } else {
        let mut c = Command::new(&argv[0]);
        c.args(&argv[1..]);
        c
    };
    cmd.env("SUDO_ASKPASS", ASKPASS)
        .env("FERRO_ADMIN_FOR", title)
        .env("FERRO_GUI", "1")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let (tx, rx) = channel();
    for out in [
        child.stdout.take().map(|o| Box::new(o) as Box<dyn std::io::Read + Send>),
        child.stderr.take().map(|e| Box::new(e) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(out);
            let mut buf = Vec::new();
            while r.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
                buf.clear();
            }
        });
    }
    Ok((child, rx))
}

fn answer(stdin: &mut Option<ChildStdin>, text: &str) {
    if let Some(i) = stdin.as_mut() {
        let _ = writeln!(i, "{text}");
        let _ = i.flush();
    }
}

pub fn run(title: &str, root: bool, argv: &[String]) -> Res<()> {
    let w = Win::open(title, W, H)?;
    let mut st = State {
        title: title.to_owned(),
        step: String::new(),
        progress: None,
        log: Vec::new(),
        scroll: 0,
        ask: None,
        result: None,
        said_done: None,
        said_fail: None,
    };
    let mut s = Surface::new(W as usize, H as usize);
    let (mut child, rx) = match start(root, title, argv) {
        Ok((c, rx)) => (Some(c), rx),
        Err(e) => {
            st.result = Some((false, format!("It couldn't be started: {e}.")));
            (None, channel().1)
        }
    };
    let mut stdin = child.as_mut().and_then(|c| c.stdin.take());
    let mut dirty = true;

    // What a reply to the question shown is, and the question gone.
    macro_rules! reply {
        ($text:expr) => {{
            let mut t: String = $text;
            answer(&mut stdin, &t);
            ferro_ui::forget(&mut t);
            st.ask = None;
            dirty = true;
        }};
    }
    // Yes / OK (true) or No / Cancel (false) on the question shown.
    macro_rules! decide {
        ($yes:expr) => {{
            let yes: bool = $yes;
            match st.ask.take() {
                Some(Ask::YesNo(_)) => reply!(if yes { "y".to_owned() } else { "n".to_owned() }),
                Some(Ask::Choose { chosen, .. }) => reply!(if yes { (chosen + 1).to_string() } else { "0".to_owned() }),
                Some(Ask::Text { mut edit, .. }) => {
                    let t = if yes { format!("={}", edit.text()) } else { "!".to_owned() };
                    edit.select_all();
                    edit.delete();
                    reply!(t)
                }
                None => {}
            }
        }};
    }

    loop {
        while let Some(ev) = w.conn.poll_for_event()? {
            match ev {
                Event::Expose(e) if e.count == 0 => dirty = true,
                Event::MapNotify(_) => w.focus()?,
                Event::ClientMessage(cm) if cm.data.as_data32()[0] == w.delete => {
                    if st.result.is_some() {
                        return Ok(());
                    }
                    // Closed with a question up: as No (the script stops
                    // there, as it does for No).
                    if st.ask.is_some() {
                        decide!(false);
                    }
                }
                Event::ButtonPress(b) => {
                    let (x, y) = (i32::from(b.event_x), i32::from(b.event_y));
                    match b.detail {
                        4 | 5 if st.ask.is_none() => {
                            let most = st.log.len().saturating_sub(log_rows());
                            st.scroll = if b.detail == 4 { (st.scroll + 3).min(most) } else { st.scroll.saturating_sub(3) };
                            dirty = true;
                        }
                        1 if st.result.is_some() && CLOSE_BTN.contains(x, y) => return Ok(()),
                        1 if st.ask.is_some() && YES_BTN.contains(x, y) => decide!(true),
                        1 if st.ask.is_some() && NO_BTN.contains(x, y) => decide!(false),
                        1 => match &mut st.ask {
                            Some(Ask::Choose { options, chosen, .. }) => {
                                let i = (y - LOG.y - 22).div_euclid(18);
                                if LOG.contains(x, y) && i >= 0 && (i as usize) < options.len() {
                                    *chosen = i as usize;
                                    dirty = true;
                                }
                            }
                            Some(Ask::Text { edit, secret: false, .. }) if FIELD.contains(x, y) => {
                                edit.click(FIELD, x, false);
                                dirty = true;
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                Event::KeyPress(k) => {
                    let key = w.keymap.typed(&k);
                    let (ctrl, shift) = win::mods(k.state);
                    match &mut st.ask {
                        Some(Ask::Text { edit, secret, .. }) => {
                            // Hidden typing isn't copied out of the box.
                            let copying = *secret && ctrl && matches!(key, Key::Char('c' | 'C' | 'x' | 'X'));
                            match if copying { Edit::Ignored } else { edit.key(key, ctrl, shift) } {
                                Edit::Changed => dirty = true,
                                Edit::Done => decide!(true),
                                Edit::Cancel => decide!(false),
                                Edit::Ignored => {}
                            }
                        }
                        Some(Ask::Choose { options, chosen, .. }) => match key {
                            Key::Up => {
                                *chosen = chosen.saturating_sub(1);
                                dirty = true;
                            }
                            Key::Down => {
                                *chosen = (*chosen + 1).min(options.len().saturating_sub(1));
                                dirty = true;
                            }
                            Key::Enter => decide!(true),
                            Key::Escape => decide!(false),
                            _ => {}
                        },
                        Some(Ask::YesNo(_)) => match key {
                            Key::Enter | Key::Char('y' | 'Y') => decide!(true),
                            Key::Escape | Key::Char('n' | 'N') => decide!(false),
                            _ => {}
                        },
                        None if st.result.is_some() && matches!(key, Key::Enter | Key::Escape) => return Ok(()),
                        None => {}
                    }
                }
                _ => {}
            }
        }

        while let Ok(line) = rx.try_recv() {
            dirty = true;
            match proto::parse(&line) {
                Said::Step(t) => {
                    st.log.push(t.clone());
                    st.step = t;
                    st.progress = None;
                }
                Said::Progress(p, what) => {
                    st.progress = Some(p);
                    st.step = what;
                }
                Said::YesNo(q) => st.ask = Some(Ask::YesNo(proto::wrap(&q, ((W - 32) / GLYPH_W) as usize))),
                Said::Choose(title, options) if !options.is_empty() => st.ask = Some(Ask::Choose { title, options, chosen: 0 }),
                Said::Choose(..) => answer(&mut stdin, "0"),
                Said::Text { prompt, suggested, secret } => st.ask = Some(Ask::Text { prompt, edit: TextEdit::new(&suggested), secret }),
                Said::Done(t) => st.said_done = Some(t),
                Said::Fail(t) => st.said_fail = Some(t),
                Said::Log(l) if l.is_empty() => {}
                Said::Log(l) => st.log.push(l),
            }
            if st.log.len() > KEEP {
                st.log.drain(..st.log.len() - KEEP);
            }
        }

        if let Some(status) = child.as_mut().and_then(|c| c.try_wait().ok().flatten()) {
            child = None;
            {
                // What it still had to say comes in before the end is told.
                std::thread::sleep(Duration::from_millis(80));
                while let Ok(l) = rx.try_recv() {
                    match proto::parse(&l) {
                        Said::Done(t) => st.said_done = Some(t),
                        Said::Fail(t) => st.said_fail = Some(t),
                        Said::Log(l) if !l.is_empty() => st.log.push(l),
                        _ => {}
                    }
                }
                st.ask = None;
                stdin = None;
                if st.result.is_none() {
                    st.result = Some(match (&st.said_fail, &st.said_done, status.success()) {
                        (Some(f), _, _) => (false, f.clone()),
                        (None, Some(d), true) => (true, d.clone()),
                        (None, None, true) => (true, "Done.".to_owned()),
                        (None, _, false) if st.log.last().is_some_and(|l| l.starts_with("sudo:")) => {
                            (false, "Nothing was done: the password wasn't given.".to_owned())
                        }
                        (None, _, false) => (false, "It didn't finish. What went wrong is under Details.".to_owned()),
                    });
                }
                dirty = true;
            }
        }

        if dirty {
            draw(&mut s, &st);
            w.show(&s)?;
            dirty = false;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Every state drawn, for looking at (`cargo test -- --ignored shots`).
    fn sample(kind: &str) -> State {
        let mut st = State {
            title: "Installing Steam".into(),
            step: "Installing libvulkan1:i386".into(),
            progress: Some(42.0),
            log: (1..40).map(|i| format!("Unpacking libfoo{i} (1.2.{i}) ...")).collect(),
            scroll: 0,
            ask: None,
            result: None,
            said_done: None,
            said_fail: None,
        };
        match kind {
            "yesno" => st.ask = Some(Ask::YesNo(proto::wrap("Steam needs you to accept Valve's Steam Subscriber Agreement (store.steampowered.com/subscriber_agreement). Accept it and install Steam?", 66))),
            "choose" => {
                st.ask = Some(Ask::Choose {
                    title: "Which Start menu folder should it go in?".into(),
                    options: ["Let the program decide", "Accessories", "Internet", "Multimedia", "System Tools", "Development", "Games"].map(String::from).to_vec(),
                    chosen: 6,
                })
            }
            "secret" => st.ask = Some(Ask::Text { prompt: "Choose a one-time password (you type it again after restarting):".into(), edit: TextEdit::new("hunter2"), secret: true }),
            "done" => st.result = Some((true, "Steam is installed. It's in the Start menu, under Programs > Games.".into())),
            "fail" => st.result = Some((false, "It didn't finish. What went wrong is under Details.".into())),
            _ => {}
        }
        st
    }

    pub(crate) fn shot(kind: &str) -> Surface<'static> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, &sample(kind));
        s
    }

    #[test]
    fn fits() {
        assert!(BOTTOM_Y + 3 * (GLYPH_H + 4) < CLOSE_BTN.y && FIELD.bottom() < CLOSE_BTN.y);
        assert!(YES_BTN.x > 16 + 60 * GLYPH_W / 2 && NO_BTN.right() < CLOSE_BTN.x);
        assert!(log_rows() >= 10);
    }
}
