//! Interactive line editor for ferro-cmd.
//!
//! Provides raw-mode readline with ANSI arrow-key cursor movement,
//! command history (Up/Down), Home/End/Delete/Backspace, and non-TTY fallback
//! for pipes and tests. Does not intercept mouse clicks so terminal context
//! menus (Copy/Paste) remain 100% functional.

use std::io::{self, BufRead, Write};

#[cfg(unix)]
use std::io::{IsTerminal, Read};

/// Reads one command line interactively or from a piped stream.
pub fn read_line(prompt: &str, input: &mut dyn BufRead, out: &mut dyn Write, history: &mut Vec<String>) -> io::Result<Option<String>> {
    #[cfg(unix)]
    {
        if std::io::stdin().is_terminal() {
            return read_line_interactive(prompt, input, out, history);
        }
    }
    #[cfg(not(unix))]
    let _ = history;

    if !prompt.is_empty() {
        write!(out, "{prompt}")?;
        out.flush()?;
    }
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
    Ok(Some(trimmed))
}

/// Reads a password without echoing characters.
pub fn read_password(input: &mut dyn BufRead, out: &mut dyn Write) -> io::Result<Option<String>> {
    #[cfg(unix)]
    {
        if std::io::stdin().is_terminal() {
            let _raw = RawModeGuard::new()?;
            let mut buf = String::new();
            let mut byte = [0u8; 1];
            while input.read_exact(&mut byte).is_ok() {
                match byte[0] {
                    b'\r' | b'\n' => {
                        write!(out, "\r\n")?;
                        out.flush()?;
                        return Ok(Some(buf));
                    }
                    b'\x03' => {
                        // Ctrl+C
                        write!(out, "^C\r\n")?;
                        out.flush()?;
                        return Ok(Some(String::new()));
                    }
                    b'\x04' if buf.is_empty() => {
                        // Ctrl+D
                        return Ok(None);
                    }
                    0x7f | 0x08 => {
                        // Backspace
                        buf.pop();
                    }
                    b if b >= 0x20 && b != 0x7f => {
                        buf.push(b as char);
                    }
                    _ => {}
                }
            }
            return Ok(None);
        }
    }
    #[cfg(not(unix))]
    let _ = out;

    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_string()))
}

#[cfg(unix)]
struct RawModeGuard {
    orig: libc::termios,
}

#[cfg(unix)]
impl RawModeGuard {
    fn new() -> io::Result<Self> {
        let fd = libc::STDIN_FILENO;
        unsafe {
            let mut orig = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut orig) != 0 {
                return Err(io::Error::last_os_error());
            }
            let mut raw = orig;
            // Disable canonical mode and echo so keys are received immediately
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { orig })
        }
    }
}

#[cfg(unix)]
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.orig);
        }
    }
}

#[cfg(unix)]
fn redraw(out: &mut dyn Write, prompt: &str, buffer: &[char], cursor: usize) -> io::Result<()> {
    let text: String = buffer.iter().collect();
    write!(out, "\r{prompt}{text}\x1b[K")?;
    let prompt_len = prompt.chars().count();
    let target = prompt_len + cursor;
    write!(out, "\r")?;
    if target > 0 {
        write!(out, "\x1b[{}C", target)?;
    }
    out.flush()
}

#[cfg(unix)]
fn read_line_interactive(prompt: &str, input: &mut dyn BufRead, out: &mut dyn Write, history: &mut Vec<String>) -> io::Result<Option<String>> {
    let _raw = RawModeGuard::new()?;

    let mut buffer: Vec<char> = Vec::new();
    let mut cursor: usize = 0;
    let mut history_idx: Option<usize> = None;
    let mut saved_buffer: Vec<char> = Vec::new();

    redraw(out, prompt, &buffer, cursor)?;

    let mut read_byte = || -> io::Result<Option<u8>> {
        let mut b = [0u8; 1];
        match input.read_exact(&mut b) {
            Ok(()) => Ok(Some(b[0])),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e),
        }
    };

    loop {
        let Some(b) = read_byte()? else {
            return Ok(None);
        };

        match b {
            b'\r' | b'\n' => {
                let res: String = buffer.iter().collect();
                if !res.is_empty() && history.last().map(String::as_str) != Some(&res) {
                    history.push(res.clone());
                }
                write!(out, "\r\n")?;
                out.flush()?;
                return Ok(Some(res));
            }
            b'\x03' => {
                // Ctrl+C: Cancel current input, print ^C, return empty line
                write!(out, "^C\r\n")?;
                out.flush()?;
                return Ok(Some(String::new()));
            }
            b'\x04' => {
                // Ctrl+D: EOF if buffer is empty
                if buffer.is_empty() {
                    write!(out, "\r\n")?;
                    out.flush()?;
                    return Ok(None);
                }
            }
            0x7f | 0x08 => {
                // Backspace (ASCII BS or DEL)
                if cursor > 0 {
                    cursor -= 1;
                    buffer.remove(cursor);
                    redraw(out, prompt, &buffer, cursor)?;
                }
            }
            b'\x01' => {
                // Ctrl+A: Home
                cursor = 0;
                redraw(out, prompt, &buffer, cursor)?;
            }
            b'\x05' => {
                // Ctrl+E: End
                cursor = buffer.len();
                redraw(out, prompt, &buffer, cursor)?;
            }
            b'\x0b' => {
                // Ctrl+K: Kill to end
                buffer.truncate(cursor);
                redraw(out, prompt, &buffer, cursor)?;
            }
            b'\x15' => {
                // Ctrl+U: Kill line to cursor
                buffer.drain(..cursor);
                cursor = 0;
                redraw(out, prompt, &buffer, cursor)?;
            }
            b'\x0c' => {
                // Ctrl+L: Clear screen
                write!(out, "\x1b[2J\x1b[H")?;
                redraw(out, prompt, &buffer, cursor)?;
            }
            b'\x1b' => {
                // ANSI / VT escape sequence
                let Some(b2) = read_byte()? else { break };
                if b2 == b'[' || b2 == b'O' {
                    let Some(b3) = read_byte()? else { break };
                    match b3 {
                        b'D' => {
                            // Left Arrow: move cursor left
                            if cursor > 0 {
                                cursor -= 1;
                                redraw(out, prompt, &buffer, cursor)?;
                            }
                        }
                        b'C' => {
                            // Right Arrow: move cursor right
                            if cursor < buffer.len() {
                                cursor += 1;
                                redraw(out, prompt, &buffer, cursor)?;
                            }
                        }
                        b'A' => {
                            // Up Arrow: History previous
                            if history_idx.is_none() {
                                saved_buffer = buffer.clone();
                                if !history.is_empty() {
                                    history_idx = Some(history.len() - 1);
                                }
                            } else if let Some(idx) = history_idx {
                                if idx > 0 {
                                    history_idx = Some(idx - 1);
                                }
                            }
                            if let Some(idx) = history_idx {
                                if let Some(entry) = history.get(idx) {
                                    buffer = entry.chars().collect();
                                    cursor = buffer.len();
                                    redraw(out, prompt, &buffer, cursor)?;
                                }
                            }
                        }
                        b'B' => {
                            // Down Arrow: History next
                            if let Some(idx) = history_idx {
                                if idx + 1 < history.len() {
                                    history_idx = Some(idx + 1);
                                    if let Some(entry) = history.get(idx + 1) {
                                        buffer = entry.chars().collect();
                                        cursor = buffer.len();
                                        redraw(out, prompt, &buffer, cursor)?;
                                    }
                                } else {
                                    history_idx = None;
                                    buffer = saved_buffer.clone();
                                    cursor = buffer.len();
                                    redraw(out, prompt, &buffer, cursor)?;
                                }
                            }
                        }
                        b'H' => {
                            // Home
                            cursor = 0;
                            redraw(out, prompt, &buffer, cursor)?;
                        }
                        b'F' => {
                            // End
                            cursor = buffer.len();
                            redraw(out, prompt, &buffer, cursor)?;
                        }
                        b'1'..=b'8' => {
                            // Extended key like Delete: \x1b[3~
                            let mut num_str = String::new();
                            num_str.push(b3 as char);
                            let mut term = 0u8;
                            while let Some(nb) = read_byte()? {
                                if nb == b'~' {
                                    term = nb;
                                    break;
                                } else if nb.is_ascii_digit() {
                                    num_str.push(nb as char);
                                } else {
                                    break;
                                }
                            }
                            if term == b'~' {
                                match num_str.as_str() {
                                    "1" | "7" => {
                                        cursor = 0;
                                        redraw(out, prompt, &buffer, cursor)?;
                                    }
                                    "4" | "8" => {
                                        cursor = buffer.len();
                                        redraw(out, prompt, &buffer, cursor)?;
                                    }
                                    "3" => {
                                        // Delete key
                                        if cursor < buffer.len() {
                                            buffer.remove(cursor);
                                            redraw(out, prompt, &buffer, cursor)?;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Regular UTF-8 characters:
            byte if byte >= 0x20 => {
                let char_len = if byte < 0x80 {
                    1
                } else if byte & 0xe0 == 0xc0 {
                    2
                } else if byte & 0xf0 == 0xe0 {
                    3
                } else if byte & 0xf8 == 0xf0 {
                    4
                } else {
                    1
                };

                let mut bytes = vec![byte];
                for _ in 1..char_len {
                    if let Some(next_b) = read_byte()? {
                        bytes.push(next_b);
                    } else {
                        break;
                    }
                }
                if let Ok(s) = std::str::from_utf8(&bytes) {
                    for ch in s.chars() {
                        buffer.insert(cursor, ch);
                        cursor += 1;
                    }
                    redraw(out, prompt, &buffer, cursor)?;
                }
            }
            _ => {}
        }
    }

    Ok(None)
}
