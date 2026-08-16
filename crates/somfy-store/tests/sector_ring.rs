//! Erase-unit arithmetic for the slot ring.
//!
//! NOR flash writes a word at a time but erases a whole sector, so a ring of
//! slots laid over it has to answer one extra question on every commit: does
//! this write need a sector erased first, and is anything irreplaceable inside
//! that sector? Getting it wrong loses every rolling code in 4 KB of flash, so
//! the answer is computed here, on the host, where it can be tested.

use somfy_store::{newest_slot, SectorRing, SlotWrite};

/// The geometry the firmware actually uses: an 8 KB partition of 256-byte
/// records over 4 KB erase sectors.
fn ring() -> SectorRing {
    SectorRing::new(8192, 256, 4096).expect("firmware geometry")
}

#[test]
fn the_firmware_geometry_is_two_sectors_of_sixteen_slots() {
    let ring = ring();
    assert_eq!(ring.layout().slot_count(), 32);
    assert_eq!(ring.sector_count(), 2);
    assert_eq!(ring.slots_per_sector(), 16);
}

/// One erased sector must never be able to take the newest record with it. A
/// single-sector region cannot promise that — the record being replaced is
/// always in the sector about to be erased — so it is rejected outright rather
/// than left as a configuration a caller could stumble into.
#[test]
fn a_region_of_fewer_than_two_sectors_is_rejected() {
    assert_eq!(SectorRing::new(4096, 256, 4096), None);
    assert_eq!(SectorRing::new(2048, 256, 4096), None);
    assert!(SectorRing::new(8192, 256, 4096).is_some());
}

/// A slot that straddles a sector boundary would be half-destroyed by one
/// erase, so slots must tile the erase unit exactly.
#[test]
fn a_slot_that_does_not_tile_the_erase_unit_is_rejected() {
    assert_eq!(SectorRing::new(8192, 300, 4096), None);
    assert_eq!(SectorRing::new(8192, 4096 * 2, 4096), None);
    assert_eq!(SectorRing::new(8192, 0, 4096), None);
    assert_eq!(SectorRing::new(8192, 256, 0), None);
}

/// A trailing part-sector could never be erased on its own, so a region that
/// is not a whole number of sectors is rejected rather than silently truncated.
#[test]
fn a_region_that_is_not_whole_sectors_is_rejected() {
    assert_eq!(SectorRing::new(8192 + 256, 256, 4096), None);
    assert_eq!(SectorRing::new(6000, 256, 4096), None);
}

#[test]
fn only_the_first_slot_of_a_sector_needs_an_erase() {
    let ring = ring();
    assert_eq!(ring.erase_before(0), Some(0));
    assert_eq!(ring.erase_before(16), Some(4096));
    for slot in (1..16).chain(17..32) {
        assert_eq!(ring.erase_before(slot), None, "slot {slot}");
    }
}

#[test]
fn a_slot_outside_the_ring_asks_for_no_erase() {
    assert_eq!(ring().erase_before(32), None);
    assert_eq!(ring().erase_before(usize::MAX), None);
}

/// The safety property the whole scheme rests on: whenever a commit erases a
/// sector, the record it is replacing — the newest one, the only copy of every
/// rolling code — is in a *different* sector. Walk the ring several times over
/// and check it at every single write.
#[test]
fn the_erased_sector_never_holds_the_newest_record() {
    let ring = ring();
    let mut newest: Option<SlotWrite> = None;

    for _ in 0..(ring.layout().slot_count() * 5) {
        let write = ring.layout().next_write(newest);
        if let Some(erased) = ring.erase_before(write.slot) {
            let erased_sector = erased / 4096;
            if let Some(previous) = newest {
                assert_ne!(
                    previous.slot / ring.slots_per_sector(),
                    erased_sector,
                    "erasing sector {erased_sector} would destroy the newest record \
                     in slot {}",
                    previous.slot
                );
            }
        }
        newest = Some(write);
    }
}

/// Same walk, but tracking which slots actually still hold a record after each
/// erase, and asserting the survivor set always contains the newest one. This
/// is the end-to-end version of the property above: it exercises `next_write`,
/// `erase_before` and `newest_slot` together, the way the firmware does.
#[test]
fn a_ring_walked_with_erases_always_still_holds_its_newest_record() {
    let ring = ring();
    let slots = ring.layout().slot_count();
    let mut sequences: Vec<Option<u32>> = vec![None; slots];
    let mut newest: Option<SlotWrite> = None;

    for _ in 0..(slots * 5) {
        let write = ring.layout().next_write(newest);

        if let Some(erased) = ring.erase_before(write.slot) {
            let first = erased / ring.layout().slot_len();
            for sequence in sequences[first..first + ring.slots_per_sector()].iter_mut() {
                *sequence = None;
            }
        }
        sequences[write.slot] = Some(write.seq);

        assert_eq!(
            newest_slot(&sequences),
            Some(write.slot),
            "after writing seq {} to slot {}",
            write.seq,
            write.slot
        );
        newest = Some(write);
    }
}

#[test]
fn a_write_lands_where_the_ring_points_when_that_slot_is_free() {
    let ring = ring();
    let free = vec![true; 32];
    for start in 0..32 {
        assert_eq!(ring.write_slot(start, &free), Some(start));
    }
}

/// The torn-write case, which is the reason `write_slot` exists. Slot 0 holds
/// the newest record and slot 1 holds the wreckage of a commit that lost power.
/// Aiming at slot 1 again would AND into it and fail forever, so the write must
/// step past it.
#[test]
fn a_write_steps_over_a_slot_left_half_written_by_a_power_cut() {
    let ring = ring();
    let mut free = vec![true; 32];
    free[0] = false; // the newest record
    free[1] = false; // torn
    assert_eq!(ring.write_slot(1, &free), Some(2));

    // And over a run of them, however many commits were interrupted.
    free[2] = false;
    free[3] = false;
    assert_eq!(ring.write_slot(1, &free), Some(4));
}

/// A slot that starts an erase unit is always available, whatever is in it,
/// because it is erased before it is written.
#[test]
fn a_sector_start_is_always_available() {
    let ring = ring();
    let free = vec![false; 32];
    assert_eq!(ring.write_slot(0, &free), Some(0));
    assert_eq!(ring.write_slot(16, &free), Some(16));
    // From anywhere else, the walk runs on to the next sector start.
    assert_eq!(ring.write_slot(1, &free), Some(16));
    assert_eq!(ring.write_slot(15, &free), Some(16));
    assert_eq!(ring.write_slot(17, &free), Some(0));
    assert_eq!(ring.write_slot(31, &free), Some(0));
}

/// The walk must terminate from every starting point even when nothing is
/// free, or a torn ring would have no way forward at all.
#[test]
fn a_write_always_finds_a_slot_however_full_the_ring_is() {
    let ring = ring();
    let free = vec![false; 32];
    for start in 0..32 {
        let slot = ring.write_slot(start, &free).expect("always finds a slot");
        let steps = (slot + 32 - start) % 32;
        assert!(
            steps < ring.slots_per_sector(),
            "walked {steps} slots from {start}, past a whole sector"
        );
    }
}

/// The safety property, now including the walk: whatever the ring has to step
/// over, the sector it ends up erasing is never the one holding the record
/// being replaced.
#[test]
fn stepping_over_wreckage_never_erases_the_newest_record() {
    let ring = ring();
    let per_sector = ring.slots_per_sector();

    for newest in 0..32 {
        // Worst case: nothing at all is free, so the walk runs as far as it can.
        let free = vec![false; 32];
        let start = ring.layout().next_write(Some(SlotWrite {
            slot: newest,
            seq: 1,
        }));
        let slot = ring.write_slot(start.slot, &free).expect("finds a slot");
        if let Some(erased) = ring.erase_before(slot) {
            assert_ne!(
                erased / 4096,
                newest / per_sector,
                "newest record in slot {newest} would be erased by a write landing in {slot}"
            );
        }
    }
}

#[test]
fn a_free_map_that_does_not_describe_this_ring_is_refused() {
    let ring = ring();
    assert_eq!(ring.write_slot(0, &[true; 31]), None);
    assert_eq!(ring.write_slot(0, &[true; 33]), None);
    assert_eq!(ring.write_slot(32, &[true; 32]), None);
}

/// What a slot holds, as flash sees it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SlotState {
    /// Erased: all ones.
    Blank,
    /// A record that passes its checksum.
    Valid(u32),
    /// A write that lost power part-way. Decodes as nothing, and cannot be
    /// written over until its sector is erased.
    Torn,
}

/// The whole scheme, simulated: lap the ring many times, tear every fourth
/// write, and check after every single operation that the newest valid record
/// is exactly the last one that finished.
///
/// This is the property the store's safety rests on. A tear must cost the
/// interrupted commit and nothing else — never the record before it, and never
/// the ring's ability to keep going.
#[test]
fn a_ring_torn_by_repeated_power_cuts_never_loses_the_last_good_record() {
    let ring = ring();
    let slots = ring.layout().slot_count();
    let mut flash = vec![SlotState::Blank; slots];
    let mut committed: Option<SlotWrite> = None;

    for attempt in 0..(slots * 6) {
        let free: Vec<bool> = flash.iter().map(|s| *s == SlotState::Blank).collect();
        let aim = ring.layout().next_write(committed);
        let slot = ring.write_slot(aim.slot, &free).expect("finds a slot");

        if let Some(erased) = ring.erase_before(slot) {
            let first = erased / ring.layout().slot_len();
            for state in flash[first..first + ring.slots_per_sector()].iter_mut() {
                *state = SlotState::Blank;
            }
        }

        // Every fourth commit loses power part-way through its write.
        let torn = attempt % 4 == 3;
        flash[slot] = if torn {
            SlotState::Torn
        } else {
            SlotState::Valid(aim.seq)
        };

        let sequences: Vec<Option<u32>> = flash
            .iter()
            .map(|state| match state {
                SlotState::Valid(seq) => Some(*seq),
                _ => None,
            })
            .collect();

        if torn {
            // The interrupted commit is lost — which is right, since it never
            // transmitted — and the record before it is still the newest.
            assert_eq!(
                newest_slot(&sequences),
                committed.map(|w| w.slot),
                "a torn write at attempt {attempt} disturbed the record before it"
            );
        } else {
            assert_eq!(
                newest_slot(&sequences),
                Some(slot),
                "the record committed at attempt {attempt} is not the newest"
            );
            committed = Some(SlotWrite { slot, seq: aim.seq });
        }
    }

    // And the ring really did keep going rather than stalling on the wreckage.
    assert!(committed.expect("commits succeeded").seq > slots as u32);
}

/// Endurance is the reason the ring exists at all. Sixteen slots per sector
/// means a sector is erased once per full lap of the ring, so 100k-cycle flash
/// takes 32x as many commits as writing one fixed record would.
#[test]
fn each_sector_is_erased_once_per_lap_of_the_ring() {
    let ring = ring();
    let slots = ring.layout().slot_count();
    let laps = 10;

    let mut erases = vec![0u32; ring.sector_count()];
    let mut newest = None;
    for _ in 0..(slots * laps) {
        let write = ring.layout().next_write(newest);
        if let Some(erased) = ring.erase_before(write.slot) {
            erases[erased / 4096] += 1;
        }
        newest = Some(write);
    }

    assert_eq!(erases, vec![laps as u32; ring.sector_count()]);
    // 32 commits per erase cycle against 100k cycles: 3.2M commits of headroom.
    assert_eq!(slots as u32 * 100_000, 3_200_000);
}
