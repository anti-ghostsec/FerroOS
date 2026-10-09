//! Desktop preview of the FerroOS shell.
//!
//!     cargo run -p ferro-preview                                 # interactive window
//!     cargo run -p ferro-preview -- --screenshot <scene> out.png # render a demo frame
//!
//! Scenes: desktop, processes, performance, terminal, budgets, privacy. Screenshots use made-up demo
//! numbers; the interactive window shows this host's real processes.

use ferro_gfx::Surface;
use ferro_path::DriveTable;
use ferro_shell::{Action, Event, Key, Mods, MouseButton, Shell, SystemInfo};
use ferro_sys::ProcInfo;
use minifb::{MouseMode, Window, WindowOptions};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use sysinfo::{ProcessesToUpdate, System};

mod session;

const W: usize = 800;
const H: usize = 600;

fn drives() -> DriveTable {
    // Show the host's real C: drive so the explorer has something to browse.
    let mut t = DriveTable::empty();
    t.mount('C', if cfg!(windows) { "C:/" } else { "/" });
    t
}

/// Real host numbers, so Task Manager can be exercised with hundreds of processes.
fn host_info(sys: &mut System, with_processes: bool) -> SystemInfo {
    sys.refresh_memory();
    sys.refresh_cpu_usage();
    let processes = with_processes.then(|| {
        sys.refresh_processes(ProcessesToUpdate::All, true);
        let cores = sys.cpus().len().max(1) as f32;
        sys.processes()
            .iter()
            .map(|(pid, p)| ProcInfo {
                pid: pid.as_u32(),
                name: p.name().to_string_lossy().into_owned(),
                rss_kb: p.memory() / 1024,
                cpu_ticks: 0,
                cpu_percent: p.cpu_usage() / cores,
                ..ProcInfo::default()
            })
            .collect()
    });
    SystemInfo {
        clock: ferro_sys::clock_12h(),
        mem_used_kb: Some(sys.used_memory() / 1024),
        mem_total_kb: Some(sys.total_memory() / 1024),
        kernel: None,
        cpu_percent: Some(sys.global_cpu_usage()),
        uptime_secs: Some(System::uptime()),
        processes,
        net: Default::default(),
        install_media: false,
        setup_status: String::new(),
    }
}

/// Plausible FerroOS-under-QEMU numbers for screenshots. Not measurements.
fn demo_info(tick: usize) -> SystemInfo {
    // (pid, name, RSS KB, sandboxed, budget MB)
    let demo = [
        (1, "init", 620, false, None),
        (36, "ferro-shell", 700, false, None),
        (37, "ferro-cmd", 510, false, None),
        (52, "ferro-run", 290, false, None),
        (53, "editor", 6_400, true, Some(256)),
        (61, "ferro-run", 290, false, None),
        (62, "photo-tool", 18_250, true, Some(64)),
    ];
    let processes = demo
        .iter()
        .map(|&(pid, name, rss_kb, sandboxed, mb): &(u32, &str, u64, bool, Option<u64>)| ProcInfo {
            pid,
            name: name.into(),
            rss_kb,
            cpu_percent: if name == "ferro-shell" { 3.0 } else { 0.0 },
            sandboxed,
            mem_limit_kb: mb.map(|m| m * 1024),
            ..ProcInfo::default()
        })
        .collect();
    let wave = ((tick as f32) * 0.35).sin();
    SystemInfo {
        clock: "12:00 PM".into(),
        mem_used_kb: Some(((30.0 + 3.0 * wave + tick as f32 * 0.05) * 1024.0) as u64),
        mem_total_kb: Some(64 * 1024),
        kernel: Some("6.12.0-ferro".into()),
        cpu_percent: Some(8.0 + 6.0 * wave.abs() + if tick.is_multiple_of(17) { 30.0 } else { 0.0 }),
        uptime_secs: Some(754),
        processes: Some(processes),
        net: Default::default(),
        install_media: false,
        setup_status: String::new(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, scene, path] if flag == "--screenshot" => screenshot(scene, path),
        [] => interactive(),
        _ => eprintln!("usage: ferro-preview [--screenshot desktop|processes|performance|terminal|budgets|privacy|vpn|setup out.png]"),
    }
}

fn click(shell: &mut Shell, x: i32, y: i32, button: MouseButton) {
    shell.handle(Event::MouseDown { x, y, button }, 0);
    if button == MouseButton::Left {
        shell.handle(Event::MouseUp { x, y, button }, 0);
    }
}

fn type_key(shell: &mut Shell, key: Key) {
    shell.handle(Event::Key { key, mods: Mods::default() }, 0);
}

fn type_text(shell: &mut Shell, text: &str) {
    for c in text.chars() {
        type_key(shell, Key::Char(c));
    }
}

/// Lets the in-process ferro-cmd thread answer before rendering.
fn settle(shell: &mut Shell) {
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(10));
        shell.poll_terminals();
    }
}

fn screenshot(scene: &str, path: &str) {
    let mut shell = Shell::new(W, H, drives());
    for tick in 0..60 {
        shell.set_info(demo_info(tick));
    }
    match scene {
        "desktop" => {
            shell.open_my_computer();
            shell.open_explorer(r"C:\");
            // Right-click empty space in the explorer window for its context menu.
            click(&mut shell, 560, 330, MouseButton::Right);
            shell.handle(Event::MouseMove { x: 600, y: 375 }, 0);
        }
        "processes" => {
            shell.open_explorer(r"C:\");
            shell.open_task_manager();
            // Select ferro-shell and right-click it (rows are sorted by memory).
            click(&mut shell, 300, 136, MouseButton::Right);
            shell.handle(Event::MouseMove { x: 340, y: 150 }, 0);
        }
        "performance" => {
            shell.open_task_manager();
            shell.task_manager_tab(1);
            // And the taskbar's context menu.
            click(&mut shell, 600, 590, MouseButton::Right);
            shell.handle(Event::MouseMove { x: 520, y: 520 }, 0);
        }
        "terminal" => {
            shell.set_terminal_spawner(Box::new(session::spawn));
            shell.open_terminal();
            settle(&mut shell);
            for line in ["ver", "echo Hello from FerroOS!", "drives", "frobnicate", "cd \\Windows", ""] {
                type_text(&mut shell, line);
                type_key(&mut shell, Key::Enter);
                settle(&mut shell);
            }
            type_text(&mut shell, "dir");
            settle(&mut shell);
            // Start > Programs flyout, hovering Command Prompt.
            shell.toggle_start_menu();
            click(&mut shell, 60, 340, MouseButton::Left);
            shell.handle(Event::MouseMove { x: 200, y: 332 }, 0);
        }
        "budgets" => {
            shell.open_task_manager();
            // Right-click the biggest process and point at a RAM budget.
            click(&mut shell, 200, 118, MouseButton::Right);
            shell.handle(Event::MouseMove { x: 240, y: 172 }, 0);
        }
        "setup" => shell.open_setup(),
        "vpn" => {
            // VPN and Tor both on and connected.
            let choices = std::env::temp_dir().join("ferro-demo-vpn.conf");
            let _ = std::fs::write(&choices, "switch network=on microphone=off camera=off vpn=on tor=on\n");
            shell.set_store_path(&choices);
            let mut info = demo_info(60);
            info.net = ferro_shell::NetStatus::parse(
                "vpn=connected\nendpoint=185.65.135.1:51820\nhandshake=14\nrx=81234\ntx=40211\ntor=on\n",
                "state=ready\ndetail=connected to the Tor network\n",
            );
            shell.set_info(info);
            shell.open_network_privacy();
        }
        "privacy" => {
            // A folder with a photo carrying GPS/camera EXIF, shown as D:\.
            let dir = std::env::temp_dir().join("ferro-demo");
            let _ = std::fs::create_dir_all(&dir);
            let seg = |m: u8, p: &[u8]| [&[0xFF, m][..], &((p.len() + 2) as u16).to_be_bytes(), p].concat();
            let exif = [b"Exif\0\0".to_vec(), vec![0u8; 9000]].concat();
            let photo = [vec![0xFF, 0xD8], seg(0xE1, &exif), seg(0xDA, &[0; 4]), vec![1, 2, 0xFF, 0xD9]].concat();
            let _ = std::fs::write(dir.join("holiday.jpg"), photo);
            let _ = std::fs::write(dir.join("notes.txt"), "shopping list");
            let mut drives = drives();
            drives.mount('D', &dir.to_string_lossy().replace('\\', "/"));
            shell = Shell::new(W, H, drives);
            for tick in 0..60 {
                shell.set_info(demo_info(tick));
            }
            shell.open_explorer(r"D:\");
            click(&mut shell, 140, 74, MouseButton::Right);
            shell.handle(Event::MouseMove { x: 180, y: 110 }, 0);
        }
        other => return eprintln!("unknown scene {other}"),
    }
    let mut s = Surface::new(W, H);
    shell.draw(&mut s);

    let rgb: Vec<u8> = s.pixels().iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    let file = std::io::BufWriter::new(std::fs::File::create(path).expect("create screenshot"));
    let mut enc = png::Encoder::new(file, W as u32, H as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().and_then(|mut w| w.write_image_data(&rgb)).expect("write png");
    println!("wrote {path}");
}

fn interactive() {
    let mut window = Window::new("FerroOS preview", W, H, WindowOptions::default()).expect("open window");
    window.set_target_fps(60);
    window.set_cursor_visibility(false);

    // Typed characters arrive through minifb's char callback; special keys
    // and Ctrl combinations through its key list.
    let typed = Rc::new(RefCell::new(Vec::<char>::new()));
    window.set_input_callback(Box::new(CharSink(Rc::clone(&typed))));

    let mut shell = Shell::new(W, H, drives());
    shell.set_terminal_spawner(Box::new(session::spawn));
    // The preview keeps its remembered choices in the host's temp folder.
    shell.set_store_path(std::env::temp_dir().join("ferro-preview-choices.conf"));
    let mut surface = Surface::new(W, H);
    let mut sys = System::new();
    let start = Instant::now();
    let mut last_info = Instant::now() - Duration::from_secs(10);
    let (mut last_pos, mut buttons) = ((-1, -1), [false; 2]);

    while window.is_open() {
        if last_info.elapsed() >= Duration::from_secs(1) {
            shell.set_info(host_info(&mut sys, shell.wants_processes()));
            last_info = Instant::now();
        }
        let now = start.elapsed().as_millis() as u64;
        if let Some((fx, fy)) = window.get_mouse_pos(MouseMode::Clamp) {
            let (x, y) = (fx as i32, fy as i32);
            if (x, y) != last_pos {
                shell.handle(Event::MouseMove { x, y }, now);
                last_pos = (x, y);
            }
            for (i, (mb, button)) in
                [(minifb::MouseButton::Left, MouseButton::Left), (minifb::MouseButton::Right, MouseButton::Right)].into_iter().enumerate()
            {
                let down = window.get_mouse_down(mb);
                if down != buttons[i] {
                    buttons[i] = down;
                    let ev = if down { Event::MouseDown { x, y, button } } else { Event::MouseUp { x, y, button } };
                    match shell.handle(ev, now) {
                        // Never kill host processes from a UI preview.
                        Some(Action::EndProcess(pid)) => println!("preview: would send SIGTERM to pid {pid}"),
                        Some(Action::SetMemoryLimit { pid, bytes }) => println!("preview: would set RAM budget of {pid} to {bytes:?}"),
                        Some(Action::SwitchesChanged) => println!("preview: kill switches now {:?}", shell.switches()),
                        Some(action) => {
                            println!("shell requested {action:?}");
                            return;
                        }
                        None => {}
                    }
                }
            }
        }
        let down = |a, b| window.is_key_down(a) || window.is_key_down(b);
        let mods = Mods {
            shift: down(minifb::Key::LeftShift, minifb::Key::RightShift),
            ctrl: down(minifb::Key::LeftCtrl, minifb::Key::RightCtrl),
            alt: down(minifb::Key::LeftAlt, minifb::Key::RightAlt),
        };
        let mut keys: Vec<Key> = window.get_keys_pressed(minifb::KeyRepeat::Yes).into_iter().filter_map(|k| special_key(k, mods)).collect();
        if !mods.ctrl && !mods.alt {
            keys.extend(typed.borrow_mut().drain(..).filter(|c| *c >= ' ' && *c != '\x7f').map(Key::Char));
        }
        typed.borrow_mut().clear();
        for key in keys {
            if let Some(action) = shell.handle(Event::Key { key, mods }, now) {
                println!("shell requested {action:?}");
                return;
            }
        }
        shell.poll_terminals();
        shell.draw(&mut surface);
        window.update_with_buffer(surface.pixels(), W, H).expect("present frame");
    }
}

struct CharSink(Rc<RefCell<Vec<char>>>);

impl minifb::InputCallback for CharSink {
    fn add_char(&mut self, uni_char: u32) {
        self.0.borrow_mut().extend(char::from_u32(uni_char));
    }
}

/// Non-text keys, plus letters when Ctrl/Alt is held (no char is typed then).
fn special_key(k: minifb::Key, mods: Mods) -> Option<Key> {
    use minifb::Key as M;
    Some(match k {
        M::Enter | M::NumPadEnter => Key::Enter,
        M::Backspace => Key::Backspace,
        M::Tab => Key::Tab,
        M::Escape => Key::Escape,
        M::Up => Key::Up,
        M::Down => Key::Down,
        M::Left => Key::Left,
        M::Right => Key::Right,
        M::Home => Key::Home,
        M::End => Key::End,
        M::Insert => Key::Insert,
        M::Delete => Key::Delete,
        M::PageUp => Key::PageUp,
        M::PageDown => Key::PageDown,
        M::F1 => Key::F(1),
        M::F2 => Key::F(2),
        M::F3 => Key::F(3),
        M::F4 => Key::F(4),
        M::F5 => Key::F(5),
        M::F6 => Key::F(6),
        M::F7 => Key::F(7),
        M::F8 => Key::F(8),
        M::F9 => Key::F(9),
        M::F10 => Key::F(10),
        M::F11 => Key::F(11),
        M::F12 => Key::F(12),
        _ if mods.ctrl || mods.alt => {
            let name = format!("{k:?}");
            let c = name.chars().next().filter(|c| name.len() == 1 && c.is_ascii_alphabetic())?;
            Key::Char(c.to_ascii_lowercase())
        }
        _ => return None,
    })
}
