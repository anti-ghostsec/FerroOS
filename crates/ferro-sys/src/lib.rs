//! Readers for Linux system state shared by FerroOS components.
//!
//! Everything returns `None`/empty on hosts without `/proc`, so callers work
//! unchanged in the desktop preview.

use std::collections::HashMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

/// The idle RAM budget FerroOS is designed around.
pub const IDLE_BUDGET_KB: u64 = 50 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemInfo {
    /// What the kernel manages. Excludes the kernel image and boot-time
    /// reservations, so it understates the true footprint.
    pub total_kb: u64,
    pub available_kb: u64,
    /// Installed RAM from /proc/iomem (needs root for real addresses).
    pub physical_kb: Option<u64>,
    /// Free memory the kernel holds back: its emergency reserve (watermarks
    /// and zone reserves) and the free pages cached per CPU. MemAvailable
    /// leaves both out, but neither is in use.
    pub reserve_kb: u64,
}

impl MemInfo {
    pub fn read() -> Option<Self> {
        let mut m = Self::parse(&fs::read_to_string("/proc/meminfo").ok()?)?;
        // /proc/iomem needs root; init records the value for everyone else.
        m.physical_kb = fs::read_to_string("/proc/iomem")
            .ok()
            .and_then(|t| parse_iomem_ram_kb(&t))
            .or_else(|| fs::read_to_string("/run/ferro/physical-kb").ok()?.trim().parse().ok());
        m.reserve_kb = fs::read_to_string("/proc/zoneinfo").map_or(0, |t| parse_zoneinfo_reserve_kb(&t));
        Some(m)
    }

    /// Memory FerroOS uses: kernel image, kernel allocations, page cache
    /// that can't be dropped, and all processes. This is the number the idle
    /// budget is about, and it's the same on any amount of RAM: free memory
    /// the kernel merely holds in reserve doesn't count, and neither does
    /// its per-page bookkeeping, which is a fixed share of whatever RAM is
    /// installed.
    pub fn footprint_kb(&self) -> u64 {
        self.physical_kb.unwrap_or(self.total_kb).saturating_sub(self.available_kb + self.reserve_kb + self.page_tracking_kb())
    }

    /// The kernel's bookkeeping for every 4 KiB page of RAM (64 bytes each,
    /// so 1.6% of installed RAM), inside the kernel's reserved memory. Every
    /// OS has an equivalent, and it grows with RAM, not with use. Only known
    /// when installed RAM is (0 otherwise).
    pub fn page_tracking_kb(&self) -> u64 {
        self.physical_kb.map_or(0, |kb| kb / 64)
    }

    /// RAM the kernel keeps for itself before userspace starts.
    pub fn kernel_reserved_kb(&self) -> Option<u64> {
        Some(self.physical_kb?.saturating_sub(self.total_kb))
    }

    pub fn parse(text: &str) -> Option<Self> {
        let field = |name: &str| {
            text.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(':')).and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
        };
        Some(Self { total_kb: field("MemTotal")?, available_kb: field("MemAvailable")?, physical_kb: None, reserve_kb: 0 })
    }

    pub fn used_kb(&self) -> u64 {
        self.total_kb.saturating_sub(self.available_kb)
    }
}

/// Free memory the kernel holds back, from /proc/zoneinfo: per zone, the
/// high watermark plus the largest lowmem protection (`totalreserve_pages`),
/// plus every per-CPU free-page cache (`count:` under `pagesets`).
fn parse_zoneinfo_reserve_kb(text: &str) -> u64 {
    let mut pages = 0u64;
    let mut high = 0u64;
    for line in text.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("high ") {
            high = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("count:") {
            pages += v.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(p) = line.strip_prefix("protection:") {
            let max = p.trim_matches(|c| c == ' ' || c == '(' || c == ')').split(',').filter_map(|n| n.trim().parse::<u64>().ok()).max().unwrap_or(0);
            pages += std::mem::take(&mut high) + max;
        }
    }
    pages * 4
}

/// Sums the top-level "System RAM" ranges in /proc/iomem. Returns `None`
/// when addresses are hidden (non-root shows all zeros).
fn parse_iomem_ram_kb(text: &str) -> Option<u64> {
    let bytes: u64 = text
        .lines()
        .filter(|l| !l.starts_with(' ') && l.ends_with(": System RAM"))
        .filter_map(|l| {
            let (start, end) = l.split(" : ").next()?.split_once('-')?;
            let (s, e) = (u64::from_str_radix(start, 16).ok()?, u64::from_str_radix(end, 16).ok()?);
            Some(e.checked_sub(s)? + 1)
        })
        .sum();
    (bytes > 4096).then_some(bytes / 1024)
}

/// This process's real user ID (`None` on hosts without /proc).
pub fn current_uid() -> Option<u32> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|l| l.strip_prefix("Uid:")?.split_whitespace().next()?.parse().ok())
}

pub fn kernel_release() -> Option<String> {
    Some(fs::read_to_string("/proc/sys/kernel/osrelease").ok()?.trim().to_owned())
}

pub fn uptime_secs() -> Option<u64> {
    let text = fs::read_to_string("/proc/uptime").ok()?;
    Some(text.split_whitespace().next()?.parse::<f64>().ok()? as u64)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub rss_kb: u64,
    /// Heap and stacks only (`RssAnon`). Program code is shared between
    /// processes and lives in the RAM-backed root file system, so it isn't
    /// this process's alone.
    pub private_kb: u64,
    /// Cumulative user + system clock ticks.
    pub cpu_ticks: u64,
    /// Share of total machine CPU since the previous [`Sampler`] sample.
    pub cpu_percent: f32,
    /// Started through the app sandbox (ferro-run).
    pub sandboxed: bool,
    /// RAM budget (cgroup `memory.max`), if one is set.
    pub mem_limit_kb: Option<u64>,
    /// Real user ID (0 = root).
    pub uid: u32,
}

/// Sandbox state of a process from its cgroup: (sandboxed, RAM budget).
fn budget_of(pid: u32) -> (bool, Option<u64>) {
    let Some(cg) =
        fs::read_to_string(format!("/proc/{pid}/cgroup")).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("0::").map(str::to_owned)))
    else {
        return (false, None);
    };
    if !cg.starts_with("/ferro/") {
        return (false, None);
    }
    // The desktop session lives in the delegated tree too, but isn't an app.
    let sandboxed = !cg.starts_with("/ferro/manual.") && cg != "/ferro/desktop";
    let limit = fs::read_to_string(format!("/sys/fs/cgroup{cg}/memory.max")).ok().and_then(|v| v.trim().parse::<u64>().ok()).map(|b| b / 1024);
    (sandboxed, limit)
}

/// Parses `/proc/<pid>/stat`. `comm` may contain spaces and parentheses, so
/// fields are counted from the *last* `)`.
fn parse_stat(text: &str) -> Option<(String, u64)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text[open + 1..close].to_owned();
    let f: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // f[0] is state; utime and stime are stat fields 14 and 15.
    let ticks = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    Some((name, ticks))
}

/// All user processes, sorted by PID. Kernel threads have no RSS and are skipped.
pub fn processes() -> Vec<ProcInfo> {
    let Ok(dir) = fs::read_dir("/proc") else { return Vec::new() };
    let mut out: Vec<ProcInfo> = dir
        .filter_map(|e| {
            let pid: u32 = e.ok()?.file_name().to_str()?.parse().ok()?;
            let (name, cpu_ticks) = parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
            let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
            let rss = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
            let rss_kb = rss.split_whitespace().next()?.parse().ok()?;
            let private_kb = status.lines().find_map(|l| l.strip_prefix("RssAnon:")?.split_whitespace().next()?.parse().ok()).unwrap_or(rss_kb);
            let (sandboxed, mem_limit_kb) = budget_of(pid);
            let uid = status.lines().find_map(|l| l.strip_prefix("Uid:")?.split_whitespace().next()?.parse().ok()).unwrap_or(0);
            Some(ProcInfo { pid, name, rss_kb, private_kb, cpu_ticks, cpu_percent: 0.0, sandboxed, mem_limit_kb, uid })
        })
        .collect();
    out.sort_by_key(|p| p.pid);
    out
}

/// (busy, total) jiffies across all CPUs, from the first line of `/proc/stat`.
pub fn cpu_jiffies() -> Option<(u64, u64)> {
    parse_cpu_line(fs::read_to_string("/proc/stat").ok()?.lines().next()?)
}

fn parse_cpu_line(line: &str) -> Option<(u64, u64)> {
    let v: Vec<u64> = line.strip_prefix("cpu ")?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    let total: u64 = v.iter().take(8).sum(); // user..steal; guest is already in user
    let idle = v.get(3)? + v.get(4).unwrap_or(&0);
    Some((total - idle, total))
}

/// Turns cumulative tick counters into usage percentages between samples.
/// Holds one u64 per process seen last time, nothing more.
#[derive(Default)]
pub struct Sampler {
    prev_cpu: Option<(u64, u64)>,
    prev_ticks: HashMap<u32, u64>,
}

impl Sampler {
    /// Returns total CPU % and, if requested, the process list with per-process CPU %.
    pub fn sample(&mut self, with_processes: bool) -> (Option<f32>, Option<Vec<ProcInfo>>) {
        let now = cpu_jiffies();
        let elapsed = match (self.prev_cpu, now) {
            (Some((_, t0)), Some((_, t1))) if t1 > t0 => Some(t1 - t0),
            _ => None,
        };
        let cpu = match (self.prev_cpu, now, elapsed) {
            (Some((b0, _)), Some((b1, _)), Some(dt)) => Some(b1.saturating_sub(b0) as f32 * 100.0 / dt as f32),
            _ => None,
        };
        self.prev_cpu = now;

        if !with_processes {
            self.prev_ticks.clear();
            return (cpu, None);
        }
        let mut procs = processes();
        if procs.is_empty() {
            return (cpu, None);
        }
        for p in &mut procs {
            if let (Some(&before), Some(dt)) = (self.prev_ticks.get(&p.pid), elapsed) {
                p.cpu_percent = p.cpu_ticks.saturating_sub(before) as f32 * 100.0 / dt as f32;
            }
        }
        self.prev_ticks = procs.iter().map(|p| (p.pid, p.cpu_ticks)).collect();
        (cpu, Some(procs))
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Taskbar-style clock, e.g. `3:07 PM`. UTC until FerroOS grows timezones.
pub fn clock_12h() -> String {
    let secs = now_secs();
    let (h, m) = ((secs / 3600) % 24, (secs / 60) % 60);
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12}:{m:02} {}", if h < 12 { "AM" } else { "PM" })
}

/// `YYYY-MM-DD HH:MM` (UTC) for a Unix timestamp.
pub fn format_utc(secs: u64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let days = (secs / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60)
}

/// Shell-style glob as used by modules.alias: `*`, `?`, `[abc]`, `[a-z]`,
/// `[!x]`/`[^x]`.
pub fn glob_match(pat: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    star = Some((pi, ti));
                    pi += 1;
                    continue;
                }
                '?' => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                '[' => {
                    if let Some((matched, next)) = class(&p, pi, t[ti]) {
                        if matched {
                            pi = next;
                            ti += 1;
                            continue;
                        }
                    }
                }
                c if c == t[ti] => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some((sp, st)) => {
                pi = sp + 1;
                ti = st + 1;
                star = Some((sp, st + 1));
            }
            None => return false,
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Matches `c` against the `[...]` class starting at `p[i]`; returns
/// (matched, index after the class), or `None` if unterminated.
fn class(p: &[char], i: usize, c: char) -> Option<(bool, usize)> {
    let mut j = i + 1;
    let negate = matches!(p.get(j), Some('!' | '^'));
    if negate {
        j += 1;
    }
    let mut matched = false;
    let mut first = true;
    while j < p.len() && (p[j] != ']' || first) {
        first = false;
        if p.get(j + 1) == Some(&'-') && p.get(j + 2).is_some_and(|&e| e != ']') {
            matched |= (p[j]..=p[j + 2]).contains(&c);
            j += 3;
        } else {
            matched |= p[j] == c;
            j += 1;
        }
    }
    (j < p.len()).then_some((matched != negate, j + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo() {
        let m = MemInfo::parse("MemTotal:   61440 kB\nMemFree: 30000 kB\nMemAvailable:  40960 kB\n").unwrap();
        assert_eq!(m.used_kb(), 20480);
        assert_eq!(MemInfo::parse("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn footprint_counts_the_kernel() {
        let iomem =
            "00000000-00000fff : Reserved\n00001000-0009fbff : System RAM\n00100000-07fdffff : System RAM\n  01000000-01ffffff : Kernel code\n";
        let phys = parse_iomem_ram_kb(iomem).unwrap();
        assert_eq!(phys, (0x9fbff - 0x1000 + 1 + 0x7fdffff - 0x100000 + 1) / 1024);
        assert_eq!(parse_iomem_ram_kb("00000000-00000000 : System RAM\n"), None, "non-root view");
        let m = MemInfo { total_kb: 93_160, available_kb: 57_052, physical_kb: Some(130_936), reserve_kb: 0 };
        assert_eq!(m.footprint_kb(), 73_884 - 130_936 / 64, "page bookkeeping isn't use");
        assert_eq!(m.kernel_reserved_kb(), Some(37_776));
    }

    #[test]
    fn parses_stat_with_awkward_comm() {
        let line = "42 (my (odd) prog) S 1 42 42 0 -1 4194560 100 0 0 0 7 3 0 0 20 0 1 0 5 1000 50";
        assert_eq!(parse_stat(line), Some(("my (odd) prog".into(), 10)));
    }

    #[test]
    fn parses_cpu_line() {
        assert_eq!(parse_cpu_line("cpu  10 0 5 80 5 0 0 0 0 0"), Some((15, 100)));
    }

    #[test]
    fn modalias_globs() {
        assert!(glob_match("pci:v00001234d00001111sv*sd*bc03sc*i*", "pci:v00001234d00001111sv00001AF4sd00001100bc03sc00i00"));
        assert!(!glob_match("pci:v00001234d00001111sv*sd*bc03sc*i*", "pci:v00008086d00001111sv0sd0bc03sc00i00"));
        assert!(glob_match("virtio:d00000001v*", "virtio:d00000001v00001AF4"));
        assert!(glob_match("serio:ty01pr*id*ex*", "serio:ty01pr00id00ex00"));
        assert!(glob_match("usb:v*p*d*dc*dsc*dp*ic03isc*ip*in*", "usb:v046DpC077d0110dc00dsc00dp00ic03isc01ip02in00"));
        assert!(glob_match("a[0-9]c", "a5c") && !glob_match("a[!0-9]c", "a5c") && glob_match("a[^x]c", "abc"));
        assert!(glob_match("*", "") && !glob_match("a", ""));
    }

    #[test]
    fn formats_dates() {
        assert_eq!(format_utc(0), "1970-01-01 00:00");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00");
        assert_eq!(format_utc(1_791_461_000), "2026-10-08 12:03");
    }

    #[test]
    fn zone_reserve_is_watermark_plus_protection() {
        let z = "Node 0, zone    DMA32\n  pages free     7000\n        min      207\n        low      258\n        high     309\n        spanned  16000\n        protection: (0, 0, 12, 0)\nNode 0, zone   Normal\n  pages free     0\n        high     0\n        protection: (0, 0, 0, 0)\n";
        assert_eq!(parse_zoneinfo_reserve_kb(z), (309 + 12) * 4);
        let with_pcp = format!("{z}  pagesets\n    cpu: 0\n              count:    1035\n              high:     1050\n");
        assert_eq!(parse_zoneinfo_reserve_kb(&with_pcp), (309 + 12 + 1035) * 4, "per-CPU free pages are free");
    }
}
