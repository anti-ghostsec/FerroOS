//! Command Prompt sessions on a Linux pseudo-terminal. The kernel's line
//! discipline gives ferro-cmd echo, line editing and Ctrl+C for free.

use crate::TermSession;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

const PROGRAM: &str = "/bin/ferro-cmd";

pub struct PtySession {
    master: File,
    child: Child,
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 }
}

pub fn spawn(cols: u16, rows: u16, command: Option<&str>) -> io::Result<Box<dyn TermSession>> {
    let (mut master, mut slave) = (0, 0);
    let ws = winsize(cols, rows);
    // SAFETY: out-pointers are valid; name/termios are optional (null).
    if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &ws) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty returned two fresh fds that we now own.
    let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    // SAFETY: plain fcntl on our own fd.
    unsafe {
        let fd = master.as_raw_fd();
        libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }

    let mut cmd = Command::new(PROGRAM);
    if let Some(first) = command {
        cmd.args(["/K", first]);
    }
    cmd.stdin(Stdio::from(slave.try_clone()?)).stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave)).env("TERM", "ansi");
    // SAFETY: setsid/ioctl are async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            // New session with the pty as its controlling terminal, so
            // Ctrl+C and hangup reach the programs running inside it.
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    Ok(Box::new(PtySession { master, child }))
}

impl TermSession for PtySession {
    fn write(&mut self, data: &[u8]) {
        let _ = self.master.write_all(data);
    }

    fn read_into(&mut self, out: &mut Vec<u8>) {
        let mut buf = [0u8; 4096];
        // EAGAIN ends the loop; EIO means the program exited.
        while let Ok(n @ 1..) = self.master.read(&mut buf) {
            out.extend_from_slice(&buf[..n]);
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let ws = winsize(cols, rows);
        // SAFETY: TIOCSWINSZ takes a pointer to winsize.
        unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) };
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // Closing the window ends the session; reap so no zombie is left.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
