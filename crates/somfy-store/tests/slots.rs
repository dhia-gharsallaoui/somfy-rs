//! Wear-levelling slot arithmetic.

use somfy_store::{newest_slot, SlotLayout, SlotWrite};

/// 4 KB region of 16-byte records — 256 slots.
fn layout() -> SlotLayout {
    SlotLayout::new(4096, 16).expect("layout")
}

#[test]
fn a_region_smaller_than_one_slot_is_rejected() {
    assert_eq!(SlotLayout::new(8, 16), None);
    assert_eq!(SlotLayout::new(0, 16), None);
    // A zero-length slot would divide by zero rather than merely be useless.
    assert_eq!(SlotLayout::new(4096, 0), None);
}

#[test]
fn slot_count_is_whole_slots_only() {
    assert_eq!(layout().slot_count(), 256);
    // A trailing partial slot is not usable and is not counted.
    assert_eq!(SlotLayout::new(100, 16).unwrap().slot_count(), 6);
}

#[test]
fn offsets_are_slot_aligned_and_bounded() {
    let l = layout();
    assert_eq!(l.offset(0), Some(0));
    assert_eq!(l.offset(1), Some(16));
    assert_eq!(l.offset(255), Some(4080));
    assert_eq!(l.offset(256), None);
    assert_eq!(l.offset(usize::MAX), None);
}

#[test]
fn the_first_write_of_a_blank_region_starts_at_slot_zero() {
    assert_eq!(layout().next_write(None), SlotWrite { slot: 0, seq: 0 });
}

#[test]
fn writes_advance_one_slot_and_one_sequence_number() {
    let l = layout();
    assert_eq!(
        l.next_write(Some(SlotWrite { slot: 0, seq: 0 })),
        SlotWrite { slot: 1, seq: 1 }
    );
    assert_eq!(
        l.next_write(Some(SlotWrite { slot: 7, seq: 99 })),
        SlotWrite { slot: 8, seq: 100 }
    );
}

#[test]
fn the_slot_index_wraps_at_the_end_of_the_region() {
    let l = SlotLayout::new(64, 16).unwrap(); // 4 slots
    assert_eq!(
        l.next_write(Some(SlotWrite { slot: 3, seq: 3 })),
        SlotWrite { slot: 0, seq: 4 }
    );
}

#[test]
fn the_sequence_number_wraps_at_u32_max() {
    let l = layout();
    assert_eq!(
        l.next_write(Some(SlotWrite {
            slot: 0,
            seq: u32::MAX
        })),
        SlotWrite { slot: 1, seq: 0 }
    );
}

/// The point of the ring: writes must be spread across every slot rather than
/// repeatedly erasing one. With 100k-cycle flash, hammering slot 0 exhausts the
/// region 256x sooner than cycling it.
#[test]
fn writes_spread_evenly_across_every_slot() {
    let l = SlotLayout::new(4096, 16).unwrap();
    let slot_count = l.slot_count();
    let writes = 10_000;

    let mut hits = vec![0u32; slot_count];
    let mut current = None;
    for _ in 0..writes {
        let w = l.next_write(current);
        assert!(w.slot < slot_count);
        hits[w.slot] += 1;
        current = Some(w);
    }

    let min = *hits.iter().min().unwrap();
    let max = *hits.iter().max().unwrap();
    // Perfect round-robin: no slot takes more than one extra write.
    assert!(
        max - min <= 1,
        "writes not spread: min {min}, max {max}, hits {hits:?}"
    );
    assert_eq!(hits.iter().sum::<u32>(), writes);
    // And the whole ring is genuinely used, not just a prefix of it.
    assert!(hits.iter().all(|&h| h > 0));
    // ~39 erase cycles per slot for 10k commits, against 100k endurance.
    assert_eq!(max, (writes as usize).div_ceil(slot_count) as u32);
}

#[test]
fn a_blank_region_has_no_newest_slot() {
    assert_eq!(newest_slot(&[]), None);
    assert_eq!(newest_slot(&[None, None, None]), None);
}

#[test]
fn the_newest_slot_is_the_highest_sequence_number() {
    assert_eq!(newest_slot(&[Some(3), Some(4), Some(1)]), Some(1));
    assert_eq!(newest_slot(&[Some(9), None, Some(2)]), Some(0));
    assert_eq!(newest_slot(&[None, Some(0)]), Some(1));
}

#[test]
fn a_partially_written_ring_finds_the_newest_record() {
    // Ring of 4 written 6 times: slots hold seqs 4, 5, 2, 3.
    assert_eq!(newest_slot(&[Some(4), Some(5), Some(2), Some(3)]), Some(1));
}

/// The sequence counter wraps long before the flash wears out, so the "newest"
/// comparison has to survive the wrap or the store would suddenly resume from
/// the oldest record it can find.
#[test]
fn newest_slot_survives_sequence_wrap_around() {
    // Slot 2 wrapped past u32::MAX; it is newer than the un-wrapped neighbours.
    let seqs = [
        Some(u32::MAX - 1),
        Some(u32::MAX),
        Some(0),
        Some(u32::MAX - 2),
    ];
    assert_eq!(newest_slot(&seqs), Some(2));

    // One further write on into the wrapped range.
    let seqs = [Some(1), Some(u32::MAX), Some(0), Some(u32::MAX - 1)];
    assert_eq!(newest_slot(&seqs), Some(0));
}

/// Walk a small ring far past the wrap point and confirm `newest_slot` always
/// names the slot just written.
#[test]
fn newest_slot_tracks_a_ring_walked_across_the_wrap() {
    let l = SlotLayout::new(64, 16).unwrap(); // 4 slots
    let mut seqs: Vec<Option<u32>> = vec![None; l.slot_count()];
    let mut current = Some(SlotWrite {
        slot: 0,
        seq: u32::MAX - 5,
    });
    seqs[0] = Some(u32::MAX - 5);

    for _ in 0..20 {
        let w = l.next_write(current);
        seqs[w.slot] = Some(w.seq);
        assert_eq!(
            newest_slot(&seqs),
            Some(w.slot),
            "after writing seq {} to slot {}: {seqs:?}",
            w.seq,
            w.slot
        );
        current = Some(w);
    }
}
