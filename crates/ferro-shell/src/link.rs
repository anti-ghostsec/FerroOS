//! Talks to ferro-system over its Unix socket. The shell runs as the
//! unprivileged desktop user; everything needing root goes through here.

use crate::{SystemLink, VaultState};
use ferro_system::SOCKET;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub struct SocketLink;

/// One request per connection: send a line, read one line back.
fn request(line: &str) -> Result<String, String> {
    let mut last = String::new();
    // ferro-system may still be starting right after boot.
    for _ in 0..30 {
        match UnixStream::connect(SOCKET) {
            Ok(mut s) => {
                // Unlocking runs Argon2id; allow it time.
                let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
                writeln!(s, "{line}").map_err(|e| e.to_string())?;
                let mut reply = String::new();
                BufReader::new(s).read_line(&mut reply).map_err(|e| e.to_string())?;
                return Ok(reply.trim_end().to_owned());
            }
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("system service unavailable ({last})"))
}

fn ok(reply: Result<String, String>) -> Result<(), String> {
    match reply? {
        r if r == "OK" => Ok(()),
        r => Err(r.strip_prefix("ERR ").unwrap_or(&r).to_owned()),
    }
}

impl SystemLink for SocketLink {
    fn vault_state(&mut self) -> VaultState {
        request("STATUS").ok().and_then(|r| VaultState::parse(&r)).unwrap_or(VaultState::None)
    }

    fn create_vault(&mut self, password: &str) -> Result<(), String> {
        ok(request(&format!("CREATE {password}")))
    }

    fn unlock_vault(&mut self, password: &str) -> Result<(), String> {
        ok(request(&format!("UNLOCK {password}")))
    }

    fn amnesic(&mut self) {
        let _ = request("AMNESIC");
    }

    fn apply_switches(&mut self, s: ferro_sandbox::store::Switches) {
        let b = |v: bool| if v { 1 } else { 0 };
        let line = format!("SWITCHES {} {} {} {} {}", b(s.network), b(s.microphone), b(s.camera), b(s.vpn), b(s.tor));
        if let Err(e) = ok(request(&line)) {
            eprintln!("ferro-shell: kill switches: {e}");
        }
    }

    fn limit_memory(&mut self, pid: u32, bytes: Option<u64>) -> Result<(), String> {
        let b = bytes.map_or("none".to_owned(), |b| b.to_string());
        ok(request(&format!("LIMIT {pid} {b}")))
    }

    fn list_disks(&mut self) -> Result<Vec<crate::DiskInfo>, String> {
        let reply = request("DISKS")?;
        match reply.strip_prefix("ERR ") {
            Some(e) => Err(e.to_owned()),
            None => Ok(crate::DiskInfo::parse_list(&reply)),
        }
    }

    fn install(&mut self, disk: &str) -> Result<(), String> {
        ok(request(&format!("INSTALL {disk}")))
    }
}
