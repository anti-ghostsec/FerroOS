//! The FerroOS vault: encrypted, crash-safe storage for what should survive
//! a reboot (settings, remembered choices, app data, documents), on a small
//! disk or partition.
//!
//! * The key is derived from the user's password with Argon2id, which is
//!   memory-hard: every guess costs 16 MiB and real time, so stolen-disk
//!   brute force is slow. The key itself is never stored.
//! * Contents are sealed with XChaCha20-Poly1305: confidentiality plus
//!   tamper detection (a modified disk fails to decrypt instead of feeding
//!   garbage to the system).
//! * Two slots, written alternately with a sequence number: a power cut
//!   while saving leaves the previous slot intact.
//! * Plaintext exists only in RAM; what reaches the disk is ciphertext.
//!
//! Layout: `[header 4 KiB][slot 0][slot 1]`, each slot
//! `[seq u64][nonce 24][len u32][ciphertext]`.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use std::fmt;
use std::io::{self, Read, Seek, SeekFrom, Write};
use zeroize::Zeroizing;

pub mod archive;

const MAGIC: &[u8; 8] = b"FERROVLT";
const VERSION: u32 = 1;
const HEADER_LEN: u64 = 4096;
const HEADER_USED: usize = 40;
const SLOT_HEAD: usize = 8 + 24 + 4;
const TAG: usize = 16;

/// Argon2id cost. 16 MiB fits a 64 MB machine while still making each
/// password guess expensive; `t` passes over that memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

pub const DEFAULT_KDF: KdfParams = KdfParams { m_kib: 16 * 1024, t: 3, p: 1 };

#[derive(Debug)]
pub enum VaultError {
    NotAVault,
    WrongPassword,
    TooLarge { size: usize, max: usize },
    Io(io::Error),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::NotAVault => f.write_str("no FerroOS vault on this disk"),
            VaultError::WrongPassword => f.write_str("wrong password (or the vault was tampered with)"),
            VaultError::TooLarge { size, max } => write!(f, "{} KB to save, but the vault holds {} KB", size / 1024, max / 1024),
            VaultError::Io(e) => write!(f, "disk error: {e}"),
        }
    }
}

impl From<io::Error> for VaultError {
    fn from(e: io::Error) -> Self {
        VaultError::Io(e)
    }
}

/// Anything that behaves like a disk: a block device, a partition, a file.
pub trait Device: Read + Write + Seek {
    /// Make writes durable (fsync).
    fn sync(&mut self) -> io::Result<()>;
}

impl Device for std::fs::File {
    fn sync(&mut self) -> io::Result<()> {
        self.sync_all()
    }
}

impl Device for io::Cursor<Vec<u8>> {
    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn read_at<D: Device>(d: &mut D, off: u64, buf: &mut [u8]) -> io::Result<()> {
    d.seek(SeekFrom::Start(off))?;
    d.read_exact(buf)
}

fn write_at<D: Device>(d: &mut D, off: u64, buf: &[u8]) -> io::Result<()> {
    d.seek(SeekFrom::Start(off))?;
    d.write_all(buf)
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("OS random source");
    b
}

fn derive_key(password: &str, salt: &[u8], kdf: KdfParams) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    let params = Params::new(kdf.m_kib, kdf.t, kdf.p, Some(32)).map_err(|_| VaultError::NotAVault)?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|_| VaultError::NotAVault)?;
    Ok(key)
}

/// True if the device holds a vault (only the public header is read).
pub fn is_vault<D: Device>(dev: &mut D) -> io::Result<bool> {
    let mut magic = [0u8; 8];
    Ok(read_at(dev, 0, &mut magic).is_ok() && &magic == MAGIC)
}

/// An unlocked vault. The key is wiped from memory when this is dropped.
pub struct Vault<D: Device> {
    dev: D,
    key: Zeroizing<[u8; 32]>,
    header: [u8; HEADER_USED],
    seq: u64,
    slot_size: u64,
}

impl<D: Device> Vault<D> {
    fn slot_size(dev: &mut D) -> io::Result<u64> {
        let len = dev.seek(SeekFrom::End(0))?;
        Ok(((len.saturating_sub(HEADER_LEN)) / 2) & !4095)
    }

    /// Largest payload that fits.
    pub fn capacity(&self) -> usize {
        (self.slot_size as usize).saturating_sub(SLOT_HEAD + TAG)
    }

    /// Formats `dev` as a new vault (destroying what's there) and stores
    /// `payload` as its first contents.
    pub fn create(mut dev: D, password: &str, kdf: KdfParams, payload: &[u8]) -> Result<Self, VaultError> {
        let slot_size = Self::slot_size(&mut dev)?;
        if slot_size < 8192 {
            return Err(VaultError::TooLarge { size: 0, max: 0 });
        }
        let salt: [u8; 16] = random();
        let mut header = [0u8; HEADER_USED];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&VERSION.to_le_bytes());
        header[12..16].copy_from_slice(&kdf.m_kib.to_le_bytes());
        header[16..20].copy_from_slice(&kdf.t.to_le_bytes());
        header[20..24].copy_from_slice(&kdf.p.to_le_bytes());
        header[24..40].copy_from_slice(&salt);
        let key = derive_key(password, &salt, kdf)?;
        // Invalidate both slots before the new header becomes visible.
        write_at(&mut dev, HEADER_LEN, &[0u8; SLOT_HEAD])?;
        write_at(&mut dev, HEADER_LEN + slot_size, &[0u8; SLOT_HEAD])?;
        let mut block = vec![0u8; HEADER_LEN as usize];
        block[..HEADER_USED].copy_from_slice(&header);
        write_at(&mut dev, 0, &block)?;
        let mut v = Self { dev, key, header, seq: 0, slot_size };
        v.save(payload)?;
        Ok(v)
    }

    /// Unlocks with `password`, returning the vault and its latest contents.
    pub fn unlock(mut dev: D, password: &str) -> Result<(Self, Vec<u8>), VaultError> {
        let mut header = [0u8; HEADER_USED];
        read_at(&mut dev, 0, &mut header)?;
        if &header[..8] != MAGIC || header[8..12] != VERSION.to_le_bytes() {
            return Err(VaultError::NotAVault);
        }
        let u32_at = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
        let kdf = KdfParams { m_kib: u32_at(12), t: u32_at(16), p: u32_at(20) };
        let key = derive_key(password, &header[24..40], kdf)?;
        let slot_size = Self::slot_size(&mut dev)?;
        let mut v = Self { dev, key, header, seq: 0, slot_size };
        let mut best: Option<(u64, Vec<u8>)> = None;
        for slot in 0..2 {
            if let Some((seq, plain)) = v.read_slot(slot)? {
                if best.as_ref().is_none_or(|(s, _)| seq > *s) {
                    best = Some((seq, plain));
                }
            }
        }
        let (seq, plain) = best.ok_or(VaultError::WrongPassword)?;
        v.seq = seq;
        Ok((v, plain))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(self.key.as_ref()))
    }

    /// Binds a slot's ciphertext to this vault's header, its sequence
    /// number and its position, so slots can't be swapped or replayed.
    fn aad(&self, seq: u64, slot: u64) -> Vec<u8> {
        let mut aad = self.header.to_vec();
        aad.extend_from_slice(&seq.to_le_bytes());
        aad.push(slot as u8);
        aad
    }

    fn read_slot(&mut self, slot: u64) -> Result<Option<(u64, Vec<u8>)>, VaultError> {
        let off = HEADER_LEN + slot * self.slot_size;
        let mut head = [0u8; SLOT_HEAD];
        read_at(&mut self.dev, off, &mut head)?;
        let seq = u64::from_le_bytes(head[..8].try_into().unwrap());
        let len = u32::from_le_bytes(head[32..36].try_into().unwrap()) as usize;
        if seq == 0 || len < TAG || len as u64 > self.slot_size - SLOT_HEAD as u64 {
            return Ok(None); // empty or never written
        }
        let mut ct = vec![0u8; len];
        read_at(&mut self.dev, off + SLOT_HEAD as u64, &mut ct)?;
        let aad = self.aad(seq, slot);
        let plain = self.cipher().decrypt(XNonce::from_slice(&head[8..32]), Payload { msg: &ct, aad: &aad });
        Ok(plain.ok().map(|p| (seq, p)))
    }

    /// Seals `payload` into the slot not holding the current contents, then
    /// syncs. Until that completes, the previous contents stay readable.
    pub fn save(&mut self, payload: &[u8]) -> Result<(), VaultError> {
        if payload.len() > self.capacity() {
            return Err(VaultError::TooLarge { size: payload.len(), max: self.capacity() });
        }
        let seq = self.seq + 1;
        let slot = seq % 2;
        let nonce: [u8; 24] = random();
        let aad = self.aad(seq, slot);
        let ct = self
            .cipher()
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: payload, aad: &aad })
            .map_err(|_| VaultError::Io(io::Error::other("encryption failed")))?;
        let mut block = Vec::with_capacity(SLOT_HEAD + ct.len());
        block.extend_from_slice(&seq.to_le_bytes());
        block.extend_from_slice(&nonce);
        block.extend_from_slice(&(ct.len() as u32).to_le_bytes());
        block.extend_from_slice(&ct);
        write_at(&mut self.dev, HEADER_LEN + slot * self.slot_size, &block)?;
        self.dev.sync()?;
        self.seq = seq;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams { m_kib: 64, t: 1, p: 1 };

    fn disk() -> io::Cursor<Vec<u8>> {
        io::Cursor::new(vec![0u8; 64 * 1024])
    }

    #[test]
    fn create_unlock_save_round_trip() {
        let v = Vault::create(disk(), "correct horse", FAST, b"settings v1").unwrap();
        let dev = v.dev;
        let (mut v, data) = Vault::unlock(dev, "correct horse").unwrap();
        assert_eq!(data, b"settings v1");
        v.save(b"settings v2").unwrap();
        v.save(b"settings v3").unwrap();
        let (_, data) = Vault::unlock(v.dev, "correct horse").unwrap();
        assert_eq!(data, b"settings v3");
    }

    #[test]
    fn wrong_password_and_not_a_vault() {
        let v = Vault::create(disk(), "right", FAST, b"x").unwrap();
        assert!(matches!(Vault::unlock(v.dev, "wrong"), Err(VaultError::WrongPassword)));
        assert!(matches!(Vault::unlock(disk(), "any"), Err(VaultError::NotAVault)));
        let mut d = disk();
        assert!(!is_vault(&mut d).unwrap());
    }

    #[test]
    fn ciphertext_only_on_disk() {
        let v = Vault::create(disk(), "pw", FAST, b"my secret browser history").unwrap();
        let raw = v.dev.into_inner();
        assert!(!raw.windows(6).any(|w| w == b"secret"));
    }

    #[test]
    fn torn_write_falls_back_to_previous_save() {
        let v = Vault::create(disk(), "pw", FAST, b"old").unwrap();
        let (mut v, _) = Vault::unlock(v.dev, "pw").unwrap();
        v.save(b"new").unwrap(); // seq 2, slot 0
                                 // Simulate power loss mid-write of the next save: corrupt slot 1's
                                 // ciphertext after giving it a newer sequence number.
        let off = (HEADER_LEN + v.slot_size) as usize;
        let mut raw = v.dev.into_inner();
        raw[off..off + 8].copy_from_slice(&3u64.to_le_bytes());
        raw[off + SLOT_HEAD] ^= 0xFF;
        let (_, data) = Vault::unlock(io::Cursor::new(raw), "pw").unwrap();
        assert_eq!(data, b"new");
    }

    #[test]
    fn tampering_is_detected() {
        let v = Vault::create(disk(), "pw", FAST, b"data").unwrap();
        // The first save (sequence 1) lands in slot 1.
        let off = (HEADER_LEN + v.slot_size) as usize + SLOT_HEAD + 2;
        let mut raw = v.dev.into_inner();
        raw[off] ^= 1;
        assert!(matches!(Vault::unlock(io::Cursor::new(raw), "pw"), Err(VaultError::WrongPassword)));
    }

    #[test]
    fn too_large_is_refused() {
        let v = Vault::create(disk(), "pw", FAST, b"").unwrap();
        let (mut v, _) = Vault::unlock(v.dev, "pw").unwrap();
        let big = vec![0u8; v.capacity() + 1];
        assert!(matches!(v.save(&big), Err(VaultError::TooLarge { .. })));
    }
}
