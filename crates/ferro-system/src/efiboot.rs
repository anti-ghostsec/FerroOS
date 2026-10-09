//! UEFI boot entries (spec 3.1.3, "Load Options"): Setup adds "FerroOS" to
//! the firmware's boot menu and puts it first, like other installers do.

/// `Boot####` and `BootOrder` live under this vendor GUID.
pub const GLOBAL_GUID: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
/// Non-volatile, visible to boot services and to the running OS.
pub const ATTRIBUTES: u32 = 0x7;

/// An `EFI_LOAD_OPTION` that starts `path` from GPT partition `number`.
pub fn load_option(description: &str, number: u32, start_lba: u64, size_lbas: u64, part_guid: [u8; 16], path: &str) -> Vec<u8> {
    let mut dp = Vec::new();
    // Hard drive media node (type 4, subtype 1): partition, GPT signature.
    dp.extend_from_slice(&[4, 1]);
    dp.extend_from_slice(&42u16.to_le_bytes());
    dp.extend_from_slice(&number.to_le_bytes());
    dp.extend_from_slice(&start_lba.to_le_bytes());
    dp.extend_from_slice(&size_lbas.to_le_bytes());
    dp.extend_from_slice(&part_guid);
    // Partition table and signature kinds: GPT, GUID.
    dp.extend_from_slice(&[2, 2]);
    // File path node (type 4, subtype 4), UCS-2 with a terminating NUL.
    let name: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    dp.extend_from_slice(&[4, 4]);
    dp.extend_from_slice(&(4 + name.len() as u16 * 2).to_le_bytes());
    name.iter().for_each(|u| dp.extend_from_slice(&u.to_le_bytes()));
    // End of the device path.
    dp.extend_from_slice(&[0x7F, 0xFF, 4, 0]);

    let mut opt = Vec::new();
    opt.extend_from_slice(&1u32.to_le_bytes()); // LOAD_OPTION_ACTIVE
    opt.extend_from_slice(&(dp.len() as u16).to_le_bytes());
    description.encode_utf16().chain(Some(0)).for_each(|u| opt.extend_from_slice(&u.to_le_bytes()));
    opt.extend_from_slice(&dp);
    opt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_option_layout() {
        let o = load_option("FerroOS", 1, 2048, 262144, [0xAB; 16], "\\EFI\\BOOT\\BOOTX64.EFI");
        let dp_len = u16::from_le_bytes([o[4], o[5]]) as usize;
        let desc_end = 6 + ("FerroOS".len() + 1) * 2;
        assert_eq!(&o[6..8], &[b'F', 0]);
        assert_eq!(o.len(), desc_end + dp_len);
        assert_eq!(&o[desc_end..desc_end + 4], &[4, 1, 42, 0]);
        assert_eq!(&o[o.len() - 4..], &[0x7F, 0xFF, 4, 0]);
    }
}
