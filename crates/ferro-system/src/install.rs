//! FerroOS Setup, the privileged half: lists disks and installs onto one.
//! The desktop's Setup wizard drives it through `DISKS` and `INSTALL`, and
//! follows progress in [`STATUS`]. Only offered in live sessions.
//!
//! The disk gets a GPT with:
//!
//! 1. EFI system partition, 128 MiB, FAT32: `\EFI\BOOT\BOOTX64.EFI`
//!    (ferro-boot), `\EFI\ferro\bzImage`, `\EFI\ferro\initramfs.cpio`
//! 2. `ferro-programs`: the programs disk (the Tor service)
//! 3. `ferro-vault`: the rest; the encrypted vault, created at first boot

use crate::gpt::{self, Part};
use crate::programs::{self, Entry};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

/// `state=working|done|error`, `percent=`, `step=`.
pub const STATUS: &str = crate::INSTALL_STATUS;
const MIB: u64 = 1 << 20;
const ESP_SIZE: u64 = 128 * MIB;

#[derive(Clone, Debug)]
pub struct Disk {
    pub name: String,
    pub bytes: u64,
    pub model: String,
}

/// The disk holding the installation files (not a target).
fn medium() -> Option<String> {
    let (dev, _) = programs::find_disk()?;
    let name = dev.file_name()?.to_string_lossy().into_owned();
    let sys = fs::canonicalize(Path::new("/sys/class/block").join(&name)).ok()?;
    // A partition's sysfs directory sits inside its disk's.
    if sys.join("partition").exists() {
        Some(sys.parent()?.file_name()?.to_string_lossy().into_owned())
    } else {
        Some(name)
    }
}

/// Disks Setup may install to: real disks of at least 2 GiB, not the medium.
pub fn disks() -> Result<Vec<Disk>, String> {
    // USB sticks can take a few seconds to show up after boot.
    let mut skip = medium();
    for _ in 0..20 {
        if skip.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        skip = medium();
    }
    if skip.is_none() {
        return Err("The installation files weren't found. Is the FerroOS USB stick still plugged in?".into());
    }
    let mut out: Vec<Disk> = fs::read_dir("/sys/block")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("vd") || name.starts_with("sd") || name.starts_with("nvme")) || Some(&name) == skip.as_ref() {
                return None;
            }
            let sectors: u64 = fs::read_to_string(e.path().join("size")).ok()?.trim().parse().ok()?;
            let bytes = sectors * 512;
            if bytes < 2 << 30 {
                return None;
            }
            let model = fs::read_to_string(e.path().join("device/model")).map(|m| m.trim().to_owned()).unwrap_or_else(|_| "Disk".into());
            Some(Disk { name, bytes, model })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn status(state: &str, percent: u32, step: &str) {
    let tmp = format!("{STATUS}.tmp");
    if fs::write(&tmp, format!("state={state}\npercent={percent}\nstep={step}\n")).is_ok() {
        let _ = fs::rename(&tmp, STATUS);
    }
}

/// Installs onto `/dev/<name>` in the background, reporting to [`STATUS`].
pub fn start(name: &str) -> Result<(), String> {
    if !disks()?.iter().any(|d| d.name == name) {
        return Err("that disk can't be used".into());
    }
    if fs::read_to_string(STATUS).is_ok_and(|s| s.contains("state=working")) {
        return Err("Setup is already running".into());
    }
    status("working", 0, "Reading the installation files");
    let name = name.to_owned();
    std::thread::spawn(move || {
        let mut touched = false;
        match install(&name, &mut touched) {
            Ok(()) => {
                status("done", 100, "FerroOS is installed");
                eprintln!("ferro-system: installed FerroOS on /dev/{name}");
            }
            Err(e) => {
                let what = if touched {
                    "The disk was partly written: it won't start anything until Setup completes."
                } else {
                    "Nothing on the disk was changed."
                };
                status("error", 0, &format!("{e} {what}"));
                eprintln!("ferro-system: install on /dev/{name} failed: {e}");
            }
        }
    });
    Ok(())
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // SAFETY: getrandom fills our buffer.
    unsafe { libc::getrandom(b.as_mut_ptr().cast(), N, 0) };
    b
}

/// A window onto part of the disk, for the FAT formatter.
struct Region<'a> {
    disk: &'a File,
    start: u64,
    len: u64,
    pos: u64,
}

impl Read for Region<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.len.saturating_sub(self.pos) as usize);
        let n = self.disk.read_at(&mut buf[..n], self.start + self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Write for Region<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = buf.len().min(self.len.saturating_sub(self.pos) as usize);
        if n == 0 && !buf.is_empty() {
            return Err(io::Error::other("past the end of the partition"));
        }
        let n = self.disk.write_at(&buf[..n], self.start + self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Region<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::Current(d) => self.pos as i64 + d,
            SeekFrom::End(d) => self.len as i64 + d,
        };
        if p < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

fn install(name: &str, touched: &mut bool) -> io::Result<()> {
    let kernel = programs::read("bzImage")?;
    let initramfs = programs::read("initramfs")?;
    let loader = programs::read("bootx64.efi")?;
    // Optional programs, verified like at run time.
    let arti = match programs::expected_sha256("arti") {
        Some(_) => {
            let tmp = Path::new("/run/ferro/setup-arti");
            programs::extract("arti", tmp).ok().and_then(|()| {
                let data = fs::read(tmp).ok();
                let _ = fs::remove_file(tmp);
                data
            })
        }
        None => None,
    };
    status("working", 10, "Creating partitions");

    let dev = Path::new("/dev").join(name);
    let disk = OpenOptions::new().read(true).write(true).open(&dev)?;
    let lbs: u64 = fs::read_to_string(format!("/sys/block/{name}/queue/logical_block_size")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(512);
    let bytes = fs::read_to_string(format!("/sys/block/{name}/size")).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0) * 512;
    let lbas = bytes / lbs;

    // Programs disk image (header + programs), as `cargo xtask` builds it.
    let mut programs_img = Vec::new();
    if let Some(a) = &arti {
        programs_img = programs::build_header(&[Entry { name: "arti".into(), offset: 4096, size: a.len() as u64 }]);
        programs_img.extend_from_slice(a);
    }
    let programs_size = (programs_img.len() as u64).next_multiple_of(MIB).max(64 * MIB) + 16 * MIB;

    let esp_start = MIB;
    let programs_start = esp_start + ESP_SIZE;
    let vault_start = programs_start + programs_size;
    let vault_end = (bytes - MIB) / MIB * MIB; // leave room for the backup table
    if vault_end <= vault_start + 64 * MIB {
        return Err(io::Error::other("the disk is too small"));
    }
    let lba = |b: u64| b / lbs;
    let esp_guid: [u8; 16] = random();
    let parts = [
        Part {
            type_guid: gpt::guid(gpt::ESP_TYPE),
            guid: esp_guid,
            first_lba: lba(esp_start),
            last_lba: lba(programs_start) - 1,
            name: "EFI system",
        },
        Part {
            type_guid: gpt::guid(gpt::LINUX_DATA_TYPE),
            guid: random(),
            first_lba: lba(programs_start),
            last_lba: lba(vault_start) - 1,
            name: "ferro-programs",
        },
        Part {
            type_guid: gpt::guid(gpt::LINUX_DATA_TYPE),
            guid: random(),
            first_lba: lba(vault_start),
            last_lba: lba(vault_end) - 1,
            name: "ferro-vault",
        },
    ];
    let (start, end, end_lba) = gpt::build(lbas, lbs, random(), &parts);
    // Whatever was there before stops being recognisable: wipe old tables
    // and the start of each new partition.
    *touched = true;
    let zeros = vec![0u8; MIB as usize];
    for at in [0, esp_start, programs_start, vault_start, bytes - MIB] {
        disk.write_all_at(&zeros, at)?;
    }
    disk.write_all_at(&start, 0)?;
    disk.write_all_at(&end, end_lba * lbs)?;

    status("working", 25, "Formatting the boot partition");
    let mut esp = Region { disk: &disk, start: esp_start, len: ESP_SIZE, pos: 0 };
    fatfs::format_volume(&mut esp, fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat32).volume_label(*b"FERROOS    "))?;
    esp.pos = 0;
    {
        let fs = fatfs::FileSystem::new(&mut esp, fatfs::FsOptions::new())?;
        {
            let efi = fs.root_dir().create_dir("EFI")?;
            let boot = efi.create_dir("BOOT")?;
            let ferro = efi.create_dir("ferro")?;
            let files: [(&fatfs::Dir<_>, &str, &[u8], u32); 3] =
                [(&boot, "BOOTX64.EFI", &loader, 35), (&ferro, "bzImage", &kernel, 45), (&ferro, "initramfs.cpio", &initramfs, 60)];
            for (dir, file, data, pct) in files {
                status("working", pct, "Copying FerroOS");
                let mut f = dir.create_file(file)?;
                f.truncate()?;
                f.write_all(data)?;
                f.flush()?;
            }
        }
        fs.unmount()?;
    }

    status("working", 70, "Copying programs");
    if !programs_img.is_empty() {
        for (i, chunk) in programs_img.chunks(4 * MIB as usize).enumerate() {
            disk.write_all_at(chunk, programs_start + i as u64 * 4 * MIB)?;
        }
    }

    status("working", 95, "Finishing");
    disk.sync_all()?;
    // Into the firmware's boot menu, first. Not fatal: firmware also starts
    // \EFI\BOOT\BOOTX64.EFI from a disk by itself.
    if let Err(e) = add_boot_entry(lba(esp_start), lba(ESP_SIZE), esp_guid) {
        eprintln!("ferro-system: couldn't add FerroOS to the firmware's boot menu: {e}");
    }
    // Tell the kernel about the new partitions (BLKRRPART).
    // SAFETY: ioctl on our open block device, no argument.
    unsafe {
        use std::os::fd::AsRawFd;
        libc::ioctl(disk.as_raw_fd(), 0x125F as _);
    }
    Ok(())
}

/// Adds "FerroOS" to the firmware's boot menu and makes it the default.
fn add_boot_entry(start_lba: u64, size_lbas: u64, esp_guid: [u8; 16]) -> io::Result<()> {
    use crate::efiboot::{load_option, ATTRIBUTES, GLOBAL_GUID};
    if !Path::new("/sys/firmware/efi").exists() {
        return Err(io::Error::other("not started with UEFI"));
    }
    let vars = Path::new("/sys/firmware/efi/efivars");
    let order_path = vars.join(format!("BootOrder-{GLOBAL_GUID}"));
    if fs::read_dir(vars).map_or(true, |mut d| d.next().is_none()) {
        // SAFETY: mount(2) with valid C strings.
        unsafe { libc::mount(c"efivarfs".as_ptr(), c"/sys/firmware/efi/efivars".as_ptr(), c"efivarfs".as_ptr(), 0, std::ptr::null()) };
    }
    let order: Vec<u16> = fs::read(&order_path)
        .map(|b| b.get(4..).unwrap_or(&[]).as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect())
        .unwrap_or_default();
    let number = (0x0F00u16..0x0FFF)
        .find(|n| !vars.join(format!("Boot{n:04X}-{GLOBAL_GUID}")).exists())
        .ok_or_else(|| io::Error::other("no free boot entry"))?;
    let mut entry = ATTRIBUTES.to_le_bytes().to_vec();
    entry.extend(load_option("FerroOS", 1, start_lba, size_lbas, esp_guid, "\\EFI\\BOOT\\BOOTX64.EFI"));
    // efivarfs takes each variable in a single write.
    fs::write(vars.join(format!("Boot{number:04X}-{GLOBAL_GUID}")), &entry)?;
    let mut new_order = ATTRIBUTES.to_le_bytes().to_vec();
    for n in std::iter::once(number).chain(order) {
        new_order.extend_from_slice(&n.to_le_bytes());
    }
    fs::write(&order_path, &new_order)?;
    eprintln!("ferro-system: added Boot{number:04X} \"FerroOS\" to the firmware's boot menu");
    Ok(())
}
