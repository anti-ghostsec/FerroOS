//! ferro-ladspa: the Sound Enhancer as a LADSPA plugin. alsa-lib (Debian's
//! libasound2) has a `ladspa` PCM of its own; the `~/.asoundrc` ferro-audio
//! writes puts this plugin in front of every program's sound, so no sound
//! server and nothing else is needed:
//!
//! ```text
//! program -> plug -> ladspa (this, in the program) -> plug -> dmix -> card
//! ```
//!
//! Each program that plays has its own copy running, in its own process.
//! They all follow `~/.config/ferro/enhancer.conf`: every 50 ms of sound
//! the file's date is looked at, and when the window has changed it, the
//! new settings are taken at once, without the program reopening anything.
//! Unreadable (or missing), it's off: samples go through untouched.
//!
//! Two audio inputs and two outputs (stereo); ALSA mixes other channel
//! counts to and from two in the `plug` before it.

use ferro_dsp::{Enhancer, Settings};
use std::ffi::{c_char, c_int, c_ulong, c_void};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

const PORT_INPUT: c_int = 1;
const PORT_OUTPUT: c_int = 2;
const PORT_AUDIO: c_int = 8;
/// LADSPA_PROPERTY_HARD_RT_CAPABLE is not claimed: a settings change reads a file.
const PROPERTY_REALTIME: c_int = 1;

#[repr(C)]
pub struct PortRangeHint {
    hint: c_int,
    lower: f32,
    upper: f32,
}

type Handle = *mut c_void;

/// LADSPA's `LADSPA_Descriptor`, field for field.
#[repr(C)]
pub struct Descriptor {
    unique_id: c_ulong,
    label: *const c_char,
    properties: c_int,
    name: *const c_char,
    maker: *const c_char,
    copyright: *const c_char,
    port_count: c_ulong,
    port_descriptors: *const c_int,
    port_names: *const *const c_char,
    port_range_hints: *const PortRangeHint,
    implementation_data: *mut c_void,
    instantiate: unsafe extern "C" fn(*const Descriptor, c_ulong) -> Handle,
    connect_port: unsafe extern "C" fn(Handle, c_ulong, *mut f32),
    activate: Option<unsafe extern "C" fn(Handle)>,
    run: unsafe extern "C" fn(Handle, c_ulong),
    run_adding: Option<unsafe extern "C" fn(Handle, c_ulong)>,
    set_run_adding_gain: Option<unsafe extern "C" fn(Handle, f32)>,
    deactivate: Option<unsafe extern "C" fn(Handle)>,
    cleanup: unsafe extern "C" fn(Handle),
}

struct Shared<T>(T);
// SAFETY: only constant data (pointers to statics) that nothing writes.
unsafe impl<T> Sync for Shared<T> {}

static PORTS: [c_int; 4] = [PORT_INPUT | PORT_AUDIO, PORT_INPUT | PORT_AUDIO, PORT_OUTPUT | PORT_AUDIO, PORT_OUTPUT | PORT_AUDIO];
static NAMES: Shared<[*const c_char; 4]> = Shared([c"In L".as_ptr(), c"In R".as_ptr(), c"Out L".as_ptr(), c"Out R".as_ptr()]);
static HINTS: [PortRangeHint; 4] = [
    PortRangeHint { hint: 0, lower: 0.0, upper: 0.0 },
    PortRangeHint { hint: 0, lower: 0.0, upper: 0.0 },
    PortRangeHint { hint: 0, lower: 0.0, upper: 0.0 },
    PortRangeHint { hint: 0, lower: 0.0, upper: 0.0 },
];

/// The label the `~/.asoundrc` asks for.
pub const LABEL: &str = "ferro_enhancer";

static DESCRIPTOR: Shared<Descriptor> = Shared(Descriptor {
    unique_id: 5871,
    label: c"ferro_enhancer".as_ptr(),
    properties: PROPERTY_REALTIME,
    name: c"FerroOS Sound Enhancer".as_ptr(),
    maker: c"FerroOS".as_ptr(),
    copyright: c"MIT OR Apache-2.0".as_ptr(),
    port_count: 4,
    port_descriptors: PORTS.as_ptr(),
    port_names: NAMES.0.as_ptr(),
    port_range_hints: HINTS.as_ptr(),
    implementation_data: std::ptr::null_mut(),
    instantiate,
    connect_port,
    activate: Some(activate),
    run,
    run_adding: None,
    set_run_adding_gain: None,
    deactivate: None,
    cleanup,
});

/// What a file looked like when last read: changed, it's read again.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seen {
    Missing,
    File { mtime: i64, mtime_ns: i64, ino: u64, len: u64 },
}

struct Plugin {
    ports: [*mut f32; 4],
    enhancer: Enhancer,
    conf: Option<PathBuf>,
    seen: Option<Seen>,
    /// Frames until the file is looked at again, and how many between.
    countdown: usize,
    every: usize,
}

impl Plugin {
    /// Takes the settings again if the file changed (or on the first call).
    fn check(&mut self) {
        let now = match self.conf.as_ref().map(std::fs::metadata) {
            Some(Ok(m)) => Seen::File { mtime: m.mtime(), mtime_ns: m.mtime_nsec(), ino: m.ino(), len: m.len() },
            _ => Seen::Missing,
        };
        if self.seen == Some(now) {
            return;
        }
        self.seen = Some(now);
        let settings = match (now, &self.conf) {
            (Seen::File { .. }, Some(p)) => std::fs::read_to_string(p).map(|t| Settings::parse(&t)).unwrap_or_default(),
            _ => Settings::default(),
        };
        self.enhancer.set(&settings);
    }
}

/// The plugin's one descriptor (index 0), for ALSA (or any LADSPA host).
#[no_mangle]
pub extern "C" fn ladspa_descriptor(index: c_ulong) -> *const Descriptor {
    if index == 0 {
        &DESCRIPTOR.0
    } else {
        std::ptr::null()
    }
}

unsafe extern "C" fn instantiate(_: *const Descriptor, rate: c_ulong) -> Handle {
    let rate = u32::try_from(rate).unwrap_or(48000);
    let every = (rate as usize / 20).max(256);
    let plugin = Plugin {
        ports: [std::ptr::null_mut(); 4],
        enhancer: Enhancer::new(rate),
        conf: ferro_dsp::settings::path(),
        seen: None,
        countdown: 0,
        every,
    };
    Box::into_raw(Box::new(plugin)).cast()
}

unsafe extern "C" fn connect_port(h: Handle, port: c_ulong, data: *mut f32) {
    // SAFETY: the host passes back the handle instantiate gave it.
    let p = unsafe { &mut *h.cast::<Plugin>() };
    if let Some(slot) = usize::try_from(port).ok().and_then(|i| p.ports.get_mut(i)) {
        *slot = data;
    }
}

unsafe extern "C" fn activate(h: Handle) {
    // SAFETY: as in connect_port.
    let p = unsafe { &mut *h.cast::<Plugin>() };
    p.countdown = 0;
}

unsafe extern "C" fn run(h: Handle, frames: c_ulong) {
    // SAFETY: as in connect_port.
    let p = unsafe { &mut *h.cast::<Plugin>() };
    let n = frames as usize;
    let [in_l, in_r, out_l, out_r] = p.ports;
    if n == 0 || [in_l, in_r, out_l, out_r].iter().any(|q| q.is_null()) {
        return;
    }
    if p.countdown < n {
        p.check();
        p.countdown = p.every;
    } else {
        p.countdown -= n;
    }
    // The input may be the output's own buffer (the host may run plugins
    // in place): copy (memmove), then work on the output only.
    // SAFETY: the host connected each port to `frames` floats.
    unsafe {
        if in_l != out_l {
            std::ptr::copy(in_l, out_l, n);
        }
        if in_r != out_r {
            std::ptr::copy(in_r, out_r, n);
        }
        if p.enhancer.bypassed() {
            return;
        }
        if out_l == out_r {
            // Both outputs in one buffer can't be: leave the sound as it is.
            return;
        }
        let l = std::slice::from_raw_parts_mut(out_l, n);
        let r = std::slice::from_raw_parts_mut(out_r, n);
        p.enhancer.process(l, r);
    }
}

unsafe extern "C" fn cleanup(h: Handle) {
    if !h.is_null() {
        // SAFETY: made by Box::into_raw in instantiate, freed once.
        drop(unsafe { Box::from_raw(h.cast::<Plugin>()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_is_ladspas() {
        let d = ladspa_descriptor(0);
        assert!(!d.is_null() && ladspa_descriptor(1).is_null());
        // SAFETY: a static.
        let d = unsafe { &*d };
        assert_eq!(d.port_count, 4);
        // SAFETY: a C string literal.
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(d.label) }.to_str().unwrap(), LABEL);
    }

    #[test]
    fn runs_in_place_and_apart() {
        let d = ladspa_descriptor(0);
        // SAFETY: the calls a LADSPA host makes, in its order, on live buffers.
        unsafe {
            let h = ((*d).instantiate)(d, 48000);
            let mut l = vec![0.25f32; 300];
            let mut r = vec![-0.5f32; 300];
            let mut out_r = vec![0.0f32; 300];
            ((*d).connect_port)(h, 0, l.as_mut_ptr());
            ((*d).connect_port)(h, 1, r.as_mut_ptr());
            ((*d).connect_port)(h, 2, l.as_mut_ptr());
            ((*d).connect_port)(h, 3, out_r.as_mut_ptr());
            ((*d).activate.unwrap())(h);
            ((*d).run)(h, 300);
            // No settings where tests run (or Off): untouched.
            if std::env::var_os("HOME").is_none_or(|_| !Settings::load().on) {
                assert!(l.iter().all(|&v| v == 0.25));
                assert_eq!(out_r, r);
            }
            ((*d).cleanup)(h);
        }
    }
}
