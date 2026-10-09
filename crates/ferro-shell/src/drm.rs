//! DRM/KMS display: the shell draws straight into a kernel "dumb" buffer
//! that is scanned out (or, for shadow-buffered drivers like bochs and
//! virtio-gpu, copied to the GPU on DIRTYFB). Unlike the fbdev path there is
//! no private copy of the screen in ferro-shell, which halves its memory.
//!
//! Raw ioctls against <drm/drm_mode.h>; no extra crates.

use ferro_gfx::Surface;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;

const fn iowr(nr: u32, size: usize) -> libc::c_ulong {
    ((3 << 30) | ((size as u32) << 16) | (0x64 << 8) | nr) as libc::c_ulong
}

const fn io_none(nr: u32) -> libc::c_ulong {
    ((0x64 << 8) | nr) as libc::c_ulong
}

#[repr(C)]
#[derive(Default)]
struct CardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ModeInfo {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    kind: u32,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Default)]
struct GetConnector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct GetEncoder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
#[derive(Default)]
struct CreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
#[derive(Default)]
struct MapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
#[derive(Default)]
struct DestroyDumb {
    handle: u32,
}

#[repr(C)]
#[derive(Default)]
struct FbCmd {
    fb_id: u32,
    width: u32,
    height: u32,
    pitch: u32,
    bpp: u32,
    depth: u32,
    handle: u32,
}

#[repr(C)]
#[derive(Default)]
struct Crtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: ModeInfo,
}

#[repr(C)]
struct ClipRect {
    x1: u16,
    y1: u16,
    x2: u16,
    y2: u16,
}

#[repr(C)]
#[derive(Default)]
struct FbDirty {
    fb_id: u32,
    flags: u32,
    color: u32,
    num_clips: u32,
    clips_ptr: u64,
}

const GETRESOURCES: libc::c_ulong = iowr(0xA0, std::mem::size_of::<CardRes>());
const GETCRTC: libc::c_ulong = iowr(0xA1, std::mem::size_of::<Crtc>());
const SETCRTC: libc::c_ulong = iowr(0xA2, std::mem::size_of::<Crtc>());
const GETENCODER: libc::c_ulong = iowr(0xA6, std::mem::size_of::<GetEncoder>());
const GETCONNECTOR: libc::c_ulong = iowr(0xA7, std::mem::size_of::<GetConnector>());
const ADDFB: libc::c_ulong = iowr(0xAE, std::mem::size_of::<FbCmd>());
const RMFB: libc::c_ulong = iowr(0xAF, std::mem::size_of::<u32>());
const DIRTYFB: libc::c_ulong = iowr(0xB1, std::mem::size_of::<FbDirty>());
const CREATE_DUMB: libc::c_ulong = iowr(0xB2, std::mem::size_of::<CreateDumb>());
const MAP_DUMB: libc::c_ulong = iowr(0xB3, std::mem::size_of::<MapDumb>());
const DESTROY_DUMB: libc::c_ulong = iowr(0xB4, std::mem::size_of::<DestroyDumb>());
const SET_MASTER: libc::c_ulong = io_none(0x1E);
const DRM_MODE_TYPE_PREFERRED: u32 = 1 << 3;

fn ioctl<T>(fd: i32, req: libc::c_ulong, arg: &mut T) -> io::Result<()> {
    // SAFETY: every `T` passed here is the repr(C) struct encoded in `req`.
    loop {
        if unsafe { libc::ioctl(fd, req as _, arg as *mut T) } == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

pub struct DrmDisplay {
    file: File,
    crtc_id: u32,
    connector_id: u32,
    mode: ModeInfo,
    fb_id: u32,
    handle: u32,
    map: *mut u8,
    map_len: usize,
    pub width: usize,
    pub height: usize,
    /// Row length in pixels.
    pub stride: usize,
    /// 16 (RGB565) or 32 (XRGB8888) bits per pixel.
    pub bpp: u32,
    /// What was on screen before us (fbcon), restored on exit.
    saved: Crtc,
}

impl DrmDisplay {
    pub fn open(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let fd = file.as_raw_fd();
        let _ = ioctl(fd, SET_MASTER, &mut 0u32);

        // Two-pass: ask for counts, then fetch the id arrays.
        let mut res = CardRes::default();
        ioctl(fd, GETRESOURCES, &mut res)?;
        let mut crtcs = vec![0u32; res.count_crtcs as usize];
        let mut conns = vec![0u32; res.count_connectors as usize];
        let mut res2 = CardRes {
            crtc_id_ptr: crtcs.as_mut_ptr() as u64,
            connector_id_ptr: conns.as_mut_ptr() as u64,
            count_crtcs: res.count_crtcs,
            count_connectors: res.count_connectors,
            ..CardRes::default()
        };
        ioctl(fd, GETRESOURCES, &mut res2)?;

        for &connector_id in &conns {
            let mut c = GetConnector { connector_id, ..GetConnector::default() };
            ioctl(fd, GETCONNECTOR, &mut c)?;
            if c.connection != 1 || c.count_modes == 0 {
                continue; // not connected
            }
            let mut modes = vec![ModeInfo::default(); c.count_modes as usize];
            let mut encs = vec![0u32; c.count_encoders as usize];
            let mut c2 = GetConnector {
                connector_id,
                modes_ptr: modes.as_mut_ptr() as u64,
                count_modes: c.count_modes,
                encoders_ptr: encs.as_mut_ptr() as u64,
                count_encoders: c.count_encoders,
                ..GetConnector::default()
            };
            ioctl(fd, GETCONNECTOR, &mut c2)?;
            modes.truncate(c2.count_modes as usize);
            let Some(mode) = modes.iter().find(|m| m.kind & DRM_MODE_TYPE_PREFERRED != 0).or(modes.first()).copied() else {
                continue;
            };
            let mut crtc_id = 0;
            if c2.encoder_id != 0 {
                let mut e = GetEncoder { encoder_id: c2.encoder_id, ..GetEncoder::default() };
                if ioctl(fd, GETENCODER, &mut e).is_ok() {
                    crtc_id = e.crtc_id;
                }
            }
            if crtc_id == 0 {
                for &enc in &encs {
                    let mut e = GetEncoder { encoder_id: enc, ..GetEncoder::default() };
                    if ioctl(fd, GETENCODER, &mut e).is_ok() {
                        if let Some(i) = (0..crtcs.len()).find(|i| e.possible_crtcs & (1 << i) != 0) {
                            crtc_id = crtcs[i];
                            break;
                        }
                    }
                }
            }
            if crtc_id == 0 {
                continue;
            }
            return Self::setup(file, crtc_id, connector_id, mode);
        }
        Err(io::Error::other("no connected display"))
    }

    fn setup(file: File, crtc_id: u32, connector_id: u32, mode: ModeInfo) -> io::Result<Self> {
        // 16-bit High Color, as Windows 95 ran: the screen buffer takes half
        // the RAM (2 MB instead of 4 at 1280x800) and the Win95 palette looks
        // the same. Drivers without RGB565 scanout get 32-bit.
        match Self::setup_bpp(file.try_clone()?, crtc_id, connector_id, mode, 16) {
            Ok(d) => Ok(d),
            Err(_) => Self::setup_bpp(file, crtc_id, connector_id, mode, 32),
        }
    }

    fn setup_bpp(file: File, crtc_id: u32, connector_id: u32, mode: ModeInfo, bpp: u32) -> io::Result<Self> {
        let fd = file.as_raw_fd();
        let (w, h) = (u32::from(mode.hdisplay), u32::from(mode.vdisplay));
        let mut saved = Crtc { crtc_id, ..Crtc::default() };
        let _ = ioctl(fd, GETCRTC, &mut saved);

        let mut dumb = CreateDumb { width: w, height: h, bpp, ..CreateDumb::default() };
        ioctl(fd, CREATE_DUMB, &mut dumb)?;
        let depth = if bpp == 16 { 16 } else { 24 };
        let mut fb = FbCmd { width: w, height: h, pitch: dumb.pitch, bpp, depth, handle: dumb.handle, ..FbCmd::default() };
        if let Err(e) = ioctl(fd, ADDFB, &mut fb) {
            let _ = ioctl(fd, DESTROY_DUMB, &mut DestroyDumb { handle: dumb.handle });
            return Err(e);
        }
        // From here on, Drop releases the buffer if anything fails.
        let mut display = Self {
            file,
            crtc_id,
            connector_id,
            mode,
            fb_id: fb.fb_id,
            handle: dumb.handle,
            map: std::ptr::null_mut(),
            map_len: 0,
            width: w as usize,
            height: h as usize,
            stride: dumb.pitch as usize / (bpp as usize / 8),
            bpp,
            saved: Crtc::default(),
        };
        let mut map = MapDumb { handle: dumb.handle, ..MapDumb::default() };
        ioctl(fd, MAP_DUMB, &mut map)?;
        let len = dumb.size as usize;
        // SAFETY: mapping the dumb buffer at the offset the kernel gave us.
        let ptr =
            unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, map.offset as libc::off_t) };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        display.map = ptr.cast();
        display.map_len = len;
        // The mode set is where a driver rejects a format it can't scan out.
        display.set_crtc()?;
        display.saved = saved;
        Ok(display)
    }

    fn set_crtc(&mut self) -> io::Result<()> {
        let mut conn = self.connector_id;
        let mut c = Crtc {
            set_connectors_ptr: &mut conn as *mut u32 as u64,
            count_connectors: 1,
            crtc_id: self.crtc_id,
            fb_id: self.fb_id,
            mode_valid: 1,
            mode: self.mode,
            ..Crtc::default()
        };
        ioctl(self.file.as_raw_fd(), SETCRTC, &mut c)
    }

    /// A surface that draws straight into the scanout buffer.
    pub fn surface(&mut self) -> Surface<'_> {
        let (w, h, stride) = (self.width, self.height, self.stride);
        // SAFETY: the mapping is `map_len` bytes, page-aligned, and lives as
        // long as `self`; the &mut borrow prevents aliasing.
        unsafe {
            if self.bpp == 16 {
                Surface::borrowed_565(std::slice::from_raw_parts_mut(self.map.cast::<u16>(), self.map_len / 2), w, h, stride)
            } else {
                Surface::borrowed(std::slice::from_raw_parts_mut(self.map.cast::<u32>(), self.map_len / 4), w, h, stride)
            }
        }
    }

    /// Tells shadow-buffered drivers to push changed areas to the GPU (all
    /// of it when `rects` is empty). Drivers that scan the dumb buffer out
    /// directly return ENOSYS, which is fine.
    pub fn flush(&mut self, rects: &[ferro_gfx::Rect]) {
        let clips: Vec<ClipRect> = rects
            .iter()
            .filter(|r| r.w > 0 && r.h > 0)
            .map(|r| ClipRect {
                x1: r.x.clamp(0, self.width as i32) as u16,
                y1: r.y.clamp(0, self.height as i32) as u16,
                x2: r.right().clamp(0, self.width as i32) as u16,
                y2: r.bottom().clamp(0, self.height as i32) as u16,
            })
            .collect();
        // The kernel insists: no clips means a null pointer (whole screen),
        // not a pointer to an empty array (EINVAL, and nothing is flushed).
        let clips_ptr = if clips.is_empty() { 0 } else { clips.as_ptr() as u64 };
        let mut d = FbDirty { fb_id: self.fb_id, num_clips: clips.len() as u32, clips_ptr, ..FbDirty::default() };
        let _ = ioctl(self.file.as_raw_fd(), DIRTYFB, &mut d);
    }
}

impl Drop for DrmDisplay {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        // Give the screen back to whatever was there (the text console).
        if self.saved.fb_id != 0 {
            let mut conn = self.connector_id;
            self.saved.set_connectors_ptr = &mut conn as *mut u32 as u64;
            self.saved.count_connectors = 1;
            let _ = ioctl(fd, SETCRTC, &mut self.saved);
        }
        if !self.map.is_null() {
            // SAFETY: unmapping what `setup_bpp` mapped.
            unsafe { libc::munmap(self.map.cast(), self.map_len) };
        }
        let _ = ioctl(fd, RMFB, &mut self.fb_id);
        let _ = ioctl(fd, DESTROY_DUMB, &mut DestroyDumb { handle: self.handle });
    }
}
