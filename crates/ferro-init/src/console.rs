//! The rescue console on /dev/console, running the shared ferro-cmd
//! interpreter with PID 1's own powers (supervised children, real shutdown).

use crate::supervisor::Supervisor;
use crate::system;
use ferro_path::DriveTable;
use std::io::{self, Write};

struct InitHost<'a>(&'a Supervisor);

impl ferro_cmd::Host for InitHost<'_> {
    fn run(&mut self, path: &str, args: &[&str], cwd: &str, _out: &mut dyn Write) -> io::Result<i32> {
        self.0.run_foreground(path, args, cwd)
    }

    fn shutdown(&mut self, reboot: bool) -> Option<String> {
        system::shutdown(reboot)
    }
}

pub fn run(sup: &Supervisor, drives: &DriveTable) -> ! {
    let (stdin, stdout) = (io::stdin(), io::stdout());
    loop {
        let mut out = stdout.lock();
        let _ = writeln!(out, "FerroOS rescue console. Type HELP for commands.\n");
        let _ = ferro_cmd::repl(&mut stdin.lock(), &mut out, &mut InitHost(sup), drives);
        drop(out);
        // EXIT just restarts the console. At EOF there is no usable console
        // input, so PID 1 idles; it must never exit.
        if stdin.lock().fill_buf_is_eof() {
            loop {
                std::thread::park();
            }
        }
    }
}

trait EofProbe {
    fn fill_buf_is_eof(&mut self) -> bool;
}

impl EofProbe for io::StdinLock<'_> {
    fn fill_buf_is_eof(&mut self) -> bool {
        use std::io::BufRead;
        self.fill_buf().map_or(true, |b| b.is_empty())
    }
}
