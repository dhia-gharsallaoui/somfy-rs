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
