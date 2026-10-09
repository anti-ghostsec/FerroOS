//! Linux backend: `/dev/fb0` for output, `/dev/input/mice` for the pointer.
//!
//! fbdev is the smallest thing that works (QEMU's bochs-drm exposes one via
//! fbdev emulation). Moving to DRM dumb buffers + page flips is the next step,
//! and is required before the shell can hand the GPU to fullscreen games.

use crate::drm::DrmDisplay;
use crate::keymap::Keyboard;
use crate::{Action, Damage, Event, MouseButton, Shell, SystemInfo};
use ferro_gfx::Surface;
use ferro_path::DriveTable;
use ferro_sys::{MemInfo, Sampler};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;
const KDSETMODE: libc::c_ulong = 0x4B3A;
const KD_TEXT: libc::c_int = 0;
const KD_GRAPHICS: libc::c_int = 1;

#[repr(C)]
#[derive(Default)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Default)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

struct Framebuffer {
    ptr: *mut u8,
    len: usize,
    width: usize,
    height: usize,
    stride: usize,
    _file: File,
}

impl Framebuffer {
    fn open(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let fd = file.as_raw_fd();
        let mut var = FbVarScreeninfo::default();
        let mut fix = FbFixScreeninfo::default();
        // SAFETY: both structs match the kernel's fb.h layout.
        unsafe {
            if libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut var) < 0 || libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut fix) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        if var.bits_per_pixel != 32 || var.red.offset != 16 {
            return Err(io::Error::other(format!(
                "unsupported pixel format: {} bpp, red at bit {} (need 32 bpp XRGB8888)",
                var.bits_per_pixel, var.red.offset
            )));
        }
        let len = fix.smem_len as usize;
        // SAFETY: mapping the device's own reported size, shared + read/write.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0) };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { ptr: ptr.cast(), len, width: var.xres as usize, height: var.yres as usize, stride: fix.line_length as usize, _file: file })
    }

    fn present(&mut self, s: &Surface) {
        let w = self.width.min(s.width);
        for y in 0..self.height.min(s.height) {
            let src = &s.pixels()[y * s.stride..y * s.stride + w];
            let off = y * self.stride;
            if off + w * 4 > self.len {
                break;
            }
            // SAFETY: bounds checked against the mapping length above.
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u8>(), self.ptr.add(off), w * 4) };
        }
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly what `open` mapped.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

/// Stops fbcon drawing its text console over the desktop while we run.
struct GraphicsMode(Option<File>);

impl GraphicsMode {
    fn enter() -> Self {
        let tty = OpenOptions::new().write(true).open("/dev/tty0").ok();
        if let Some(t) = &tty {
            // SAFETY: KDSETMODE takes an int argument.
            unsafe { libc::ioctl(t.as_raw_fd(), KDSETMODE as _, KD_GRAPHICS) };
        }
        Self(tty)
    }

    fn leave(&mut self) {
        if let Some(t) = self.0.take() {
            // SAFETY: as above.
            unsafe { libc::ioctl(t.as_raw_fd(), KDSETMODE as _, KD_TEXT) };
        }
    }
}

impl Drop for GraphicsMode {
    fn drop(&mut self) {
        self.leave();
    }
}

/// PS/2-protocol relative pointer from mousedev.
struct Mouse {
    file: File,
    x: i32,
    y: i32,
    max: (i32, i32),
    buttons: u8,
}

impl Mouse {
    fn open(w: usize, h: usize) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open("/dev/input/mice")?;
        let max = (w as i32 - 1, h as i32 - 1);
        Ok(Self { file, x: max.0 / 2, y: max.1 / 2, max, buttons: 0 })
    }

    /// Converts any pending PS/2 packets into events.
    fn read_events(&mut self, events: &mut Vec<Event>) {
        let mut buf = [0u8; 3 * 64];
        let n = self.file.read(&mut buf).unwrap_or(0);
        for p in buf[..n].as_chunks::<3>().0 {
            let (dx, dy) = (p[1] as i8 as i32, p[2] as i8 as i32);
            if dx != 0 || dy != 0 {
                self.x = (self.x + dx).clamp(0, self.max.0);
                self.y = (self.y - dy).clamp(0, self.max.1);
                events.push(Event::MouseMove { x: self.x, y: self.y });
            }
            for (bit, button) in [(1u8, MouseButton::Left), (2, MouseButton::Right)] {
                let (was, now) = (self.buttons & bit != 0, p[0] & bit != 0);
                let (x, y) = (self.x, self.y);
                match (was, now) {
                    (false, true) => events.push(Event::MouseDown { x, y, button }),
                    (true, false) => events.push(Event::MouseUp { x, y, button }),
                    _ => {}
                }
            }
            self.buttons = p[0] & 3;
        }
    }
}

const EV_KEY: u16 = 1;

/// `struct input_event` on 64-bit Linux.
#[repr(C)]
struct InputEvent {
    time: [i64; 2],
    kind: u16,
    code: u16,
    value: i32,
}

/// Every evdev device that has letter keys, grabbed so key presses don't
/// also reach the text console underneath the desktop.
struct Keyboards {
    files: Vec<File>,
    state: Keyboard,
}

impl Keyboards {
    fn open() -> Self {
        // EVIOCGBIT(EV_KEY, 96 bytes) and EVIOCGRAB, from <linux/input.h>.
        const EVIOCGBIT_KEY: libc::c_ulong = (2 << 30) | (96 << 16) | (0x45 << 8) | 0x21;
        const EVIOCGRAB: libc::c_ulong = (1 << 30) | (4 << 16) | (0x45 << 8) | 0x90;
        const KEY_A: usize = 30;
        let mut files = Vec::new();
        for entry in std::fs::read_dir("/dev/input").into_iter().flatten().flatten() {
            if !entry.file_name().to_string_lossy().starts_with("event") {
                continue;
            }
            let Ok(file) = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(entry.path()) else {
                continue;
            };
            let mut bits = [0u8; 96];
            // SAFETY: the buffer is exactly the size encoded in the request.
            let ok = unsafe { libc::ioctl(file.as_raw_fd(), EVIOCGBIT_KEY as _, bits.as_mut_ptr()) } >= 0;
            if ok && bits[KEY_A / 8] & (1 << (KEY_A % 8)) != 0 {
                // SAFETY: EVIOCGRAB takes an int flag.
                unsafe { libc::ioctl(file.as_raw_fd(), EVIOCGRAB as _, 1) };
                files.push(file);
            }
        }
        Self { files, state: Keyboard::default() }
    }

    fn read_events(&mut self, index: usize, events: &mut Vec<Event>) {
        const SIZE: usize = std::mem::size_of::<InputEvent>();
        let mut buf = [0u8; SIZE * 32];
        let n = self.files[index].read(&mut buf).unwrap_or(0);
        for raw in buf[..n].as_chunks::<SIZE>().0 {
            // SAFETY: InputEvent is plain data and `raw` is exactly its size.
            let ev: InputEvent = unsafe { std::ptr::read_unaligned(raw.as_ptr().cast()) };
            if ev.kind == EV_KEY {
                events.extend(self.state.event(ev.code, ev.value));
            }
        }
    }
}

/// Waits up to `timeout` for mouse or keyboard input and collects events.
fn poll_input(mouse: &mut Option<Mouse>, keyboards: &mut Keyboards, timeout: Duration) -> Vec<Event> {
    let mut fds: Vec<libc::pollfd> =
        mouse.iter().map(|m| &m.file).chain(&keyboards.files).map(|f| libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLIN, revents: 0 }).collect();
    let mut events = Vec::new();
    if fds.is_empty() {
        std::thread::sleep(timeout);
        return events;
    }
    // SAFETY: `fds` is a valid array of pollfd for the call.
    if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout.as_millis() as libc::c_int) } <= 0 {
        return events;
    }
    let mut ready = fds.iter().map(|p| p.revents & libc::POLLIN != 0);
    if let Some(m) = mouse {
        if ready.next() == Some(true) {
            m.read_events(&mut events);
        }
    }
    for (i, r) in ready.enumerate() {
        if r {
            keyboards.read_events(i, &mut events);
        }
    }
    events
}
fn system_info(sampler: &mut Sampler, with_processes: bool) -> SystemInfo {
    let mem = MemInfo::read();
    let (cpu_percent, processes) = sampler.sample(with_processes);
    SystemInfo {
        clock: ferro_sys::clock_12h(),
        // The tray and Task Manager show the true footprint, kernel included.
        mem_used_kb: mem.map(|m| m.footprint_kb()),
        mem_total_kb: mem.map(|m| m.physical_kb.unwrap_or(m.total_kb)),
        kernel: ferro_sys::kernel_release(),
        cpu_percent,
        uptime_secs: ferro_sys::uptime_secs(),
        processes,
        net: ferro_shell_net_status(),
        install_media: Path::new("/run/ferro/install-media").exists(),
        setup_status: std::fs::read_to_string("/run/ferro/install-status").unwrap_or_default(),
    }
}

fn ferro_shell_net_status() -> crate::NetStatus {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    crate::NetStatus::parse(&read("/run/ferro/net-status"), &read("/run/ferro/tor-status"))
}

/// Task Manager's "End Process": SIGTERM, refusing init itself.
fn end_process(pid: u32) {
    if pid <= 1 {
        return;
    }
    // SAFETY: plain kill(2).
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
        eprintln!("ferro-shell: kill {pid}: {}", io::Error::last_os_error());
    }
}

/// Where frames go: DRM draws in place; fbdev needs a private back buffer
/// (its mapping may be flushed to the screen mid-draw).
enum Display {
    Drm(DrmDisplay),
    Fb(Framebuffer, Surface<'static>),
}

impl Display {
    fn open() -> Self {
        match DrmDisplay::open("/dev/dri/card0") {
            Ok(d) => {
                eprintln!("ferro-shell: DRM/KMS {}x{} at {} bpp, drawing in place", d.width, d.height, d.bpp);
                return Display::Drm(d);
            }
            Err(e) => eprintln!("ferro-shell: DRM unavailable ({e}); using /dev/fb0"),
        }
        let fb = Framebuffer::open("/dev/fb0").unwrap_or_else(|e| {
            eprintln!("ferro-shell: /dev/fb0: {e}");
            std::process::exit(1);
        });
        let surface = Surface::new(fb.width, fb.height);
        Display::Fb(fb, surface)
    }

    fn size(&self) -> (usize, usize) {
        match self {
            Display::Drm(d) => (d.width, d.height),
            Display::Fb(fb, _) => (fb.width, fb.height),
        }
    }

    /// Redraws what changed: everything, or only the damaged rectangles.
    fn frame(&mut self, shell: &Shell, damage: &Damage) {
        let rects: &[ferro_gfx::Rect] = match damage {
            Damage::None => return,
            Damage::Full => &[],
            Damage::Rects(r) => r,
        };
        match self {
            Display::Drm(d) => {
                draw_damaged(shell, &mut d.surface(), rects);
                d.flush(rects);
            }
            Display::Fb(fb, surface) => {
                draw_damaged(shell, surface, rects);
                fb.present(surface);
            }
        }
    }
}

fn draw_damaged(shell: &Shell, s: &mut Surface, rects: &[ferro_gfx::Rect]) {
    if rects.is_empty() {
        s.set_clip(None);
        shell.draw(s);
        return;
    }
    for r in rects {
        s.set_clip(Some(*r));
        shell.draw(s);
    }
    s.set_clip(None);
}

pub fn run() {
    let mut display = Display::open();
    let mut gfx = GraphicsMode::enter();
    let (width, height) = display.size();

    let mut drives = DriveTable::default();
    if Path::new("/mnt/d").is_dir() {
        drives.mount('D', "/mnt/d");
    }
    let mut shell = Shell::new(width, height, drives);
    let mut mouse = Mouse::open(width, height).map_err(|e| eprintln!("ferro-shell: no pointer yet (/dev/input/mice: {e}); will keep looking")).ok();
    if let Some(m) = &mouse {
        shell.move_cursor(m.x, m.y);
    }
    let mut keyboards = Keyboards::open();
    if keyboards.files.is_empty() {
        eprintln!("ferro-shell: no keyboard found in /dev/input");
    }
    shell.set_terminal_spawner(Box::new(crate::pty::spawn));
    shell.set_store_path(ferro_sandbox::store::DEFAULT_PATH);
    shell.set_system_link(Box::new(crate::link::SocketLink));
    shell.begin_logon();
    // The boot menu's "Install FerroOS" entry goes straight to Setup.
    if std::fs::read_to_string("/proc/cmdline").is_ok_and(|c| c.split_whitespace().any(|w| w == "ferro.install")) {
        shell.open_setup();
    }

    let start = Instant::now();
    let mut sampler = Sampler::default();
    let mut next_tick = Instant::now();
    let mut dirty = true;
    loop {
        let now = Instant::now();
        if now >= next_tick {
            shell.set_info(system_info(&mut sampler, shell.wants_processes()));
            // Pointer and keyboard drivers can arrive after us: the PS/2 mouse
            // probes asynchronously, and USB devices are plugged in any time.
            if mouse.is_none() {
                mouse = Mouse::open(width, height).ok();
                if let Some(m) = &mouse {
                    shell.move_cursor(m.x, m.y);
                }
            }
            if keyboards.files.is_empty() {
                keyboards = Keyboards::open();
            }

            next_tick = now + Duration::from_secs(1);
            dirty = true;
        }
        // Terminals have no fd in this loop, so poll them at ~60 Hz while open.
        let mut timeout = next_tick.saturating_duration_since(now);
        if shell.has_terminals() {
            timeout = timeout.min(Duration::from_millis(16));
        }
        let events = poll_input(&mut mouse, &mut keyboards, timeout);
        dirty |= !events.is_empty();
        dirty |= shell.poll_terminals();
        for ev in events {
            match shell.handle(ev, start.elapsed().as_millis() as u64) {
                Some(Action::EndProcess(pid)) => end_process(pid),
                // Applied through ferro-system by the shell itself.
                Some(Action::SwitchesChanged | Action::SetMemoryLimit { .. }) => {}
                Some(action) => {
                    drop(display); // restore the text console first
                    gfx.leave();
                    std::process::exit(action.exit_code().unwrap_or(0));
                }
                None => {}
            }
        }
        if dirty {
            let damage = shell.take_damage();
            display.frame(&shell, &damage);
            dirty = false;
        }
    }
}
