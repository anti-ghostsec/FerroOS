//! The X11 protocol as bytes, as far as the gate needs it: the two setup
//! messages, how long each request and each reply or event is, and how big
//! a picture GetImage hands back. Nothing here talks to anything.

/// The byte order a program chose when it connected (true: little-endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order(pub bool);

impl Order {
    pub fn u16(self, b: &[u8], at: usize) -> u16 {
        let v = [b[at], b[at + 1]];
        if self.0 {
            u16::from_le_bytes(v)
        } else {
            u16::from_be_bytes(v)
        }
    }

    pub fn u32(self, b: &[u8], at: usize) -> u32 {
        let v = [b[at], b[at + 1], b[at + 2], b[at + 3]];
        if self.0 {
            u32::from_le_bytes(v)
        } else {
            u32::from_be_bytes(v)
        }
    }

    pub fn put_u16(self, b: &mut [u8], at: usize, v: u16) {
        b[at..at + 2].copy_from_slice(&if self.0 { v.to_le_bytes() } else { v.to_be_bytes() });
    }

    pub fn put_u32(self, b: &mut [u8], at: usize, v: u32) {
        b[at..at + 4].copy_from_slice(&if self.0 { v.to_le_bytes() } else { v.to_be_bytes() });
    }
}

pub const fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

/// The program's first message, from its first 12 bytes: its byte order,
/// its protocol version and how long the whole message is.
pub fn setup_request(head: &[u8]) -> Option<(Order, u16, u16, usize)> {
    let order = match head.first()? {
        b'l' => Order(true),
        b'B' => Order(false),
        _ => return None,
    };
    if head.len() < 12 {
        return None;
    }
    let (name, data) = (usize::from(order.u16(head, 6)), usize::from(order.u16(head, 8)));
    Some((order, order.u16(head, 2), order.u16(head, 4), 12 + pad4(name) + pad4(data)))
}

/// The same first message, with the display's real cookie in place of
/// whatever the program offered.
pub fn setup_request_with(order: Order, major: u16, minor: u16, cookie: &[u8]) -> Vec<u8> {
    const NAME: &[u8] = b"MIT-MAGIC-COOKIE-1";
    let mut b = vec![0u8; 12];
    b[0] = if order.0 { b'l' } else { b'B' };
    order.put_u16(&mut b, 2, major);
    order.put_u16(&mut b, 4, minor);
    order.put_u16(&mut b, 6, NAME.len() as u16);
    order.put_u16(&mut b, 8, cookie.len() as u16);
    b.extend_from_slice(NAME);
    b.resize(12 + pad4(NAME.len()), 0);
    b.extend_from_slice(cookie);
    b.resize(12 + pad4(NAME.len()) + pad4(cookie.len()), 0);
    b
}

/// How long the display's answer to the first message is, from its first 8 bytes.
pub fn setup_reply_len(order: Order, head: &[u8]) -> usize {
    8 + 4 * usize::from(order.u16(head, 6))
}

/// What the gate keeps of the display's description of itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Setup {
    /// A program's own windows, pixmaps... have ids `base | n`, n within `mask`.
    pub base: u32,
    pub mask: u32,
    /// The scanline padding of XY images (bits).
    pub bitmap_pad: u8,
    /// Each depth's (bits per pixel, scanline padding in bits).
    pub formats: Vec<(u8, u8, u8)>,
    /// Each screen's root window, its visual and its depth.
    pub roots: Vec<(u32, u32, u8)>,
}

pub fn parse_setup(order: Order, b: &[u8]) -> Option<Setup> {
    if b.len() < 40 || b[0] != 1 {
        return None;
    }
    let vendor = usize::from(order.u16(b, 24));
    let (screens, nformats) = (usize::from(b[28]), usize::from(b[29]));
    let mut at = 40 + pad4(vendor);
    let mut formats = Vec::with_capacity(nformats);
    for _ in 0..nformats {
        let f = b.get(at..at + 8)?;
        formats.push((f[0], f[1], f[2]));
        at += 8;
    }
    let mut roots = Vec::with_capacity(screens);
    for _ in 0..screens {
        let s = b.get(at..at + 40)?;
        roots.push((order.u32(s, 0), order.u32(s, 32), s[38]));
        let depths = usize::from(s[39]);
        at += 40;
        for _ in 0..depths {
            let d = b.get(at..at + 8)?;
            at += 8 + 24 * usize::from(order.u16(d, 2));
        }
    }
    Some(Setup { base: order.u32(b, 12), mask: order.u32(b, 16), bitmap_pad: b[33], formats, roots })
}

/// One request, as the program sent it.
pub struct Req<'a> {
    pub order: Order,
    pub bytes: &'a [u8],
    /// Sent with BIG-REQUESTS' longer header: its fields sit 4 bytes later.
    pub big: bool,
}

impl Req<'_> {
    pub fn opcode(&self) -> u8 {
        self.bytes[0]
    }

    /// The second byte: an extension's request number, or a core request's small field.
    pub fn minor(&self) -> u8 {
        self.bytes[1]
    }

    /// Where field `at` (as the protocol numbers it, from the request's start) really is.
    pub fn at(&self, at: usize) -> usize {
        if self.big && at >= 4 {
            at + 4
        } else {
            at
        }
    }

    pub fn u8(&self, at: usize) -> Option<u8> {
        self.bytes.get(self.at(at)).copied()
    }

    pub fn u16(&self, at: usize) -> Option<u16> {
        let at = self.at(at);
        (self.bytes.len() >= at + 2).then(|| self.order.u16(self.bytes, at))
    }

    pub fn u32(&self, at: usize) -> Option<u32> {
        let at = self.at(at);
        (self.bytes.len() >= at + 4).then(|| self.order.u32(self.bytes, at))
    }
}

/// How long the request at the start of `b` is, once enough of it is in to
/// say, and whether it uses the longer header.
pub fn request_len(order: Order, b: &[u8]) -> Option<(usize, bool)> {
    if b.len() < 4 {
        return None;
    }
    match usize::from(order.u16(b, 2)) {
        0 if b.len() < 8 => None,
        // Never shorter than its own header.
        0 => Some(((order.u32(b, 4) as usize).max(2) * 4, true)),
        n => Some((n * 4, false)),
    }
}

/// How long the reply, error or event starting with these 32 bytes is.
pub fn packet_len(order: Order, head: &[u8]) -> usize {
    if head[0] == 1 || head[0] & 0x7f == 35 {
        32 + 4 * order.u32(head, 4) as usize
    } else {
        32
    }
}

/// A reply's or an error's sequence number; an event's too (all but KeymapNotify).
pub fn sequence(order: Order, head: &[u8]) -> Option<u16> {
    (head[0] & 0x7f != 11).then(|| order.u16(head, 2))
}

/// How many bytes of picture GetImage returns for a `w` x `h` image of
/// `depth`: Z format (2) as the display stores pixels, XY format (1) one
/// bit-plane after another, only the planes asked for.
pub fn image_bytes(setup: &Setup, format: u8, depth: u8, plane_mask: u32, w: u16, h: u16) -> usize {
    let (w, h) = (usize::from(w), usize::from(h));
    let line = |bits: usize, pad: u8| -> usize {
        let pad = usize::from(pad.max(8));
        bits.div_ceil(pad) * pad / 8
    };
    if format == 2 {
        let (bpp, pad) = setup.formats.iter().find(|f| f.0 == depth).map_or((32, 32), |f| (f.1, f.2));
        line(w * usize::from(bpp), pad) * h
    } else {
        let all = if depth >= 32 { u32::MAX } else { (1u32 << depth) - 1 };
        (plane_mask & all).count_ones() as usize * line(w, setup.bitmap_pad) * h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LE: Order = Order(true);

    #[test]
    fn reads_the_programs_first_message() {
        let mut b = setup_request_with(LE, 11, 0, &[7; 16]);
        assert_eq!(setup_request(&b), Some((LE, 11, 0, b.len())));
        assert_eq!(b.len(), 12 + 20 + 16);
        b[0] = b'B';
        assert_eq!(setup_request(&b).map(|s| s.0), Some(Order(false)));
        assert_eq!(setup_request(b"x"), None);
    }

    #[test]
    fn requests_long_and_short() {
        // GetInputFocus: one unit.
        assert_eq!(request_len(LE, &[43, 0, 1, 0]), Some((4, false)));
        // BIG-REQUESTS: 0 in the short length, the real one after.
        assert_eq!(request_len(LE, &[72, 2, 0, 0, 0x10, 0, 0, 0]), Some((64, true)));
        assert_eq!(request_len(LE, &[72, 2, 0, 0]), None, "not enough in yet to say");
        // A big length of 0 or 1 would be shorter than its header.
        assert_eq!(request_len(LE, &[72, 2, 0, 0, 0, 0, 0, 0]), Some((8, true)));
    }

    #[test]
    fn big_requests_fields_sit_later() {
        let mut b = vec![73u8, 2, 0, 0, 6, 0, 0, 0];
        b.extend_from_slice(&0x1234u32.to_le_bytes());
        b.resize(24, 0);
        let r = Req { order: LE, bytes: &b, big: true };
        assert_eq!(r.u32(4), Some(0x1234), "GetImage's drawable, after the longer header");
    }

    #[test]
    fn replies_and_events() {
        let mut reply = [0u8; 32];
        reply[0] = 1;
        reply[4] = 3;
        assert_eq!(packet_len(LE, &reply), 44);
        let mut generic = [0u8; 32];
        generic[0] = 35 | 0x80;
        generic[4] = 1;
        assert_eq!(packet_len(LE, &generic), 36);
        let event = [12u8; 32];
        assert_eq!(packet_len(LE, &event), 32);
        assert_eq!(sequence(LE, &[11u8; 32]), None, "KeymapNotify has none");
    }

    #[test]
    fn picture_sizes() {
        let setup = Setup { bitmap_pad: 32, formats: vec![(1, 1, 32), (24, 32, 32)], ..Default::default() };
        assert_eq!(image_bytes(&setup, 2, 24, !0, 10, 3), 10 * 4 * 3);
        // XY: 24 planes of 10 bits, each line padded to 32 bits.
        assert_eq!(image_bytes(&setup, 1, 24, !0, 10, 3), 24 * 4 * 3);
        assert_eq!(image_bytes(&setup, 1, 24, 0b101, 10, 3), 2 * 4 * 3, "only the planes asked for");
        assert_eq!(image_bytes(&setup, 2, 1, !0, 33, 1), 8);
    }
}
