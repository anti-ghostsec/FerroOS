//! ferro-boot: FerroOS's boot loader on an installed disk.
//!
//! UEFI firmware starts it from the EFI system partition as the default
//! program (`\EFI\BOOT\BOOTX64.EFI`). It starts the Linux kernel stored next
//! to it through the kernel's EFI stub, with FerroOS's command line; the stub
//! then loads the initramfs named by `initrd=` from the same partition.
//! No menu, no settings, nothing written anywhere.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use uefi::boot::{self, LoadImageSource, OpenProtocolAttributes, OpenProtocolParams};
use uefi::prelude::*;
use uefi::proto::device_path::build::{self, DevicePathBuilder};
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::BootPolicy;
use uefi::CStr16;

const KERNEL: &CStr16 = cstr16!("\\EFI\\ferro\\bzImage");
/// The vault is found by its partition name, never by guessing a disk. The
/// `*hash_entries` fix the kernel's lookup tables at desktop sizes; left to
/// the kernel they grow with RAM (about 30 MB on a 16 GB PC).
const CMDLINE: &str =
    "initrd=\\EFI\\ferro\\initramfs.cpio ferro.vault=PARTLABEL=ferro-vault quiet loglevel=3 console=ttyS0 dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512";

#[entry]
fn main() -> Status {
    match boot_kernel() {
        Ok(()) => Status::SUCCESS,
        Err(e) => {
            uefi::println!("FerroOS couldn't start: {:?}", e.status());
            boot::stall(10_000_000);
            e.status()
        }
    }
}

fn boot_kernel() -> uefi::Result {
    let me = boot::image_handle();
    // The partition we were loaded from, as a full device path...
    let device = boot::open_protocol_exclusive::<LoadedImage>(me)?.device().ok_or(Status::NOT_FOUND)?;
    // SAFETY: a shared, read-only look at the device path; nothing else
    // can uninstall it while we hold it.
    let dev_path = unsafe {
        boot::open_protocol::<DevicePath>(OpenProtocolParams { handle: device, agent: me, controller: None }, OpenProtocolAttributes::GetProtocol)?
    };
    // ...plus the kernel's file name.
    let mut buf = Vec::new();
    let mut builder = DevicePathBuilder::with_vec(&mut buf);
    for node in dev_path.node_iter() {
        builder = builder.push(&node).map_err(|_| Status::BAD_BUFFER_SIZE)?;
    }
    let path = builder.push(&build::media::FilePath { path_name: KERNEL }).and_then(|b| b.finalize()).map_err(|_| Status::BAD_BUFFER_SIZE)?;

    let kernel = boot::load_image(me, LoadImageSource::FromDevicePath { device_path: path, boot_policy: BootPolicy::ExactMatch })?;
    drop(dev_path);

    // The command line, as the UCS-2 string UEFI programs receive.
    let options: Vec<u16> = CMDLINE.encode_utf16().chain(Some(0)).collect();
    {
        let mut image = boot::open_protocol_exclusive::<LoadedImage>(kernel)?;
        // SAFETY: `options` outlives the kernel's use of it (start_image
        // only returns if the kernel fails to start).
        unsafe { image.set_load_options(options.as_ptr().cast(), (options.len() * 2) as u32) };
    }
    boot::start_image(kernel)
}
