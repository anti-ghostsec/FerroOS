//! Character-art sprites. Each byte is one pixel looked up in [`palette`];
//! `.` is transparent. Icons are 16x16 and drawn at 2x for "large icons".

use crate::color::*;

pub type Icon = &'static [&'static str];

pub fn palette(ch: u8) -> Option<u32> {
    Some(match ch {
        b'k' => BLACK,
        b'w' => WHITE,
        b'g' => FACE,
        b'l' => LIGHT,
        b'd' => SHADOW,
        b'b' => NAVY,
        b'B' => 0x0000FF,
        b'c' => 0x00FFFF,
        b'y' => 0xFFFF00,
        b'Y' => 0x808000,
        b'G' => 0x00FF00,
        b'o' => 0xE0601C, // rust orange
        b'R' => 0x802000,
        _ => return None,
    })
}

pub const COMPUTER: Icon = &[
    "................",
    ".kkkkkkkkkkkkk..",
    ".kwgggggggggdk..",
    ".kgkkkkkkkkgdk..",
    ".kgkcccccbkgdk..",
    ".kgkcccccbkgdk..",
    ".kgkcccccbkgdk..",
    ".kgkbbbbbbkgdk..",
    ".kgkkkkkkkkgdk..",
    ".kggggggggggdk..",
    ".kkkkkkkkkkkkk..",
    "....kkgggggkk...",
    "..kkwwwwwwwwwkk.",
    ".kwggggggggggggk",
    ".kgggggggggddddk",
    ".kkkkkkkkkkkkkkk",
];

pub const RECYCLE: Icon = &[
    "................",
    "......kkkk......",
    "...kkkwwwwkkk...",
    "..kwwwwwwwwwwk..",
    "..kkkkkkkkkkkk..",
    "...kwgwgwgwgk...",
    "...kwgwgwgwgk...",
    "...kwgwgwgwgk...",
    "...kwgwgwgwgk...",
    "...kwgwgwgwgk...",
    "...kwgwgwgwgk...",
    "...kwgwgwgwgk...",
    "...kwgwgwgwgk...",
    "....kwgwgwgk....",
    "....kkkkkkkk....",
    "................",
];

pub const FOLDER: Icon = &[
    "................",
    "................",
    "..kkkkk.........",
    ".kyYyYyk........",
    ".kkkkkkkkkkkkk..",
    ".kywywywywywyYk.",
    ".kwyyyyyyyyyyYk.",
    ".kyyyyyyyyyyyYk.",
    ".kyyyyyyyyyyyYk.",
    ".kyyyyyyyyyyyYk.",
    ".kyyyyyyyyyyyYk.",
    ".kyyyyyyyyyyyYk.",
    ".kYYYYYYYYYYYYk.",
    ".kkkkkkkkkkkkkk.",
    "................",
    "................",
];

pub const FILE: Icon = &[
    "................",
    "..kkkkkkkkk.....",
    "..kwwwwwwwkk....",
    "..kwwwwwwwkwk...",
    "..kwwwwwwwkkkk..",
    "..kwkkkkkwwwdk..",
    "..kwwwwwwwwwdk..",
    "..kwkkkkkkkwdk..",
    "..kwwwwwwwwwdk..",
    "..kwkkkkkkkwdk..",
    "..kwwwwwwwwwdk..",
    "..kwkkkkkwwwdk..",
    "..kwwwwwwwwwdk..",
    "..kwwwwwwwwwdk..",
    "..kkkkkkkkkkkk..",
    "................",
];

pub const DRIVE: Icon = &[
    "................",
    "................",
    "................",
    "................",
    "................",
    "..kkkkkkkkkkkk..",
    ".kwwwwwwwwwwwwk.",
    ".kwggggggggggdk.",
    ".kggggggggggGdk.",
    ".kddddddddddddk.",
    ".kkkkkkkkkkkkkk.",
    "................",
    "................",
    "................",
    "................",
    "................",
];

pub const HELP: Icon = &[
    "................",
    "...kkkkkkkkkk...",
    "..kBBBBBBBBBBk..",
    "..kBBBkkkkBBBk..",
    "..kBBkwwwwkBBk..",
    "..kBBBBBBkwBBk..",
    "..kBBBBBkwkBBk..",
    "..kBBBBkwkBBBk..",
    "..kBBBBkwkBBBk..",
    "..kBBBBBBBBBBk..",
    "..kBBBBkwkBBBk..",
    "..kBBBBkkkBBBk..",
    "..kBBBBBBBBBBk..",
    "..kwwwwwwwwwwk..",
    "..kkkkkkkkkkkk..",
    "................",
];

/// The FerroOS logo: an iron-oxide tile with an "F".
pub const FERRO: Icon = &[
    "................",
    ".kkkkkkkkkkkkkk.",
    ".koooooooooooRk.",
    ".kowwwwwwwwooRk.",
    ".kowwwwwwwwooRk.",
    ".kowwooooooooRk.",
    ".kowwooooooooRk.",
    ".kowwwwwwooooRk.",
    ".kowwwwwwooooRk.",
    ".kowwooooooooRk.",
    ".kowwooooooooRk.",
    ".kowwooooooooRk.",
    ".kowwooooooooRk.",
    ".kRRRRRRRRRRRRk.",
    ".kkkkkkkkkkkkkk.",
    "................",
];

pub const CURSOR: Icon = &[
    "k...........",
    "kk..........",
    "kwk.........",
    "kwwk........",
    "kwwwk.......",
    "kwwwwk......",
    "kwwwwwk.....",
    "kwwwwwwk....",
    "kwwwwwwwk...",
    "kwwwwwwwwk..",
    "kwwwwwkkkkk.",
    "kwwkwwk.....",
    "kwk.kwwk....",
    "kk..kwwk....",
    "k....kwwk...",
    ".....kwwk...",
    "......kwwk..",
    "......kwwk..",
    ".......kk...",
];

pub const RADIO: Icon = &[
    "....dddd....",
    "..ddkkkkdd..",
    ".dkkwwwwkkl.",
    ".dkwwwwwwlw.",
    "dkwwwwwwwwlw",
    "dkwwwwwwwwlw",
    "dkwwwwwwwwlw",
    "dkwwwwwwwwlw",
    ".dkwwwwwwlw.",
    ".dlwwwwwwlw.",
    "..wwllllww..",
    "....wwww....",
];

// Monochrome glyphs for window buttons and menus (drawn with `sprite_mono`).

pub const GLYPH_CLOSE: Icon = &["kk....kk", ".kk..kk.", "..kkkk..", "...kk...", "..kkkk..", ".kk..kk.", "kk....kk"];

pub const GLYPH_MIN: Icon = &["......", "......", "......", "......", "......", "kkkkkk", "kkkkkk"];

pub const GLYPH_MAX: Icon = &["kkkkkkkkk", "kkkkkkkkk", "k.......k", "k.......k", "k.......k", "k.......k", "kkkkkkkkk"];

pub const GLYPH_RESTORE: Icon = &["..kkkkkk", "..kkkkkk", "..k....k", "kkkkkk.k", "kkkkkk.k", "k....kkk", "k....k..", "k....k..", "kkkkkk.."];

pub const GLYPH_SUBMENU: Icon = &["k...", "kk..", "kkk.", "kkkk", "kkk.", "kk..", "k..."];

pub const ARROW_UP: Icon = &["...k...", "..kkk..", ".kkkkk.", "kkkkkkk"];

pub const ARROW_DOWN: Icon = &["kkkkkkk", ".kkkkk.", "..kkk..", "...k..."];

/// Command Prompt: a black console window with a green prompt.
pub const TERMINAL: Icon = &[
    "................",
    ".kkkkkkkkkkkkkk.",
    ".kbbbbbbbbbbwbk.",
    ".kkkkkkkkkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    ".kkGkkkkkkkkkkk.",
    ".kkkGkkkkkkkkkk.",
    ".kkkkGkkkkkkkkk.",
    ".kkkGkkkkkkkkkk.",
    ".kkGkkGGGGkkkkk.",
    ".kkkkkkkkkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    "................",
];

/// Tray: network kill switch.
pub const NETWORK: Icon = &[
    "................",
    ".....kkkkkk.....",
    "...kkBBGGBBkk...",
    "..kBBGGGGBBBBk..",
    "..kBGGGGBBBBBk..",
    ".kBBGGGBBBBBBBk.",
    ".kBBBGBBBBGGBBk.",
    ".kBBBBBBBGGGGBk.",
    ".kBBBBBBGGGGGBk.",
    ".kBBBBBBBGGGBBk.",
    "..kBBBBBBBGBBk..",
    "..kBBBBBBBBBBk..",
    "...kkBBBBBBkk...",
    ".....kkkkkk.....",
    "................",
    "................",
];

/// Tray: microphone kill switch.
pub const MICROPHONE: Icon = &[
    "................",
    "......kkkk......",
    ".....kddddk.....",
    ".....kdlldk.....",
    ".....kdlldk.....",
    ".....kdlldk.....",
    "...k.kdlldk.k...",
    "...k.kddddk.k...",
    "...k..kkkk..k...",
    "....k......k....",
    ".....kkkkkk.....",
    ".......kk.......",
    ".......kk.......",
    ".....kkkkkk.....",
    "................",
    "................",
];

/// Tray: camera kill switch.
pub const CAMERA: Icon = &[
    "................",
    "................",
    "................",
    "....kkkk........",
    ".kkkkkkkkkkkkkk.",
    ".kggggkkkkggggk.",
    ".kgggkwwwwkgggk.",
    ".kggkwbbbbwkggk.",
    ".kggkwbbbbwkggk.",
    ".kgggkwwwwkgggk.",
    ".kggggkkkkggggk.",
    ".kkkkkkkkkkkkkk.",
    "................",
    "................",
    "................",
    "................",
];

pub const GLYPH_CHECK: Icon = &["......k", ".....kk", "k...kkk", "kk.kkk.", "kkkkk..", ".kkk...", "..k...."];

/// Task Manager window icon: a tiny green-on-black monitor graph.
pub const TASKMGR: Icon = &[
    "................",
    ".kkkkkkkkkkkkkk.",
    ".kddddddddddddk.",
    ".kdkkkkkkkkkkdk.",
    ".kdkkkkkkkkGkdk.",
    ".kdkkkkkkkGkkdk.",
    ".kdkkGkkkGkkkdk.",
    ".kdkGkGkGkkkkdk.",
    ".kdGkkkGkkkkkdk.",
    ".kdkkkkkkkkkkdk.",
    ".kddddddddddddk.",
    ".kkkkkkkkkkkkkk.",
    "......kddk......",
    "....kkkkkkkk....",
    "....kggggggk....",
    "....kkkkkkkk....",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprites_are_rectangular() {
        let all: &[(&str, Icon)] = &[
            ("COMPUTER", COMPUTER),
            ("RECYCLE", RECYCLE),
            ("FOLDER", FOLDER),
            ("FILE", FILE),
            ("DRIVE", DRIVE),
            ("HELP", HELP),
            ("FERRO", FERRO),
            ("CURSOR", CURSOR),
            ("RADIO", RADIO),
            ("GLYPH_CLOSE", GLYPH_CLOSE),
            ("GLYPH_MIN", GLYPH_MIN),
            ("GLYPH_MAX", GLYPH_MAX),
            ("GLYPH_RESTORE", GLYPH_RESTORE),
            ("GLYPH_SUBMENU", GLYPH_SUBMENU),
            ("ARROW_UP", ARROW_UP),
            ("ARROW_DOWN", ARROW_DOWN),
            ("TASKMGR", TASKMGR),
            ("TERMINAL", TERMINAL),
            ("NETWORK", NETWORK),
            ("MICROPHONE", MICROPHONE),
            ("CAMERA", CAMERA),
            ("GLYPH_CHECK", GLYPH_CHECK),
        ];
        for &(name, icon) in all {
            let w = icon[0].len();
            assert!(icon.iter().all(|r| r.len() == w), "{name} has ragged rows");
        }
        for icon in [COMPUTER, RECYCLE, FOLDER, FILE, DRIVE, HELP, FERRO, TASKMGR] {
            assert_eq!((icon.len(), icon[0].len()), (16, 16));
        }
    }
}
