use crate::{Command, Frame};

/// Persisted per-shade rolling code. The transmitted frame carries the
/// current value; the store increments after building. The CALLER must
/// persist the incremented value BEFORE transmitting the frame (design-doc
/// invariant "persist before TX", docs/specs/2026-07-15-rust-rewrite-design.md §4).
///
/// The wire sequence real remotes use: the key byte is `0xA0 | (code & 0x0F)`
/// derived from the same rolling code placed in the frame, and each
/// successive transmission uses `code + 1`. Storage semantics can differ
/// across implementations, however: some persist the LAST-SENT code, while
/// `RollingCode` holds the NEXT-TO-SEND value.
///
/// # Migrating a last-sent-style stored value
///
/// Because of that off-by-one, anyone importing a rolling code that was
/// stored as last-sent must initialize with
/// `RollingCode(stored_last_sent.wrapping_add(1))`,
/// never `RollingCode(stored_last_sent)` — otherwise the first transmitted
/// frame replays the last-sent code and desyncs the motor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollingCode(pub u16);

impl RollingCode {
    pub fn next_frame(&mut self, command: Command, address: u32) -> Frame {
        let code = self.0;
        self.0 = self.0.wrapping_add(1);
        Frame {
            key: 0xA0 | (code as u8 & 0x0F),
            command,
            rolling_code: code,
            address,
        }
    }
}
