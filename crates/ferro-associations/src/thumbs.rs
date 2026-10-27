//! Thumbnails: a picture's own pixels, or a frame from a video, made small.
//! Shared by the file manager and the desktop; made aside (a big picture or
//! a video takes a moment), never on a window's own thread.

use ferro_gfx::DecodedImage;
use std::path::Path;

/// Pictures that get a thumbnail.
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "webp", "gif", "ppm"];
/// Videos that get one: a frame from them.
pub const VIDEO_EXTS: &[&str] =
    &["mp4", "m4v", "mkv", "webm", "avi", "mov", "flv", "wmv", "mpg", "mpeg", "ts", "m2ts", "mts", "3gp", "ogv", "vob", "divx", "asf"];

fn ext(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
}

/// Whether `path` gets a thumbnail at all.
pub fn has_thumbnail(path: &Path) -> bool {
    let e = ext(path);
    IMAGE_EXTS.contains(&e.as_str()) || VIDEO_EXTS.contains(&e.as_str())
}

/// The thumbnail, `size` pixels square at most; `None` if it can't be made.
pub fn thumbnail(path: &Path, size: u32) -> Option<DecodedImage> {
    if VIDEO_EXTS.contains(&ext(path).as_str()) {
        video_frame(path, size)
    } else {
        std::fs::read(path).ok().and_then(|b| DecodedImage::thumbnail_fit(&b, size))
    }
}

/// A frame from a video, a second in (or its first, for a shorter one), by
/// ffmpeg inside ferro-jail: a booby-trapped video that took ffmpeg over
/// could neither reach the internet nor start anything.
pub fn video_frame(path: &Path, size: u32) -> Option<DecodedImage> {
    let filter = format!("scale={size}:{size}:force_original_aspect_ratio=decrease,pad={size}:{size}:(ow-iw)/2:(oh-ih)/2:white");
    for at in ["1", "0"] {
        let head = ["-nostdin", "-loglevel", "quiet", "-ss", at, "-i"];
        let tail = ["-frames:v", "1", "-vf", filter.as_str(), "-f", "image2pipe", "-vcodec", "bmp", "-"];
        let run = |cmd: &mut std::process::Command| cmd.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok();
        let out = match run(std::process::Command::new("ferro-jail").arg("ffmpeg").args(head).arg(path).args(tail)) {
            Some(o) => o,
            // Where ferro-jail isn't installed (building FerroOS elsewhere).
            None => run(std::process::Command::new("ffmpeg").args(head).arg(path).args(tail))?,
        };
        if let Some(img) = (!out.stdout.is_empty()).then(|| DecodedImage::decode(&out.stdout)).flatten() {
            return Some(img);
        }
    }
    None
}
