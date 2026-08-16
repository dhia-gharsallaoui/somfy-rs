//! Wear-levelling slot arithmetic — pure, so it is testable on the host.
//!
//! The rolling-code region is a ring of fixed-size slots. Each commit writes
//! the *next* slot with a sequence number one greater than the last, so writes
//! spread evenly around the ring instead of hammering one address, and the
//! newest record is the one with the highest sequence number.
//!
//! Nothing here touches flash and nothing here defines a record layout: the
//! firmware side owns the bytes and the I/O, and asks these functions where to
//! put them.

/// Geometry of the rolling-code region: how many slots, and where each starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotLayout {
    region_len: usize,
    slot_len: usize,
}

/// Where the next record goes and what sequence number it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotWrite {
    /// Slot index, always `< SlotLayout::slot_count`.
    pub slot: usize,
    /// Monotonic write counter; wraps at [`u32::MAX`].
    pub seq: u32,
}

impl SlotLayout {
    /// `None` unless the region holds at least one whole slot.
    pub const fn new(region_len: usize, slot_len: usize) -> Option<SlotLayout> {
        if slot_len == 0 || region_len < slot_len {
            return None;
        }
        Some(SlotLayout {
            region_len,
            slot_len,
        })
    }

    /// Slots that fit in the region. Always at least 1.
    pub const fn slot_count(&self) -> usize {
        self.region_len / self.slot_len
    }

    /// Bytes per slot, as given to [`SlotLayout::new`].
    pub const fn slot_len(&self) -> usize {
        self.slot_len
    }

    /// Byte offset of `slot` from the start of the region, or `None` if the
    /// index is out of range.
    pub const fn offset(&self, slot: usize) -> Option<usize> {
        if slot >= self.slot_count() {
            return None;
        }
        Some(slot * self.slot_len)
    }

    /// Where to write next, given the newest record found (or `None` on a
    /// blank region).
    ///
    /// Plain round-robin: slot `n + 1 mod count`, sequence `seq + 1`. That is
    /// the whole wear-levelling policy, and it is enough — every slot takes an
    /// equal share of the writes, so the region lasts `slot_count` times as
    /// long as a fixed address would.
    pub const fn next_write(&self, newest: Option<SlotWrite>) -> SlotWrite {
        match newest {
            None => SlotWrite { slot: 0, seq: 0 },
            Some(prev) => SlotWrite {
                // `% slot_count` also normalises an out-of-range `prev.slot`,
                // so a corrupt record cannot aim a write past the region.
                slot: (prev.slot % self.slot_count() + 1) % self.slot_count(),
                seq: prev.seq.wrapping_add(1),
            },
        }
    }
}

/// A [`SlotLayout`] laid over a medium that erases in units larger than a slot.
///
/// NOR flash writes a word at a time but can only return bits to one by erasing
/// a whole sector, so a ring living on it has to answer one question the plain
/// layout does not: **before writing this slot, must a sector be erased, and is
/// anything irreplaceable inside it?**
///
/// The construction rules exist to make the answer to the second half always
/// "no":
///
/// - slots tile the erase unit exactly, so one erase never takes half a record;
/// - the region is a whole number of erase units, so no part-sector is stranded;
/// - the region spans **at least two** erase units, which is what guarantees the
///   record being replaced — the newest one, the only copy of every rolling code
///   — is in a different unit from the one being erased. A single-unit region
///   cannot promise that at all, so it is rejected rather than left available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectorRing {
    layout: SlotLayout,
    erase_len: usize,
}

impl SectorRing {
    /// `None` unless the geometry satisfies every rule in the type's docs.
    pub const fn new(region_len: usize, slot_len: usize, erase_len: usize) -> Option<SectorRing> {
        let Some(layout) = SlotLayout::new(region_len, slot_len) else {
            return None;
        };
        if erase_len == 0 || !erase_len.is_multiple_of(slot_len) {
            return None;
        }
        if !region_len.is_multiple_of(erase_len) || region_len / erase_len < 2 {
            return None;
        }
        Some(SectorRing { layout, erase_len })
    }

    /// The slot arithmetic underneath: offsets, counts and [`SlotLayout::next_write`].
    pub const fn layout(&self) -> &SlotLayout {
        &self.layout
    }

    /// Erase units in the region. Always at least 2.
    pub const fn sector_count(&self) -> usize {
        self.layout.region_len / self.erase_len
    }

    /// Slots in one erase unit. Always at least 1, and always a whole number.
    pub const fn slots_per_sector(&self) -> usize {
        self.erase_len / self.layout.slot_len
    }

    /// Byte offset of the erase unit that must be cleared before writing
    /// `slot`, or `None` if the slot is already inside an erased unit.
    ///
    /// Only the first slot of a unit needs one: the ring advances by one slot
    /// per commit, so every later slot in that unit was erased by the same
    /// operation and has not been written since. An out-of-range slot asks for
    /// nothing — it cannot be written either.
    pub const fn erase_before(&self, slot: usize) -> Option<usize> {
        let Some(offset) = self.layout.offset(slot) else {
            return None;
        };
        if offset.is_multiple_of(self.erase_len) {
            Some(offset)
        } else {
            None
        }
    }

    /// Which slot a write starting at `start` should actually land in, given
    /// which slots are free. `free[i]` is true when slot `i` is erased.
    ///
    /// ## Why this is not just `start`
    ///
    /// Because of the torn write. Losing power mid-commit leaves a partial
    /// record in the slot the ring was writing, and on the next boot the newest
    /// *valid* record is the one before it — so [`SlotLayout::next_write`] aims
    /// at that same half-written slot again. Flash programming only clears
    /// bits, so writing there would AND into the wreckage and produce another
    /// unreadable record. The ring would never advance again: **one power cut
    /// at the wrong moment would wedge the store permanently**, and every
    /// subsequent commit would fail, which means nothing could ever transmit
    /// again either.
    ///
    /// So a write walks forward to the first slot that is free or that starts
    /// an erase unit — a unit start is erased before it is written, which makes
    /// it free. The walk terminates within [`SectorRing::slots_per_sector`]
    /// steps for that reason, and lands in the unit containing `start` or the
    /// one after it, never in the unit holding the record being replaced.
    ///
    /// `None` if `free` does not describe exactly this ring, or — impossibly,
    /// given the above — if no slot in range will take a write.
    pub fn write_slot(&self, start: usize, free: &[bool]) -> Option<usize> {
        let slot_count = self.layout.slot_count();
        if free.len() != slot_count || start >= slot_count {
            return None;
        }
        (0..self.slots_per_sector())
            .map(|step| (start + step) % slot_count)
            .find(|slot| self.erase_before(*slot).is_some() || free[*slot])
    }
}

/// Index of the newest record, given each slot's sequence number
/// (`None` for a blank or unreadable slot).
///
/// Comparison is wrapping, so a sequence counter that has rolled over past
/// [`u32::MAX`] still orders correctly: with far fewer than 2^31 live slots,
/// the newest is the one no other record is "ahead" of by less than half the
/// counter's range.
///
/// Returns `None` when no slot holds a record. Ties keep the earlier index;
/// duplicate sequence numbers mean a corrupt region and the caller should treat
/// the result as arbitrary.
pub fn newest_slot(sequences: &[Option<u32>]) -> Option<usize> {
    /// Half the counter's range. A difference below this is "ahead"; a
    /// difference at or above it is "behind, seen across the wrap".
    const HALF: u32 = 1 << 31;

    let mut best: Option<(usize, u32)> = None;
    for (index, sequence) in sequences.iter().enumerate() {
        let Some(sequence) = *sequence else { continue };
        let newer = match best {
            None => true,
            Some((_, best_seq)) => {
                let ahead = sequence.wrapping_sub(best_seq);
                ahead != 0 && ahead < HALF
            }
        };
        if newer {
            best = Some((index, sequence));
        }
    }
    best.map(|(index, _)| index)
}
