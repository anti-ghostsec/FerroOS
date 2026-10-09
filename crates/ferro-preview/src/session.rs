//! Command Prompt sessions for the preview: ferro-cmd runs on a thread in
//! this process, connected by channels. Windows has no pty, so this session
//! does the line discipline a pty would (echo, backspace, line buffering).

use ferro_path::DriveTable;
use ferro_shell::TermSession;
use std::io::{self, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub struct LocalSession {
    input: Sender<Vec<u8>>,
    output: Receiver<Vec<u8>>,
    echo: Vec<u8>,
    line: Vec<u8>,
    last_cr: bool,
    done: Arc<AtomicBool>,
}

struct ChanReader {
    rx: Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChanReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.buf.len() {
            match self.rx.recv() {
                Ok(b) => (self.buf, self.pos) = (b, 0),
                Err(_) => return Ok(0), // window closed: EOF
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

struct ChanWriter(Sender<Vec<u8>>);

impl Write for ChanWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.send(data.to_vec()).map_err(|_| io::ErrorKind::BrokenPipe)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs programs with their output captured into the terminal.
struct CaptureHost;

impl ferro_cmd::Host for CaptureHost {
    fn run(&mut self, path: &str, args: &[&str], cwd: &str, out: &mut dyn Write) -> io::Result<i32> {
        let o = std::process::Command::new(path).args(args).current_dir(cwd).output()?;
        out.write_all(&o.stdout)?;
        out.write_all(&o.stderr)?;
        Ok(o.status.code().unwrap_or(-1))
    }

    fn shutdown(&mut self, _: bool) -> Option<String> {
        Some("Use Shut Down... on the Start menu.".into())
    }
}

pub fn spawn(_cols: u16, _rows: u16, command: Option<&str>) -> io::Result<Box<dyn TermSession>> {
    let first = command.map(str::to_owned);
    let (in_tx, in_rx) = channel();
    let (out_tx, out_rx) = channel();
    let done = Arc::new(AtomicBool::new(false));
    let finished = Arc::clone(&done);
    std::thread::Builder::new().name("ferro-cmd".into()).spawn(move || {
        let mut drives = DriveTable::empty();
        drives.mount('C', if cfg!(windows) { "C:/" } else { "/" });
        let mut input = BufReader::new(ChanReader { rx: in_rx, buf: Vec::new(), pos: 0 });
        let mut out = ChanWriter(out_tx);
        let _ = ferro_cmd::banner(&mut out);
        let _ = ferro_cmd::repl_from(first.as_deref(), &mut input, &mut out, &mut CaptureHost, &drives);
        finished.store(true, Ordering::SeqCst);
    })?;
    Ok(Box::new(LocalSession { input: in_tx, output: out_rx, echo: Vec::new(), line: Vec::new(), last_cr: false, done }))
}

impl TermSession for LocalSession {
    fn write(&mut self, data: &[u8]) {
        if data.first() == Some(&0x1B) {
            return; // arrows etc.: no line history yet
        }
        for &b in data {
            match b {
                0x7F | 0x08 => {
                    if self.line.pop().is_some() {
                        self.echo.extend_from_slice(b"\x08 \x08");
                    }
                }
                b'\r' => {
                    self.echo.extend_from_slice(b"\r\n");
                    let mut line = std::mem::take(&mut self.line);
                    line.push(b'\n');
                    let _ = self.input.send(line);
                }
                0x03 => {
                    self.echo.extend_from_slice(b"^C\r\n");
                    self.line.clear();
                    let _ = self.input.send(b"\n".to_vec());
                }
                0x20.. => {
                    self.line.push(b);
                    self.echo.push(b);
                }
                _ => {}
            }
        }
    }

    fn read_into(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.echo);
        while let Ok(chunk) = self.output.try_recv() {
            // A pty's ONLCR: turn bare \n into \r\n.
            for b in chunk {
                if b == b'\n' && !self.last_cr {
                    out.push(b'\r');
                }
                self.last_cr = b == b'\r';
                out.push(b);
            }
        }
    }

    fn resize(&mut self, _cols: u16, _rows: u16) {}

    fn is_alive(&mut self) -> bool {
        !self.done.load(Ordering::SeqCst)
    }
}
