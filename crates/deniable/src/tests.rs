use std::io::{Read, Seek, SeekFrom, Write};

use super::*;

/// Cheap enough for tests; real use takes [`Kdf::DEFAULT`] or more.
const FAST: Kdf = Kdf { memory_kib: 64, passes: 1, lanes: 1 };

fn fresh(blocks: u64) -> Store<MemDevice> {
    Store::format_with(MemDevice::new(blocks), FAST).unwrap()
}

fn reopen(store: Store<MemDevice>) -> Store<MemDevice> {
    Store::open_with(store.into_device(), FAST).unwrap()
}

fn read_all(store: &mut Store<MemDevice>, id: VolumeId) -> Vec<u8> {
    let mut out = vec![0; store.len(id).unwrap() as usize];
    assert_eq!(store.read(id, 0, &mut out).unwrap(), out.len());
    out
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

#[test]
fn a_volume_survives_closing_and_opening() {
    let mut store = fresh(256);
    let id = store.create(b"correct horse", &[]).unwrap();
    store.write(id, 0, b"hello, hidden world").unwrap();
    store.flush().unwrap();

    let mut store = reopen(store);
    let id = store.unlock(b"correct horse").unwrap();
    assert_eq!(read_all(&mut store, id), b"hello, hidden world");
}

#[test]
fn a_wrong_password_and_a_missing_volume_look_the_same() {
    let mut store = fresh(256);
    store.create(b"right", &[]).unwrap();
    store.flush().unwrap();
    let mut store = reopen(store);
    assert!(matches!(store.unlock(b"wrong"), Err(Error::NotFound)));
    let mut empty = fresh(256);
    assert!(matches!(empty.unlock(b"right"), Err(Error::NotFound)));
}

#[test]
fn the_top_password_opens_everything_and_the_lower_one_only_itself() {
    let mut store = fresh(512);
    let decoy = store.create(b"decoy", &[]).unwrap();
    let hidden = store.create(b"hidden", &[decoy]).unwrap();
    store.write(decoy, 0, b"shopping list").unwrap();
    store.write(hidden, 0, &pattern(20_000, 7)).unwrap();
    store.flush().unwrap();

    let mut store = reopen(store);
    let hidden = store.unlock(b"hidden").unwrap();
    let below = store.below(hidden).unwrap();
    assert_eq!(below.len(), 1);
    assert_eq!(read_all(&mut store, below[0]), b"shopping list");
    assert_eq!(read_all(&mut store, hidden), pattern(20_000, 7));

    // Under pressure: only the decoy is revealed, and nothing in it points up.
    let mut store = reopen(store);
    let decoy = store.unlock(b"decoy").unwrap();
    assert_eq!(store.volumes(), vec![decoy]);
    assert!(store.below(decoy).unwrap().is_empty());
    assert_eq!(read_all(&mut store, decoy), b"shopping list");
}

#[test]
fn working_with_everything_unlocked_never_harms_the_lower_volume() {
    let mut store = fresh(1024);
    let decoy = store.create(b"decoy", &[]).unwrap();
    store.write(decoy, 0, &pattern(50_000, 1)).unwrap();
    let hidden = store.create(b"hidden", &[decoy]).unwrap();
    store.flush().unwrap();
    for round in 0..20u8 {
        store.write(hidden, round as u64 * 1000, &pattern(30_000, round)).unwrap();
        store.flush().unwrap();
    }
    let mut store = reopen(store);
    let decoy = store.unlock(b"decoy").unwrap();
    assert_eq!(read_all(&mut store, decoy), pattern(50_000, 1));
}

#[test]
fn the_container_looks_random_before_and_after_use() {
    fn looks_random(bytes: &[u8]) {
        let mut counts = [0u64; 256];
        bytes.iter().for_each(|&b| counts[b as usize] += 1);
        let expected = bytes.len() as f64 / 256.0;
        let chi: f64 = counts.iter().map(|&c| (c as f64 - expected).powi(2) / expected).sum();
        // 255 degrees of freedom: anything this far out is not random.
        assert!(chi < 400.0, "chi-square {chi}");
        assert!(bytes.chunks(BLOCK).all(|b| b.iter().any(|&x| x != 0)), "an all-zero block");
    }
    let mut store = fresh(512);
    looks_random(store.dev.as_bytes());
    let id = store.create(b"pw", &[]).unwrap();
    // Very regular plaintext, which must not show through.
    store.write(id, 0, &vec![0u8; 200_000]).unwrap();
    store.write(id, 300_000, &vec![b'A'; 100_000]).unwrap();
    store.flush().unwrap();
    looks_random(store.dev.as_bytes());
    assert!(!store.dev.as_bytes().windows(64).any(|w| w.iter().all(|&b| b == b'A')));
}

#[test]
fn changes_that_were_never_saved_are_gone() {
    let mut store = fresh(256);
    let id = store.create(b"pw", &[]).unwrap();
    store.write(id, 0, b"kept").unwrap();
    store.flush().unwrap();
    store.write(id, 0, b"lost, never saved").unwrap();
    let mut store = reopen(store);
    let id = store.unlock(b"pw").unwrap();
    assert_eq!(read_all(&mut store, id), b"kept");
}

/// Writes go through until `torn_at` is written: that write leaves garbage
/// and fails, as a power cut in the middle of it would.
struct PowerCut {
    inner: MemDevice,
    torn_at: Option<u64>,
}

impl BlockDevice for PowerCut {
    fn blocks(&self) -> u64 {
        self.inner.blocks()
    }
    fn read(&mut self, index: u64, buf: &mut [u8; BLOCK]) -> io::Result<()> {
        self.inner.read(index, buf)
    }
    fn write(&mut self, index: u64, buf: &[u8; BLOCK]) -> io::Result<()> {
        if self.torn_at == Some(index) {
            let mut half = *buf;
            half[BLOCK / 2..].fill(0x5a);
            self.inner.write(index, &half)?;
            return Err(io::Error::other("power cut"));
        }
        self.inner.write(index, buf)
    }
    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_power_cut_while_saving_leaves_the_previous_state() {
    let mut store = Store::format_with(PowerCut { inner: MemDevice::new(256), torn_at: None }, FAST).unwrap();
    let id = store.create(b"pw", &[]).unwrap();
    store.write(id, 0, &pattern(9000, 3)).unwrap();
    store.flush().unwrap();

    store.write(id, 0, &pattern(9000, 4)).unwrap();
    let v = store.volume(id).unwrap();
    store.dev.torn_at = Some(v.slots[1 - v.current] as u64);
    assert!(store.flush().is_err());

    let mut dev = store.into_device();
    dev.torn_at = None;
    let mut store = Store::open_with(dev, FAST).unwrap();
    let id = store.unlock(b"pw").unwrap();
    let mut out = vec![0; 9000];
    store.read(id, 0, &mut out).unwrap();
    assert_eq!(out, pattern(9000, 3));
}

#[test]
fn partial_writes_holes_and_resizing() {
    let mut store = fresh(256);
    let id = store.create(b"pw", &[]).unwrap();
    // A hole: the first blocks were never written and read as zeros.
    store.write(id, 3 * PAYLOAD as u64 + 10, b"far out").unwrap();
    let mut out = vec![1u8; 3 * PAYLOAD + 17];
    store.read(id, 0, &mut out).unwrap();
    assert!(out[..3 * PAYLOAD + 10].iter().all(|&b| b == 0));
    assert_eq!(&out[3 * PAYLOAD + 10..], b"far out");

    // A write across a block boundary, then one inside it.
    store.write(id, PAYLOAD as u64 - 3, b"abcdef").unwrap();
    store.write(id, PAYLOAD as u64 - 1, b"Z").unwrap();
    let mut out = [0u8; 6];
    store.read(id, PAYLOAD as u64 - 3, &mut out).unwrap();
    assert_eq!(&out, b"abZdef");
    store.flush().unwrap();

    // Shrinking forgets the tail, and growing again reads zeros there.
    store.set_len(id, PAYLOAD as u64).unwrap();
    store.set_len(id, PAYLOAD as u64 + 100).unwrap();
    let mut out = [9u8; 100];
    store.read(id, PAYLOAD as u64, &mut out).unwrap();
    assert!(out.iter().all(|&b| b == 0));
    store.set_len(id, PAYLOAD as u64 - 2).unwrap();
    store.set_len(id, PAYLOAD as u64).unwrap();
    let mut out = [9u8; 2];
    store.read(id, PAYLOAD as u64 - 2, &mut out).unwrap();
    assert_eq!(out, [0, 0]);
    store.flush().unwrap();

    let mut store = reopen(store);
    let id = store.unlock(b"pw").unwrap();
    assert_eq!(store.len(id).unwrap(), PAYLOAD as u64);
    let mut out = [9u8; 3];
    store.read(id, PAYLOAD as u64 - 3, &mut out).unwrap();
    assert_eq!(&out, b"a\0\0");
}

#[test]
fn a_volume_bigger_than_one_map_block() {
    let mut store = fresh(4096);
    let id = store.create(b"pw", &[]).unwrap();
    let data = pattern(MAP_ENTRIES * PAYLOAD * 2 + 12345, 9);
    store.write(id, 0, &data).unwrap();
    store.flush().unwrap();
    assert_eq!(store.volume(id).unwrap().map_blocks.len(), 3);
    let mut store = reopen(store);
    let id = store.unlock(b"pw").unwrap();
    assert_eq!(read_all(&mut store, id), data);
}

#[test]
fn rewriting_keeps_the_same_number_of_blocks_in_use() {
    let mut store = fresh(512);
    let id = store.create(b"pw", &[]).unwrap();
    store.write(id, 0, &pattern(40_000, 0)).unwrap();
    store.flush().unwrap();
    let in_use = store.used_count;
    for round in 0..10 {
        store.write(id, 0, &pattern(40_000, round)).unwrap();
        store.write(id, 0, &pattern(40_000, round + 1)).unwrap();
        store.flush().unwrap();
        assert_eq!(store.used_count, in_use);
    }
}

#[test]
fn a_replaced_block_can_no_longer_be_read_with_the_key() {
    let mut store = fresh(256);
    let id = store.create(b"pw", &[]).unwrap();
    store.write(id, 0, b"first version").unwrap();
    store.flush().unwrap();
    let old = store.volume(id).unwrap().map[0];
    store.write(id, 0, b"second version").unwrap();
    store.flush().unwrap();
    let v = store.take(id).unwrap();
    assert!(store.read_sealed(&v.keys, KIND_DATA, old).unwrap().is_none());
}

#[test]
fn destroying_a_volume() {
    let mut store = fresh(512);
    let decoy = store.create(b"decoy", &[]).unwrap();
    let hidden = store.create(b"hidden", &[decoy]).unwrap();
    store.write(hidden, 0, b"top").unwrap();
    store.write(decoy, 0, b"bottom").unwrap();
    store.flush().unwrap();
    store.destroy(decoy).unwrap();
    store.flush().unwrap();

    let mut store = reopen(store);
    assert!(matches!(store.unlock(b"decoy"), Err(Error::NotFound)));
    let hidden = store.unlock(b"hidden").unwrap();
    assert!(store.below(hidden).unwrap().is_empty());
    assert_eq!(read_all(&mut store, hidden), b"top");
}

#[test]
fn a_password_can_only_be_used_once() {
    let mut store = fresh(256);
    store.create(b"pw", &[]).unwrap();
    assert!(matches!(store.create(b"pw", &[]), Err(Error::Exists)));
    store.flush().unwrap();
    // Not unlocked, but still there: making it again would wipe it.
    let mut store = reopen(store);
    assert!(matches!(store.create(b"pw", &[]), Err(Error::Exists)));
}

#[test]
fn a_full_container_says_so() {
    let mut store = fresh(MIN_BLOCKS);
    let id = store.create(b"pw", &[]).unwrap();
    assert!(matches!(store.write(id, 0, &vec![1; PAYLOAD * MIN_BLOCKS as usize]), Err(Error::Full)));
    assert!(matches!(Store::format_with(MemDevice::new(MIN_BLOCKS - 1), FAST), Err(Error::BadSize)));
}

#[test]
fn a_volume_as_a_file() {
    let mut store = fresh(256);
    let id = store.create(b"pw", &[]).unwrap();
    {
        let mut f = store.file(id);
        f.write_all(b"0123456789").unwrap();
        f.seek(SeekFrom::Start(2)).unwrap();
        f.write_all(b"ab").unwrap();
        f.seek(SeekFrom::End(-3)).unwrap();
        let mut tail = String::new();
        f.read_to_string(&mut tail).unwrap();
        assert_eq!(tail, "789");
        f.flush().unwrap();
    }
    let mut store = reopen(store);
    let id = store.unlock(b"pw").unwrap();
    let mut all = String::new();
    store.file(id).read_to_string(&mut all).unwrap();
    assert_eq!(all, "01ab456789");
}

#[test]
fn a_container_in_a_file() {
    let path = std::env::temp_dir().join(format!("deniable-test-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut store = Store::format_with(FileDevice::create(&path, 128).unwrap(), FAST).unwrap();
    let id = store.create(b"pw", &[]).unwrap();
    store.write(id, 0, b"on disk").unwrap();
    store.flush().unwrap();
    drop(store);
    let mut store = Store::open_with(FileDevice::open(&path).unwrap(), FAST).unwrap();
    let id = store.unlock(b"pw").unwrap();
    let mut out = [0u8; 7];
    store.read(id, 0, &mut out).unwrap();
    assert_eq!(&out, b"on disk");
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn a_volume_can_be_opened_with_a_key_instead_of_a_password() {
    let mut store = fresh(256);
    let key = [7u8; 32];
    let id = store.create_key(&key, &[]).unwrap();
    store.write(id, 0, b"keyed").unwrap();
    store.flush().unwrap();
    let mut store = reopen(store);
    let id = store.unlock_key(&key).unwrap();
    assert_eq!(read_all(&mut store, id), b"keyed");
}

/// The markers of the volume's two superblocks, as they are on the disk.
fn marks_on_disk<D: BlockDevice>(store: &mut Store<D>, id: VolumeId) -> Vec<u64> {
    let v = store.take(id).unwrap();
    let marks = v.slots.iter().filter_map(|&s| store.read_sealed(&v.keys, KIND_SUPER, s).unwrap()).map(|p| u64_at(&p[..], 8)).collect();
    store.volumes[id.0] = Some(v);
    marks
}

/// The bytes on the disk of the superblock the volume would be opened from.
fn current_superblock(store: &Store<MemDevice>, id: VolumeId) -> (u32, Vec<u8>) {
    let v = store.volume(id).unwrap();
    let slot = v.slots[v.current];
    (slot, store.dev.as_bytes()[slot as usize * BLOCK..(slot as usize + 1) * BLOCK].to_vec())
}

/// A save of only the hidden volume still saves the decoy below it, so
/// comparing copies of the disk with the decoy password always finds a
/// decoy save to account for the changed blocks.
#[test]
fn saving_the_hidden_volume_saves_the_decoy_too() {
    let mut store = fresh(512);
    let decoy = store.create(b"decoy", &[]).unwrap();
    let hidden = store.create(b"hidden", &[decoy]).unwrap();
    store.flush().unwrap();
    let before = store.volume(decoy).unwrap().gen;
    let (slot_before, _) = current_superblock(&store, decoy);
    store.write(hidden, 0, b"only the hidden volume changed").unwrap();
    store.flush().unwrap();
    // One step on, in the other slot: exactly what a save of the decoy does.
    assert_eq!(store.volume(decoy).unwrap().gen, next_gen(before), "the decoy's superblock stayed as it was");
    let (slot_after, bytes_after) = current_superblock(&store, decoy);
    assert_ne!(slot_after, slot_before, "the decoy's superblock wasn't written again");
    // Nothing changed: nothing is written.
    let after = store.volume(decoy).unwrap().gen;
    store.flush().unwrap();
    assert_eq!(store.volume(decoy).unwrap().gen, after);
    assert_eq!(current_superblock(&store, decoy), (slot_after, bytes_after));

    // And what was written is the newer copy: it's the one opened next time.
    let mut store = reopen(store);
    let decoy = store.unlock(b"decoy").unwrap();
    assert_eq!(current_superblock(&store, decoy).0, slot_after);
    assert_eq!(store.volume(decoy).unwrap().gen, after);
}

/// However often the volumes are saved, the marker in a superblock stays
/// 0, 1 or 2: the decoy's says nothing about how busy the disk has been.
#[test]
fn the_save_marker_never_counts() {
    let mut store = fresh(512);
    let decoy = store.create(b"decoy", &[]).unwrap();
    let hidden = store.create(b"hidden", &[decoy]).unwrap();
    store.write(decoy, 0, b"shopping list").unwrap();
    store.flush().unwrap();
    let mut seen = HashSet::new();
    for round in 0..100u8 {
        store.write(hidden, 0, &[round; 10]).unwrap();
        store.flush().unwrap();
        for id in [decoy, hidden] {
            let marks = marks_on_disk(&mut store, id);
            assert_eq!(marks.len(), 2);
            assert!(marks.iter().all(|m| m & GEN_CYCLE != 0 && m & !GEN_CYCLE < GEN_STEPS), "a marker of {marks:?} after {round} saves");
            seen.extend(marks);
        }
        if round % 7 == 0 {
            // The newest copy is still the one that opens.
            let mut again = Store::open_with(MemDevice::from_bytes(store.dev.as_bytes().to_vec()), FAST).unwrap();
            let hidden = again.unlock(b"hidden").unwrap();
            assert_eq!(read_all(&mut again, hidden), [round; 10]);
        }
    }
    assert_eq!(seen.len(), 3, "the marker took more than three values");
}

#[test]
fn the_newer_of_two_superblocks() {
    let c = |n: u64| GEN_CYCLE | n;
    // Around the cycle.
    assert!(newer(c(1), c(0)) && !newer(c(0), c(1)));
    assert!(newer(c(2), c(1)) && !newer(c(1), c(2)));
    assert!(newer(c(0), c(2)) && !newer(c(2), c(0)));
    // Counts from earlier versions: the larger one.
    assert!(newer(8, 7) && !newer(7, 8));
    assert!(newer(3, 2) && !newer(2, 3));
    // The first save by this version beats any count.
    assert!(newer(c(0), u64::MAX >> 1) && !newer(u64::MAX >> 1, c(0)));
    assert_eq!(next_gen(0), c(0));
    assert_eq!(next_gen(41), c(0));
    assert_eq!(next_gen(c(0)), c(1));
    assert_eq!(next_gen(c(2)), c(0));
}

/// A volume last saved by an earlier version, whose superblocks count
/// saves, opens at its newest state and carries on with the cycling marker.
#[test]
fn a_volume_with_counting_superblocks_carries_on() {
    for saves in 1..6u8 {
        let mut store = fresh(512);
        let id = store.create(b"pw", &[]).unwrap();
        // Save the way earlier versions did: generations 1, 2, 3...
        let mut v = store.take(id).unwrap();
        rewrite_generation(&mut store, &mut v, 1);
        store.volumes[id.0] = Some(v);
        for round in 0..saves {
            store.write(id, 0, &[round; 500]).unwrap();
            let mut v = store.take(id).unwrap();
            let counted = v.gen + 1;
            store.save(&mut v).unwrap();
            // Put the count where the marker went.
            rewrite_generation(&mut store, &mut v, counted);
            store.volumes[id.0] = Some(v);
        }
        let mut store = reopen(store);
        let id = store.unlock(b"pw").unwrap();
        assert_eq!(store.volume(id).unwrap().gen, saves as u64 + 1);
        assert_eq!(read_all(&mut store, id), vec![saves - 1; 500]);

        // Three more saves: one with a count still in the other slot, then
        // markers in both.
        for round in 0..3u8 {
            store.write(id, 0, &[100 + round; 500]).unwrap();
            store.flush().unwrap();
            store = reopen(store);
            let again = store.unlock(b"pw").unwrap();
            assert!(store.volume(again).unwrap().gen & GEN_CYCLE != 0);
            assert_eq!(read_all(&mut store, again), vec![100 + round; 500]);
        }
    }
}

/// Rewrites the volume's current superblock with `gen` in place of the
/// generation it was saved with.
fn rewrite_generation(store: &mut Store<MemDevice>, v: &mut Volume, gen: u64) {
    let slot = v.slots[v.current];
    let mut sup = store.read_sealed(&v.keys, KIND_SUPER, slot).unwrap().unwrap();
    sup[8..16].copy_from_slice(&gen.to_le_bytes());
    store.write_sealed(&v.keys, KIND_SUPER, slot, &sup).unwrap();
    v.gen = gen;
}

/// Saves the way containers were saved before the marker stopped counting:
/// the superblock holds the number of saves so far.
fn legacy_save(store: &mut Store<MemDevice>, id: VolumeId) {
    let mut v = store.take(id).unwrap();
    let chunks: Vec<&[u32]> = v.map.chunks(MAP_ENTRIES).collect();
    let new_map: Vec<u32> = chunks.iter().map(|_| store.allocate().unwrap()).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let mut payload = Zeroizing::new([0u8; PAYLOAD]);
        payload[..4].copy_from_slice(&new_map.get(i + 1).copied().unwrap_or(0).to_le_bytes());
        payload[4..8].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
        for (j, b) in chunk.iter().enumerate() {
            payload[8 + j * 4..12 + j * 4].copy_from_slice(&b.to_le_bytes());
        }
        store.write_sealed(&v.keys, KIND_MAP, new_map[i], &payload).unwrap();
    }
    let mut sup = Zeroizing::new([0u8; PAYLOAD]);
    let gen = v.gen + 1;
    sup[..8].copy_from_slice(SUPER_MAGIC);
    sup[8..16].copy_from_slice(&gen.to_le_bytes());
    sup[16..24].copy_from_slice(&v.len.to_le_bytes());
    sup[24..28].copy_from_slice(&new_map.first().copied().unwrap_or(0).to_le_bytes());
    sup[28..32].copy_from_slice(&(new_map.len() as u32).to_le_bytes());
    sup[32] = v.below.len() as u8;
    for (i, k) in v.below.iter().enumerate() {
        sup[SUPER_HEAD + i * 32..SUPER_HEAD + (i + 1) * 32].copy_from_slice(&k[..]);
    }
    let next = 1 - v.current;
    store.write_sealed(&v.keys, KIND_SUPER, v.slots[next], &sup).unwrap();
    let old_map = std::mem::replace(&mut v.map_blocks, new_map);
    for b in old_map.into_iter().chain(std::mem::take(&mut v.retired)) {
        store.scrub(b).unwrap();
    }
    v.current = next;
    v.gen = gen;
    v.fresh.clear();
    v.dirty = false;
    store.volumes[id.0] = Some(v);
}

/// A container whose superblocks hold save counters, as every container made
/// before the marker did, after `saves` saves of each volume.
fn legacy_container(saves: u64) -> (Store<MemDevice>, Vec<u8>, Vec<u8>) {
    let mut store = fresh(512);
    // Making a volume wrote its first superblock with a count of 1.
    let decoy = store.create(b"decoy", &[]).unwrap();
    let hidden = store.create(b"hidden", &[decoy]).unwrap();
    for id in [decoy, hidden] {
        let mut v = store.take(id).unwrap();
        rewrite_generation(&mut store, &mut v, 1);
        store.volumes[id.0] = Some(v);
    }
    let (mut bottom, mut top) = (Vec::new(), Vec::new());
    for n in 1..saves {
        bottom = pattern(5000, n as u8);
        top = pattern(20_000 + n as usize, n as u8 ^ 0x55);
        store.write(decoy, 0, &bottom).unwrap();
        store.write(hidden, 0, &top).unwrap();
        legacy_save(&mut store, decoy);
        legacy_save(&mut store, hidden);
        assert_eq!(store.volume(decoy).unwrap().gen, n + 1);
    }
    (reopen(store), bottom, top)
}

/// Containers written before the marker stopped counting open with their
/// newest state, whatever their counters had reached, and carry on working.
#[test]
fn a_container_with_save_counters_still_opens() {
    for saves in 1..=9 {
        let (mut store, bottom, top) = legacy_container(saves);
        let hidden = store.unlock(b"hidden").unwrap();
        let decoy = store.below(hidden).unwrap()[0];
        if saves > 1 {
            let mut counters = marks_on_disk(&mut store, hidden);
            counters.sort();
            assert_eq!(counters, [saves - 1, saves], "not the container this test means to open");
        }
        assert_eq!(read_all(&mut store, decoy), bottom);
        assert_eq!(read_all(&mut store, hidden), top);

        // The decoy password alone opens it too.
        let mut lower = Store::open_with(MemDevice::from_bytes(store.dev.as_bytes().to_vec()), FAST).unwrap();
        let only = lower.unlock(b"decoy").unwrap();
        assert_eq!(read_all(&mut lower, only), bottom);

        // Saving goes on from the old counter, and within two saves no
        // counter is left on the disk.
        for round in 0..5u8 {
            let top = pattern(15_000, round);
            store.set_len(hidden, 0).unwrap();
            store.write(hidden, 0, &top).unwrap();
            store.flush().unwrap();
            if round >= 1 {
                for id in [decoy, hidden] {
                    assert!(marks_on_disk(&mut store, id).iter().all(|m| m & GEN_CYCLE != 0), "a counter is still there after {round} saves");
                }
            }
            store = reopen(store);
            let again = store.unlock(b"hidden").unwrap();
            assert_eq!((again, store.below(again).unwrap()), (hidden, vec![decoy]));
            assert_eq!(read_all(&mut store, hidden), top);
            assert_eq!(read_all(&mut store, decoy), bottom);
        }
    }
}

/// A container written by the version before the marker cycled, with one
/// volume saved six times and another three: both open at their last save,
/// keep working, and survive a power cut during their first save here.
#[test]
fn a_container_from_the_earlier_version_opens_and_carries_on() {
    let old: &[u8] = include_bytes!("fixtures/counting-saves.bin");
    let (a, b) = ([7u8; 32], [9u8; 32]);

    let mut store = Store::open_with(MemDevice::from_bytes(old.to_vec()), FAST).unwrap();
    let ia = store.unlock_key(&a).unwrap();
    let ib = store.unlock_key(&b).unwrap();
    assert_eq!((store.volume(ia).unwrap().gen, store.volume(ib).unwrap().gen), (6, 3));
    assert_eq!(read_all(&mut store, ia), vec![4u8; 9000]);
    assert_eq!(read_all(&mut store, ib), vec![101u8; 5000]);

    // A power cut at every point of the first save by this version leaves
    // either the old contents or the new, never neither.
    let mut cut_at = 0;
    loop {
        let mut store = Store::open_with(FailAfter::new(MemDevice::from_bytes(old.to_vec()), cut_at), FAST).unwrap();
        let ia = store.unlock_key(&a).unwrap();
        store.unlock_key(&b).unwrap();
        let finished = store.write(ia, 0, &[55u8; 9000]).and_then(|_| store.flush()).is_ok();
        let mut after = Store::open_with(MemDevice::from_bytes(store.into_device().inner.into_bytes()), FAST).unwrap();
        let ia = after.unlock_key(&a).unwrap();
        let ib = after.unlock_key(&b).unwrap();
        let got = read_all(&mut after, ia);
        assert!(got == vec![4u8; 9000] || got == vec![55u8; 9000], "cut after {cut_at} writes");
        assert_eq!(read_all(&mut after, ib), vec![101u8; 5000], "cut after {cut_at} writes");
        if finished {
            assert_eq!(got, vec![55u8; 9000]);
            break;
        }
        cut_at += 1;
    }
    assert!(cut_at > 5);

    // And many saves on, reopening each time.
    for round in 0..7u8 {
        store.write(ia, 0, &[round; 9000]).unwrap();
        store.flush().unwrap();
        store = reopen(store);
        let ia2 = store.unlock_key(&a).unwrap();
        let ib2 = store.unlock_key(&b).unwrap();
        assert_eq!(read_all(&mut store, ia2), vec![round; 9000]);
        assert_eq!(read_all(&mut store, ib2), vec![101u8; 5000]);
        assert!(store.volume(ia2).unwrap().gen & GEN_CYCLE != 0 && store.volume(ib2).unwrap().gen & GEN_CYCLE != 0);
    }
}

/// A device whose writes start failing after a number of them, like a
/// power cut.
struct FailAfter {
    inner: MemDevice,
    left: u64,
}

impl FailAfter {
    fn new(inner: MemDevice, writes: u64) -> Self {
        Self { inner, left: writes }
    }
}

impl BlockDevice for FailAfter {
    fn blocks(&self) -> u64 {
        self.inner.blocks()
    }
    fn read(&mut self, index: u64, buf: &mut [u8; BLOCK]) -> io::Result<()> {
        self.inner.read(index, buf)
    }
    fn write(&mut self, index: u64, buf: &[u8; BLOCK]) -> io::Result<()> {
        if self.left == 0 {
            return Err(io::Error::other("power cut"));
        }
        self.left -= 1;
        self.inner.write(index, buf)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.inner.sync()
    }
}

/// Fails one write or sync, counted from when it's armed. A write that
/// fails leaves half a block of garbage behind.
struct Flaky {
    inner: MemDevice,
    fail_in: Option<u64>,
    failed: bool,
}

impl Flaky {
    fn step(&mut self) -> bool {
        match self.fail_in.as_mut() {
            Some(0) => {
                self.fail_in = None;
                self.failed = true;
                true
            }
            Some(n) => {
                *n -= 1;
                false
            }
            None => false,
        }
    }
}

impl BlockDevice for Flaky {
    fn blocks(&self) -> u64 {
        self.inner.blocks()
    }
    fn read(&mut self, index: u64, buf: &mut [u8; BLOCK]) -> io::Result<()> {
        self.inner.read(index, buf)
    }
    fn write(&mut self, index: u64, buf: &[u8; BLOCK]) -> io::Result<()> {
        if self.step() {
            let mut half = *buf;
            half[BLOCK / 2..].fill(0x5a);
            self.inner.write(index, &half)?;
            return Err(io::Error::other("write failed"));
        }
        self.inner.write(index, buf)
    }
    fn sync(&mut self) -> io::Result<()> {
        if self.step() {
            return Err(io::Error::other("sync failed"));
        }
        Ok(())
    }
}

const BOTTOM: usize = 7000;

/// A decoy and a hidden volume, both saved, on a device that can be made
/// to fail.
fn flaky(old: &[u8]) -> (Store<Flaky>, VolumeId, VolumeId) {
    let mut store = Store::format_with(Flaky { inner: MemDevice::new(256), fail_in: None, failed: false }, FAST).unwrap();
    let decoy = store.create(b"decoy", &[]).unwrap();
    let hidden = store.create(b"hidden", &[decoy]).unwrap();
    store.write(decoy, 0, &pattern(BOTTOM, 1)).unwrap();
    store.write(hidden, 0, old).unwrap();
    store.flush().unwrap();
    (store, decoy, hidden)
}

/// What the passwords open in a copy of the disk as it is now, as if the
/// machine had stopped here.
fn on_disk(store: &Store<Flaky>) -> (Vec<u8>, Vec<u8>) {
    let mut copy = Store::open_with(MemDevice::from_bytes(store.dev.inner.as_bytes().to_vec()), FAST).unwrap();
    let hidden = copy.unlock(b"hidden").expect("the hidden volume no longer opens");
    let decoy = copy.below(hidden).unwrap()[0];
    let found = (read_all(&mut copy, decoy), read_all(&mut copy, hidden));
    let mut lower = Store::open_with(copy.into_device(), FAST).unwrap();
    let only = lower.unlock(b"decoy").expect("the decoy no longer opens");
    assert_eq!(read_all(&mut lower, only), found.0);
    found
}

/// Runs `change` then a save with the `n`th write or sync failing, for every
/// `n`. Whenever it fails the disk must hold the old contents or the new,
/// and the open store must be able to carry on: `after` checks what it holds
/// once a save has gone through, and then the change is made again in full.
fn failing_everywhere(old: &[u8], new: &[u8], change: impl Fn(&mut Store<Flaky>, VolumeId) -> Result<()>, after: impl Fn(&[u8], bool)) {
    let mut failures = 0;
    for n in 0.. {
        let (mut store, decoy, hidden) = flaky(old);
        store.dev.fail_in = Some(n);
        let changed = change(&mut store, hidden);
        let saved = changed.is_ok() && store.flush().is_ok();
        let failed = store.dev.failed;
        store.dev.fail_in = None;

        // Stopping here: the old state or the new one, whole.
        let (bottom, top) = on_disk(&store);
        assert_eq!(bottom, pattern(BOTTOM, 1), "the decoy changed (failure {n})");
        assert!(top == old || top == new, "neither the old contents nor the new (failure {n})");
        if !failed {
            assert!(saved && top == new);
            break;
        }
        failures += 1;

        // Carrying on: the next save goes through, and what it saves opens.
        // The decoy is written to, so there's a save even if the failed
        // change left nothing to save in the hidden volume.
        store.write(decoy, 0, &pattern(1, 1)).unwrap();
        store.flush().unwrap_or_else(|e| panic!("the save after failure {n}: {e}"));
        let (bottom, top) = on_disk(&store);
        assert_eq!(bottom, pattern(BOTTOM, 1));
        after(&top, changed.is_ok());
        // Nothing is left in use that the volumes don't account for.
        let in_use: usize = store.volumes.iter().flatten().map(|v| 2 + v.map_blocks.len() + v.map.iter().filter(|&&b| b != 0).count()).sum();
        assert_eq!(store.used_count as usize, 1 + in_use, "blocks leaked (failure {n})");

        change(&mut store, hidden).unwrap();
        store.flush().unwrap();
        assert_eq!(on_disk(&store).1, new, "after failure {n}");
    }
    assert!(failures > 10, "only {failures} failures were tried");
}

/// A write that grows the volume and fails part-way must not leave the map
/// and the length disagreeing: saved like that, the volume would never
/// open again.
#[test]
fn a_write_that_fails_part_way_leaves_a_volume_that_opens() {
    let old = pattern(2 * PAYLOAD + 500, 2);
    let new = pattern(5 * PAYLOAD + 100, 3);
    failing_everywhere(
        &old,
        &new,
        |store, id| store.write(id, 0, &new),
        |top, wrote| {
            if wrote {
                assert_eq!(top, new);
            } else {
                // A short write: the new bytes up to where it stopped.
                assert!((old.len()..=new.len()).contains(&top.len()));
                let stopped = top.iter().zip(&new).take_while(|(a, b)| a == b).count();
                assert!(stopped % PAYLOAD == 0 || stopped == top.len(), "stopped at {stopped}");
                assert_eq!(top[stopped..], old[stopped.min(old.len())..]);
            }
        },
    );
}

/// The same for blocks rewritten before they were saved, a write past the
/// end, and cutting the volume short.
#[test]
fn rewriting_and_resizing_that_fail_part_way_leave_a_volume_that_opens() {
    let old = pattern(4 * PAYLOAD, 4);
    let mut new = pattern(3 * PAYLOAD, 5);
    new[100..2 * PAYLOAD].copy_from_slice(&pattern(2 * PAYLOAD - 100, 6));
    new.truncate(PAYLOAD + 300);
    new.resize(6 * PAYLOAD, 0);
    new.extend(b"past the end");
    let change = |store: &mut Store<Flaky>, id: VolumeId| {
        store.write(id, 0, &pattern(3 * PAYLOAD, 5))?;
        store.write(id, 100, &pattern(2 * PAYLOAD - 100, 6))?;
        store.set_len(id, PAYLOAD as u64 + 300)?;
        store.write(id, 6 * PAYLOAD as u64, b"past the end")
    };
    // Whatever was reached, it was read back whole: no block was lost.
    failing_everywhere(&old, &new, change, |top, changed| assert!(!changed || top == new));
}

/// A save that fails after the new superblock is written must not write
/// that slot's twin next: the new state is the saved one by then.
#[test]
fn a_save_that_fails_while_tidying_up_is_still_a_save() {
    let old = pattern(3 * PAYLOAD, 7);
    let new = pattern(3 * PAYLOAD, 8);
    let (mut store, _, hidden) = flaky(&old);
    store.set_cover(0, 0);
    store.write(hidden, 0, &new).unwrap();
    // The decoy saves first: one map block, a sync, its superblock, a sync,
    // and the old map block overwritten. The hidden volume's save is the
    // same again; this fails the first block it overwrites afterwards.
    store.dev.fail_in = Some(5 + 4);
    let before = store.volume(hidden).unwrap().current;
    assert!(store.flush().is_err());
    assert!(store.dev.failed);
    let v = store.volume(hidden).unwrap();
    assert_ne!(v.current, before, "the save wasn't taken as done");
    assert_eq!(on_disk(&store).1, new);
    store.flush().unwrap();
    assert_eq!(on_disk(&store).1, new);
}

#[test]
fn a_cost_too_low_or_too_high_is_refused() {
    /// A device that mustn't be read or written.
    struct Untouched;
    impl BlockDevice for Untouched {
        fn blocks(&self) -> u64 {
            MIN_BLOCKS
        }
        fn read(&mut self, _: u64, _: &mut [u8; BLOCK]) -> io::Result<()> {
            panic!("read before the cost was checked");
        }
        fn write(&mut self, _: u64, _: &[u8; BLOCK]) -> io::Result<()> {
            panic!("written before the cost was checked");
        }
        fn sync(&mut self) -> io::Result<()> {
            panic!("synced before the cost was checked");
        }
    }
    let refused = |kdf: Kdf| {
        assert!(!kdf.acceptable());
        assert!(matches!(Store::format(Untouched, kdf), Err(Error::Kdf)));
        assert!(matches!(Store::open(Untouched, kdf), Err(Error::Kdf)));
    };
    refused(FAST);
    refused(Kdf { memory_kib: Kdf::MIN_MEMORY_KIB - 1, ..Kdf::DEFAULT });
    refused(Kdf { memory_kib: Kdf::MAX_MEMORY_KIB + 1, ..Kdf::DEFAULT });
    refused(Kdf { memory_kib: u32::MAX, ..Kdf::DEFAULT });
    refused(Kdf { passes: 0, ..Kdf::DEFAULT });
    refused(Kdf { passes: Kdf::MAX_PASSES + 1, ..Kdf::DEFAULT });
    refused(Kdf { passes: u32::MAX, ..Kdf::DEFAULT });
    refused(Kdf { lanes: 0, ..Kdf::DEFAULT });
    refused(Kdf { lanes: Kdf::MAX_LANES + 1, ..Kdf::DEFAULT });

    assert!(Kdf::DEFAULT.acceptable());
    let least = Kdf { memory_kib: Kdf::MIN_MEMORY_KIB, passes: 1, lanes: 1 };
    let mut store = Store::format(MemDevice::new(MIN_BLOCKS), least).unwrap();
    let id = store.create(b"pw", &[]).unwrap();
    store.write(id, 0, b"kept").unwrap();
    store.flush().unwrap();
    let mut store = Store::open(store.into_device(), least).unwrap();
    let id = store.unlock(b"pw").unwrap();
    assert_eq!(read_all(&mut store, id), b"kept");
}

#[cfg(unix)]
#[test]
fn a_new_container_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("deniable-test-mode-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    FileDevice::create(&path, MIN_BLOCKS).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(mode & 0o077, 0, "others can open the container: {:o}", mode & 0o777);
}

/// The number of cover writes is drawn at random, not fixed by how much
/// was written.
#[test]
fn the_cover_write_count_varies() {
    struct Counting(MemDevice, u64);
    impl BlockDevice for Counting {
        fn blocks(&self) -> u64 {
            self.0.blocks()
        }
        fn read(&mut self, index: u64, buf: &mut [u8; BLOCK]) -> io::Result<()> {
            self.0.read(index, buf)
        }
        fn write(&mut self, index: u64, buf: &[u8; BLOCK]) -> io::Result<()> {
            self.1 += 1;
            self.0.write(index, buf)
        }
        fn sync(&mut self) -> io::Result<()> {
            self.0.sync()
        }
    }
    let mut counts = HashSet::new();
    let mut store = Store::format_with(Counting(MemDevice::new(1024), 0), FAST).unwrap();
    let id = store.create(b"pw", &[]).unwrap();
    store.flush().unwrap();
    for round in 0..12u8 {
        let start = store.dev.1;
        store.write(id, 0, &[round; 100]).unwrap();
        store.flush().unwrap();
        counts.insert(store.dev.1 - start);
    }
    assert!(counts.len() > 1, "every save wrote the same number of blocks");
}
