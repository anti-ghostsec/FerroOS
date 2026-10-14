//! ferro-screensaver: Retro Windows 95 Screensavers (Starfield, Matrix, Blank).

mod matrix;
mod stars;

use ferro_gfx::Surface;
use ferro_ui::Res;
use matrix::MatrixRain;
use stars::Starfield;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Stars,
    Matrix,
    Blank,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut mode = Mode::Stars;
    let mut lock = false;
    let mut preview_size: Option<(i32, i32)> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--matrix" | "matrix" => mode = Mode::Matrix,
            "--blank" | "blank" => mode = Mode::Blank,
            "--stars" | "stars" => mode = Mode::Stars,
            "--lock" => lock = true,
            "--preview" if i + 2 < args.len() => {
                if let (Ok(pw), Ok(ph)) = (args[i + 1].parse::<i32>(), args[i + 2].parse::<i32>()) {
                    preview_size = Some((pw, ph));
                    i += 2;
                }
            }
            _ => {}
        }
        i += 1;
    }

    if let Err(e) = run(mode, lock, preview_size) {
        eprintln!("ferro-screensaver: {e}");
    }
}

fn run(mode: Mode, lock: bool, preview: Option<(i32, i32)>) -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];

    let (w, h) = preview.unwrap_or((screen.width_in_pixels as i32, screen.height_in_pixels as i32));

    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;

    let values = CreateWindowAux::new()
        .background_pixel(screen.black_pixel)
        .override_redirect(if preview.is_none() { 1 } else { 0 })
        .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::POINTER_MOTION);

    conn.create_window(screen.root_depth, win, screen.root, 0, 0, w as u16, h as u16, 0, WindowClass::INPUT_OUTPUT, screen.root_visual, &values)?;

    let title = "Screensaver";
    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;

    conn.create_gc(gc, win, &CreateGCAux::new())?;
    conn.map_window(win)?;
    conn.flush()?;

    let mut starfield = Starfield::default();
    let mut matrix_rain = MatrixRain::new(w);

    let mut surface = Surface::new(w as usize, h as usize);

    let mut initial_pos: Option<(i32, i32)> = None;

    loop {
        // Poll for input events
        while let Some(event) = conn.poll_for_event()? {
            match event {
                Event::ClientMessage(_) => return Ok(()),
                Event::KeyPress(_) | Event::ButtonPress(_) => {
                    if preview.is_none() {
                        wake_up(lock);
                        return Ok(());
                    }
                }
                Event::MotionNotify(e) if preview.is_none() => {
                    let (mx, my) = (i32::from(e.event_x), i32::from(e.event_y));
                    if let Some((ix, iy)) = initial_pos {
                        if (mx - ix).abs() > 15 || (my - iy).abs() > 15 {
                            wake_up(lock);
                            return Ok(());
                        }
                    } else {
                        initial_pos = Some((mx, my));
                    }
                }
                _ => {}
            }
        }

        // Render frame
        match mode {
            Mode::Stars => {
                starfield.tick();
                starfield.draw(&mut surface, w, h);
            }
            Mode::Matrix => {
                matrix_rain.tick(h);
                matrix_rain.draw(&mut surface, w, h);
            }
            Mode::Blank => {
                surface.fill(ferro_gfx::Rect::new(0, 0, w, h), ferro_gfx::color::BLACK);
            }
        }

        ferro_ui::show(&conn, win, gc, screen.root_depth, &surface, w, h)?;
        conn.flush()?;

        sleep(Duration::from_millis(33)); // ~30 fps
    }
}

fn wake_up(lock: bool) {
    if lock {
        let _ = Command::new("ferro-logon").args(["/usr/local/lib/ferro-heavy/check-password", "/tmp/lock-unlock"]).status();
    }
}
