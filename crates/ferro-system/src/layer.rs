//! The Debian layer: a minimal Debian next to FerroOS, so programs built
//! for ordinary Linux (glibc) run. FerroOS itself doesn't use it.
//!
//! Three pieces, stacked:
//!
//! 1. **The base**: a squashfs image of a minimal Debian, on a partition of
//!    its own (on the installation medium, and `ferro-debian` on an installed
//!    disk, where Setup copies it, so installing needs no network). The disk
//!    is untrusted: the image carries a hash tree (dm-verity) whose root is
//!    compiled into FerroOS, so every block is checked as it's read, and a
//!    changed one reads as an error. The base is never written to.
//! 2. **Your changes** (what `DEBIAN apt install ...` adds), kept on top of
//!    the base with overlayfs. With the vault unlocked on an installed disk
//!    they live on the `ferro-debian-data` partition: LUKS2, opened with the
//!    vault's password, formatted the first time. In a live or "save nothing"
//!    session they live in RAM and are gone at shutdown.
//! 3. **`/usr`**: the combined Debian's `/usr`, where FerroOS's own root
//!    expects programs' loader and libraries. Sandboxed programs see that;
//!    `DEBIAN <command>` runs inside the whole combined Debian, as root.
//!
//! Root inside Debian is root: with a vault, `DEBIAN` wants its password
//! each time, so a program running as you can't quietly become root.

/// Where Debian's `/usr` appears.
pub const MOUNT: &str = "/usr";
/// `state=working|done|error`, `code=`: the last `DEBIAN` command.
pub const STATUS: &str = "/run/ferro/debian-status";
/// Its output so far. In Debian's own /run: FerroOS's is 4 MB, and apt
/// can say more than that.
pub const LOG: &str = "/run/ferro/debian/root/run/ferro-debian.log";
/// The reply when `DEBIAN` needs the vault password and didn't get the
/// right one.
pub const WRONG_PASSWORD: &str = "password required";
/// The partition Setup makes for what's installed into Debian.
pub const DATA_PARTITION: &str = "ferro-debian-data";
const MAGIC: &[u8; 4] = b"hsqs";

/// Expected SHA-256 of the whole base image (file system and hash tree),
/// fixed when FerroOS was built: what Setup checks as it copies. `None`
/// when this build has no Debian layer.
pub fn expected_sha256() -> Option<&'static str> {
    option_env!("FERRO_DEBIAN_SHA256")
}

/// What checks the base image block by block, fixed when FerroOS was built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verity {
    /// The hash tree's root: change any block and it no longer matches.
    pub root: &'static str,
    pub salt: &'static str,
    /// 4 KiB blocks of file system; the hash tree follows them.
    pub data_blocks: u64,
    /// 4 KiB blocks in all.
    pub total_blocks: u64,
}

/// `root:salt:data blocks:total blocks`, as the build writes it.
pub fn parse_verity(s: &'static str) -> Option<Verity> {
    let mut it = s.trim().split(':');
    let (root, salt) = (it.next()?, it.next()?);
    let (data_blocks, total_blocks) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    let hex = |h: &str| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit());
    (hex(root) && root.len() == 64 && hex(salt) && data_blocks > 0 && total_blocks > data_blocks).then_some(Verity {
        root,
        salt,
        data_blocks,
        total_blocks,
    })
}

/// `None` when this build has no Debian layer.
pub fn verity() -> Option<Verity> {
    parse_verity(option_env!("FERRO_DEBIAN_VERITY")?)
}

/// The file system's length from its superblock (padded to 4 KiB, as
/// mksquashfs writes it), or `None` if this isn't squashfs.
pub fn image_len(superblock: &[u8]) -> Option<u64> {
    if superblock.get(..4)? != MAGIC {
        return None;
    }
    let used = u64::from_le_bytes(superblock.get(40..48)?.try_into().ok()?);
    (used >= 96).then(|| used.next_multiple_of(4096))
}

/// A device-mapper request: the fixed header (`struct dm_ioctl`) for device
/// `name`, followed by one target when `table` gives its length in sectors,
/// its type and its parameters.
pub fn dm_request(name: &str, flags: u32, table: Option<(u64, &str, &str)>) -> Vec<u8> {
    const HEADER: usize = 312;
    let mut b = vec![0u8; HEADER];
    for (i, v) in [4u32, 0, 0].iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes()); // interface version
    }
    b[16..20].copy_from_slice(&(HEADER as u32).to_le_bytes()); // data_start
    b[28..32].copy_from_slice(&flags.to_le_bytes());
    b[48..48 + name.len().min(127)].copy_from_slice(&name.as_bytes()[..name.len().min(127)]);
    if let Some((sectors, kind, params)) = table {
        b[20..24].copy_from_slice(&1u32.to_le_bytes()); // target_count
        let mut spec = vec![0u8; 40];
        spec[8..16].copy_from_slice(&sectors.to_le_bytes());
        spec[24..24 + kind.len().min(15)].copy_from_slice(&kind.as_bytes()[..kind.len().min(15)]);
        spec.extend_from_slice(params.as_bytes());
        spec.push(0);
        spec.resize(spec.len().next_multiple_of(8), 0);
        let next = spec.len() as u32;
        spec[20..24].copy_from_slice(&next.to_le_bytes());
        b.extend_from_slice(&spec);
    }
    let size = b.len() as u32;
    b[12..16].copy_from_slice(&size.to_le_bytes()); // data_size
    b
}

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(not(target_os = "linux"))]
pub fn nuke() {}
#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::ffi::CString;
    use std::fs::{self, File};
    use std::io::{self, Read, Write};
    use std::os::unix::fs::FileExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::Mutex;
    use std::time::Duration;

    /// The base image, read-only.
    const BASE: &str = "/run/ferro/debian/base";
    /// Where changes are kept: the LUKS2 partition's file system, or RAM.
    const DATA: &str = "/run/ferro/debian/data";
    /// The combined Debian.
    const ROOT: &str = "/run/ferro/debian/root";
    const MAPPER: &str = "ferro-debian";
    const RESET_FLAG: &str = "reset";

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Stage {
        Absent,
        /// The base alone: programs run, nothing can be installed.
        ReadOnly,
        Writable,
    }

    static STAGE: Mutex<Stage> = Mutex::new(Stage::Absent);
    /// The vault password of this session, salted and hashed, for `DEBIAN`.
    static PASSWORD: Mutex<Option<([u8; 16], [u8; 32])>> = Mutex::new(None);

    fn password_hash(salt: &[u8; 16], password: &str) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(salt);
        hash.update(password.as_bytes());
        hash.finalize().into()
    }

    /// Notes the password the vault was just created or unlocked with.
    pub fn remember_password(password: &str) {
        let mut salt = [0u8; 16];
        // SAFETY: getrandom fills our buffer.
        unsafe { libc::getrandom(salt.as_mut_ptr().cast(), salt.len(), 0) };
        *PASSWORD.lock().unwrap() = Some((salt, password_hash(&salt, password)));
    }

    /// Whether `DEBIAN` needs the vault password in this session.
    pub fn password_needed() -> bool {
        PASSWORD.lock().unwrap().is_some()
    }

    fn password_ok(given: Option<&str>) -> bool {
        match (&*PASSWORD.lock().unwrap(), given) {
            (None, _) => true,
            (Some((salt, want)), Some(p)) => {
                // Every byte is compared, whatever the first difference.
                password_hash(salt, p).iter().zip(want).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
            }
            (Some(_), None) => false,
        }
    }

    /// Where this session's changes to Debian go.
    pub enum Storage {
        /// RAM: gone at shutdown.
        Ram,
        /// The disk's LUKS2 partition, opened with this password.
        Disk(String),
    }

    /// Disks and partitions that start with a file system the size of this
    /// build's base image, with the whole image's length.
    fn candidates() -> Vec<(PathBuf, u64)> {
        let Some(v) = verity() else { return Vec::new() };
        let mut out = Vec::new();
        for e in fs::read_dir("/sys/class/block").into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("vd") || name.starts_with("sd") || name.starts_with("nvme")) {
                continue;
            }
            let dev = PathBuf::from("/dev").join(&name);
            let mut sb = [0u8; 96];
            if File::open(&dev).and_then(|mut f| f.read_exact(&mut sb)).is_err() {
                continue;
            }
            if image_len(&sb) == Some(v.data_blocks * 4096) {
                out.push((dev, v.total_blocks * 4096));
            }
        }
        out.sort();
        out
    }

    /// Reads the image on `dev` through `sink`, and fails unless it's the
    /// one this FerroOS was built with.
    fn verified(dev: &Path, len: u64, mut sink: impl FnMut(u64, &[u8]) -> io::Result<()>) -> io::Result<()> {
        let want = expected_sha256().ok_or_else(|| io::Error::other("this FerroOS build has no Debian layer"))?;
        let mut src = File::open(dev)?;
        let mut hash = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut at = 0;
        while at < len {
            let n = buf.len().min((len - at) as usize);
            src.read_exact(&mut buf[..n])?;
            hash.update(&buf[..n]);
            sink(at, &buf[..n])?;
            at += n as u64;
        }
        // Don't keep a copy of the whole image cached.
        // SAFETY: fadvise on our own open descriptor.
        unsafe {
            use std::os::fd::AsRawFd;
            libc::posix_fadvise(src.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
        }
        let got: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != want {
            return Err(io::Error::other(format!("the Debian layer on {} has been modified; refusing to use it", dev.display())));
        }
        Ok(())
    }

    /// The base image's device and length. Not yet checked: mounting checks
    /// it block by block, and Setup's [`copy`] checks it whole.
    pub fn find() -> Option<(PathBuf, u64)> {
        candidates().into_iter().next()
    }

    /// Sets up a read-only device that checks every block of the image on
    /// `dev` against the hash tree whose root this build was given.
    fn verified_device(dev: &Path, v: &Verity) -> io::Result<PathBuf> {
        const NAME: &str = "ferro-debian-base";
        const READ_ONLY: u32 = 1;
        // _IOWR(0xfd, nr, struct dm_ioctl)
        let request = |nr: u64| (3u64 << 30) | (312 << 16) | (0xfd << 8) | nr;
        let (create, suspend, load) = (request(3), request(6), request(9));
        module("dm_verity")?;
        let control = fs::OpenOptions::new().read(true).write(true).open("/dev/mapper/control")?;
        let call = |nr: u64, mut buf: Vec<u8>| -> io::Result<Vec<u8>> {
            use std::os::fd::AsRawFd;
            // SAFETY: the buffer is a complete dm_ioctl request of the size
            // its header states.
            if unsafe { libc::ioctl(control.as_raw_fd(), nr as _, buf.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(buf)
        };
        let dev = dev.to_string_lossy();
        // version, data device, hash device, block sizes, data blocks, where
        // the tree starts (after its one-block header), algorithm, root, salt
        let params = format!("1 {dev} {dev} 4096 4096 {} {} sha256 {} {}", v.data_blocks, v.data_blocks + 1, v.root, v.salt);
        let made = call(create, dm_request(NAME, 0, None))?;
        call(load, dm_request(NAME, READ_ONLY, Some((v.data_blocks * 8, "verity", &params))))?;
        call(suspend, dm_request(NAME, 0, None))?; // without the suspend flag: resume
        let number = u64::from_le_bytes(made[40..48].try_into().unwrap());
        let minor = (number & 0xff) | ((number >> 12) & 0xfff00);
        Ok(PathBuf::from(format!("/dev/dm-{minor}")))
    }

    fn mount(source: &str, target: &str, fstype: &str, flags: libc::c_ulong, data: &str) -> io::Result<()> {
        fs::create_dir_all(target)?;
        let (s, t, f, d) = (CString::new(source)?, CString::new(target)?, CString::new(fstype)?, CString::new(data)?);
        // SAFETY: mount(2) with valid C strings.
        if unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), flags, d.as_ptr().cast()) } != 0 {
            let e = io::Error::last_os_error();
            return Err(io::Error::new(e.kind(), format!("mounting {target}: {e}")));
        }
        Ok(())
    }

    fn unmount(target: &str) {
        if let Ok(t) = CString::new(target) {
            // SAFETY: umount2(2) with a valid C string.
            unsafe { libc::umount2(t.as_ptr(), libc::MNT_DETACH) };
        }
    }

    fn module(name: &str) -> io::Result<()> {
        ferro_net::route::load_module(name).map_err(|e| io::Error::other(format!("{name}: {e}")))
    }

    /// What a Debian program needs around it: kernel interfaces, devices,
    /// scratch space.
    fn mount_system_dirs(root: &str) -> io::Result<()> {
        let none = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
        mount("proc", &format!("{root}/proc"), "proc", none, "")?;
        mount("sys", &format!("{root}/sys"), "sysfs", none, "")?;
        mount("dev", &format!("{root}/dev"), "devtmpfs", libc::MS_NOSUID, "")?;
        mount("devpts", &format!("{root}/dev/pts"), "devpts", libc::MS_NOSUID | libc::MS_NOEXEC, "")?;
        mount("tmpfs", &format!("{root}/run"), "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, "mode=755")?;
        mount("tmpfs", &format!("{root}/tmp"), "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, "mode=1777")
    }

    /// Finds, verifies and mounts the base. Disks (USB sticks most of all)
    /// can take a few seconds to show up after boot, so this keeps looking
    /// for a while; meant for a background thread.
    pub fn mount_when_found() {
        let Some(v) = verity() else { return };
        for _ in 0..40 {
            let found = candidates();
            for (dev, len) in &found {
                let mounted = module("squashfs")
                    .and_then(|()| verified_device(dev, &v))
                    .and_then(|checked| mount(&checked.to_string_lossy(), BASE, "squashfs", libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV, ""))
                    .and_then(|()| mount_system_dirs(BASE))
                    .and_then(|()| mount(&format!("{BASE}/usr"), MOUNT, "", libc::MS_BIND, ""));
                match mounted {
                    Ok(()) => {
                        *STAGE.lock().unwrap() = Stage::ReadOnly;
                        eprintln!("ferro-system: Debian layer ready on {MOUNT} ({} MB on {}, every block checked)", len >> 20, dev.display());
                        // Their firmware is in the layer.
                        crate::wifi::load_drivers();
                        return;
                    }
                    Err(e) => eprintln!("ferro-system: Debian layer on {}: {e}", dev.display()),
                }
            }
            if !found.is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// A GPT partition by its name, from the kernel's device events.
    fn partition_by_name(label: &str) -> Option<PathBuf> {
        fs::read_dir("/sys/class/block").ok()?.flatten().find_map(|e| {
            let uevent = fs::read_to_string(e.path().join("uevent")).ok()?;
            let name = uevent.lines().find_map(|l| l.strip_prefix("PARTNAME="))?;
            let dev = uevent.lines().find_map(|l| l.strip_prefix("DEVNAME="))?;
            (name == label).then(|| PathBuf::from("/dev").join(dev))
        })
    }

    /// Runs one of the base's own tools (cryptsetup, mke2fs) inside the
    /// verified base, feeding it `input`.
    fn base_tool(program: &str, args: &[&str], input: &[u8]) -> io::Result<()> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin")
            // There's no udev to wait for: device-mapper makes its own nodes.
            .env("DM_DISABLE_UDEV", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // SAFETY: chroot and chdir are async-signal-safe; the paths are
        // static C strings.
        unsafe {
            cmd.pre_exec(|| {
                let base = c"/run/ferro/debian/base";
                if libc::chroot(base.as_ptr()) != 0 || libc::chdir(c"/".as_ptr()) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input);
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr);
            return Err(io::Error::other(format!("{program}: {}", why.lines().last().unwrap_or("failed").trim())));
        }
        Ok(())
    }

    /// Opens the disk's LUKS2 partition with `password` (formatting it the
    /// first time) and mounts its file system on [`DATA`].
    fn open_disk(password: &str) -> io::Result<()> {
        let dev = partition_by_name(DATA_PARTITION).ok_or_else(|| io::Error::other("this disk has no ferro-debian-data partition"))?;
        let dev = dev.to_string_lossy().into_owned();
        let mapped = format!("/dev/mapper/{MAPPER}");
        let mut magic = [0u8; 6];
        let fresh = !(File::open(&dev).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"LUKS\xba\xbe");
        module("dm_crypt")?;
        module("ext4")?;
        // The password is the whole of stdin, so it may hold anything.
        if fresh {
            let format = ["luksFormat", "--type", "luks2", "--batch-mode", "--disable-locks", "--pbkdf", "argon2id", "--key-file", "-", &dev];
            base_tool("/usr/sbin/cryptsetup", &format, password.as_bytes())?;
        }
        base_tool("/usr/sbin/cryptsetup", &["open", "--type", "luks2", "--disable-locks", "--key-file", "-", &dev, MAPPER], password.as_bytes())?;
        if fresh {
            base_tool("/usr/sbin/mke2fs", &["-q", "-t", "ext4", "-L", MAPPER, &mapped], b"")?;
            eprintln!("ferro-system: formatted {dev} (LUKS2) for Debian");
        }
        mount(&mapped, DATA, "ext4", libc::MS_NOSUID | libc::MS_NODEV, "")
    }

    /// Makes Debian writable for this session, keeping changes in `storage`.
    /// Falls back to RAM if the disk can't be used. Does nothing if it has
    /// already been done; waits a while for the base at boot.
    pub fn make_writable(storage: Storage) {
        if verity().is_none() {
            return;
        }
        for _ in 0..80 {
            if *STAGE.lock().unwrap() != Stage::Absent {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let mut stage = STAGE.lock().unwrap();
        if *stage != Stage::ReadOnly {
            return;
        }
        let mut kept = "in RAM, until shutdown";
        if let Storage::Disk(mut password) = storage {
            match open_disk(&password) {
                Ok(()) => kept = "on the disk, encrypted",
                Err(e) => eprintln!("ferro-system: Debian: can't keep changes on the disk ({e}); using RAM"),
            }
            // SAFETY: zeroes are valid UTF-8; the password shouldn't outlive its use.
            unsafe { password.as_bytes_mut().fill(0) };
        }
        let combined = (|| {
            if kept.starts_with("in RAM") {
                mount("tmpfs", DATA, "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, "mode=755")?;
            }
            // DEBIAN /RESET asked for a fresh start.
            if Path::new(DATA).join(RESET_FLAG).exists() {
                for dir in ["upper", "work"] {
                    let _ = fs::remove_dir_all(Path::new(DATA).join(dir));
                }
                let _ = fs::remove_file(Path::new(DATA).join(RESET_FLAG));
                eprintln!("ferro-system: Debian reset to how it shipped");
            }
            module("overlay")?;
            fs::create_dir_all(format!("{DATA}/upper"))?;
            fs::create_dir_all(format!("{DATA}/work"))?;
            mount("overlay", ROOT, "overlay", libc::MS_NOSUID, &format!("lowerdir={BASE},upperdir={DATA}/upper,workdir={DATA}/work"))?;
            mount_system_dirs(ROOT)?;
            // Programs keep whichever /usr they started with.
            unmount(MOUNT);
            mount(&format!("{ROOT}/usr"), MOUNT, "", libc::MS_BIND, "")
        })();
        match combined {
            Ok(()) => {
                *stage = Stage::Writable;
                eprintln!("ferro-system: Debian is writable; changes are kept {kept}");
            }
            Err(e) => eprintln!("ferro-system: Debian stays read-only: {e}"),
        }
    }

    /// A command that will run `program` as root inside Debian, once Debian
    /// is writable (ferro-system's own helpers: Wi-Fi).
    pub fn command(program: &str) -> Option<Command> {
        if *STAGE.lock().unwrap() != Stage::Writable {
            return None;
        }
        let mut cmd = Command::new(program);
        cmd.env_clear().env("PATH", "/usr/sbin:/usr/bin");
        // SAFETY: chroot and chdir are async-signal-safe; the paths are
        // static C strings.
        unsafe {
            cmd.pre_exec(|| {
                let root = c"/run/ferro/debian/root";
                if libc::chroot(root.as_ptr()) != 0 || libc::chdir(c"/".as_ptr()) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Some(cmd)
    }

    /// Where `rel` inside Debian is from outside.
    pub fn path(rel: &str) -> PathBuf {
        Path::new(ROOT).join(rel)
    }

    /// Forgets everything installed into Debian, at the next start.
    pub fn reset(password: Option<&str>) -> Result<(), String> {
        if !password_ok(password) {
            return Err(WRONG_PASSWORD.into());
        }
        if *STAGE.lock().unwrap() != Stage::Writable {
            return Err("Debian isn't ready yet".into());
        }
        fs::write(Path::new(DATA).join(RESET_FLAG), "").map_err(|e| e.to_string())
    }

    /// Sends apt through Tor while Tor-only mode is on, so the mirror sees
    /// an exit relay and the network sees nothing; directly otherwise.
    fn route_apt() {
        let conf = format!("{ROOT}/etc/apt/apt.conf.d/90ferro-tor");
        if Path::new(ferro_net::TOR_FILE).exists() {
            let proxy = format!("socks5h://{}", ferro_net::TOR_SOCKS);
            let _ = fs::write(&conf, format!("Acquire::http::Proxy \"{proxy}\";\nAcquire::https::Proxy \"{proxy}\";\n"));
        } else {
            let _ = fs::remove_file(&conf);
        }
    }

    /// Runs `line` as root inside Debian (`apt install ...`), in the
    /// background: output goes to [`LOG`], progress to [`STATUS`].
    pub fn run(line: &str, password: Option<&str>) -> Result<(), String> {
        if !password_ok(password) {
            return Err(WRONG_PASSWORD.into());
        }
        if *STAGE.lock().unwrap() != Stage::Writable {
            return Err("Debian isn't ready yet (log on first, or this build has no Debian layer)".into());
        }
        route_apt();
        if fs::read_to_string(STATUS).is_ok_and(|s| s.contains("state=working")) {
            return Err("another DEBIAN command is still running".into());
        }
        let log = File::create(LOG).map_err(|e| e.to_string())?;
        // Names resolve through FerroOS's encrypted resolver.
        let _ = fs::copy("/etc/resolv.conf", format!("{ROOT}/etc/resolv.conf"));
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", line])
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin")
            .env("HOME", "/root")
            .env("TERM", "dumb")
            .env("DEBIAN_FRONTEND", "noninteractive")
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log);
        // SAFETY: chroot and chdir are async-signal-safe; the paths are
        // static C strings.
        unsafe {
            cmd.pre_exec(|| {
                let root = c"/run/ferro/debian/root";
                if libc::chroot(root.as_ptr()) != 0 || libc::chdir(c"/".as_ptr()) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let status = |state: &str, code: i32| {
            let tmp = format!("{STATUS}.tmp");
            if fs::write(&tmp, format!("state={state}\ncode={code}\n")).is_ok() {
                let _ = fs::rename(&tmp, STATUS);
            }
        };
        status("working", 0);
        std::thread::spawn(move || match child.wait() {
            Ok(s) if s.success() => status("done", 0),
            Ok(s) => status("error", s.code().unwrap_or(-1)),
            Err(_) => status("error", -1),
        });
        Ok(())
    }

    /// Before power-off: what's on the disk is written out and detached.
    pub fn close() {
        if *STAGE.lock().unwrap() != Stage::Writable {
            return;
        }
        // SAFETY: plain syscall.
        unsafe { libc::sync() };
        for dir in ["tmp", "run", "dev/pts", "dev", "sys", "proc"] {
            unmount(&format!("{ROOT}/{dir}"));
        }
        for target in [MOUNT, ROOT, DATA] {
            unmount(target);
        }
    }

    /// Setup: copies the verified base from `dev` onto `disk` at `start`.
    pub fn copy(dev: &Path, len: u64, disk: &File, start: u64, mut progress: impl FnMut(u64)) -> io::Result<()> {
        verified(dev, len, |at, chunk| {
            progress(at);
            disk.write_all_at(chunk, start + at)
        })
    }

    /// Duress / panic: wipes the Debian encrypted data partition (LUKS2 headers and keyslots).
    pub fn nuke() {
        if let Some(dev) = partition_by_name(DATA_PARTITION) {
            if let Ok(mut f) = fs::OpenOptions::new().write(true).open(&dev) {
                let zeros = vec![0u8; 16 * 1024 * 1024]; // 16 MiB
                let _ = std::io::Write::write_all(&mut f, &zeros);
                let _ = f.sync_all();
                eprintln!("ferro-system: nuke: wiped {}", dev.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_image_length() {
        let mut sb = vec![0u8; 96];
        sb[..4].copy_from_slice(b"hsqs");
        sb[40..48].copy_from_slice(&70_000_001u64.to_le_bytes());
        assert_eq!(image_len(&sb), Some(70_000_640));
        sb[0] = b'x';
        assert_eq!(image_len(&sb), None);
        assert_eq!(image_len(b"hsqs"), None);
    }

    #[test]
    fn reads_the_verity_line() {
        let root = "ab".repeat(32);
        let line: &'static str = Box::leak(format!("{root}:00ff:6703:6757\n").into_boxed_str());
        let v = parse_verity(line).unwrap();
        assert_eq!((v.root, v.salt, v.data_blocks, v.total_blocks), (root.as_str(), "00ff", 6703, 6757));
        assert_eq!(parse_verity("abcd:00:1:2"), None);
        assert_eq!(parse_verity("zz"), None);
    }

    #[test]
    fn builds_device_mapper_requests() {
        let plain = dm_request("ferro-debian-base", 0, None);
        assert_eq!(plain.len(), 312);
        assert_eq!(&plain[..4], &4u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(plain[12..16].try_into().unwrap()), 312);
        assert_eq!(&plain[48..65], b"ferro-debian-base");
        let table = dm_request("x", 1, Some((80, "verity", "1 /dev/vdb")));
        assert_eq!(table.len() % 8, 0);
        assert_eq!(u32::from_le_bytes(table[12..16].try_into().unwrap()) as usize, table.len());
        assert_eq!(u32::from_le_bytes(table[20..24].try_into().unwrap()), 1);
        assert_eq!(u64::from_le_bytes(table[320..328].try_into().unwrap()), 80);
        assert_eq!(&table[336..342], b"verity");
        assert_eq!(&table[352..362], b"1 /dev/vdb");
    }
}
