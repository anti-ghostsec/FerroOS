//! A GUID partition table (UEFI spec, chapter 5), written from scratch for
//! Setup: protective MBR, primary header and entries, backup copies at the
//! end of the disk.

pub const ESP_TYPE: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
pub const LINUX_DATA_TYPE: &str = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";
const ENTRIES: u64 = 128;
const ENTRY_SIZE: u64 = 128;

/// "C12A7328-F81F-..." in GPT's on-disk byte order (first three fields
/// little-endian, the rest as written).
pub fn guid(s: &str) -> [u8; 16] {
    let hex: Vec<u8> = s.bytes().filter(|b| *b != b'-').collect();
    let mut b = [0u8; 16];
    for (i, out) in b.iter_mut().enumerate() {
        let pair = std::str::from_utf8(&hex[i * 2..i * 2 + 2]).unwrap_or("00");
        *out = u8::from_str_radix(pair, 16).unwrap_or(0);
    }
    b[0..4].reverse();
    b[4..6].reverse();
    b[6..8].reverse();
    b
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

pub struct Part<'a> {
    pub type_guid: [u8; 16],
    pub guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64,
    pub name: &'a str,
}

/// The table for a disk of `lbas` blocks of `lbs` bytes: (bytes for the
/// start of the disk, bytes for its end, LBA where the end part goes).
pub fn build(lbas: u64, lbs: u64, disk_guid: [u8; 16], parts: &[Part]) -> (Vec<u8>, Vec<u8>, u64) {
    let entry_lbas = (ENTRIES * ENTRY_SIZE).div_ceil(lbs);
    let last = lbas - 1;
    let mut entries = vec![0u8; (ENTRIES * ENTRY_SIZE) as usize];
    for (i, p) in parts.iter().enumerate() {
        let e = &mut entries[i * ENTRY_SIZE as usize..(i + 1) * ENTRY_SIZE as usize];
        e[0..16].copy_from_slice(&p.type_guid);
        e[16..32].copy_from_slice(&p.guid);
        e[32..40].copy_from_slice(&p.first_lba.to_le_bytes());
        e[40..48].copy_from_slice(&p.last_lba.to_le_bytes());
        for (k, unit) in p.name.encode_utf16().take(36).enumerate() {
            e[56 + k * 2..58 + k * 2].copy_from_slice(&unit.to_le_bytes());
        }
    }
    let entries_crc = crc32(&entries);
    let header = |current: u64, backup: u64, entries_lba: u64| {
        let mut h = vec![0u8; lbs as usize];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&current.to_le_bytes());
        h[32..40].copy_from_slice(&backup.to_le_bytes());
        h[40..48].copy_from_slice(&(2 + entry_lbas).to_le_bytes());
        h[48..56].copy_from_slice(&(last - entry_lbas - 1).to_le_bytes());
        h[56..72].copy_from_slice(&disk_guid);
        h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
        h[80..84].copy_from_slice(&(ENTRIES as u32).to_le_bytes());
        h[84..88].copy_from_slice(&(ENTRY_SIZE as u32).to_le_bytes());
        h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        let crc = crc32(&h[..92]);
        h[16..20].copy_from_slice(&crc.to_le_bytes());
        h
    };

    // LBA 0: a protective MBR, so old tools see the disk as in use.
    let mut start = vec![0u8; lbs as usize];
    let mbr = &mut start[446..462];
    mbr[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    mbr[4] = 0xEE;
    mbr[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    mbr[8..12].copy_from_slice(&1u32.to_le_bytes());
    mbr[12..16].copy_from_slice(&(last.min(0xFFFF_FFFF) as u32).to_le_bytes());
    start[510] = 0x55;
    start[511] = 0xAA;
    start.extend(header(1, last, 2));
    start.extend(&entries);

    let backup_entries_lba = last - entry_lbas;
    let mut end = entries;
    end.extend(header(last, 1, backup_entries_lba));
    (start, end, backup_entries_lba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_and_guid_match_the_spec() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(guid(ESP_TYPE)[..4], [0x28, 0x73, 0x2A, 0xC1]);
        assert_eq!(guid(ESP_TYPE)[8..], [0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]);
    }

    #[test]
    fn table_is_self_consistent() {
        let parts = [Part { type_guid: guid(ESP_TYPE), guid: [7; 16], first_lba: 2048, last_lba: 4095, name: "ferro-vault" }];
        let (start, end, backup_at) = build(8192, 512, [9; 16], &parts);
        assert_eq!(&start[510..512], &[0x55, 0xAA]);
        let h = &start[512..1024];
        assert_eq!(&h[..8], b"EFI PART");
        let mut zeroed = h[..92].to_vec();
        zeroed[16..20].fill(0);
        assert_eq!(crc32(&zeroed).to_le_bytes(), h[16..20]);
        assert_eq!(u64::from_le_bytes(h[32..40].try_into().unwrap()), 8191, "backup header at the last LBA");
        assert_eq!(backup_at, 8191 - 32);
        let entries = &start[1024..1024 + 16384];
        assert_eq!(crc32(entries).to_le_bytes(), h[88..92]);
        assert_eq!(&entries[56..58], &[b'f', 0], "UTF-16 name");
        assert_eq!(end.len(), 16384 + 512);
        assert_eq!(&end[16384..16392], b"EFI PART");
    }
}
