use crate::{Command, Frame};

/// Persisted per-shade rolling code. The transmitted frame carries the
/// current value; the store increments after building. The CALLER must
/// persist the incremented value BEFORE transmitting the frame
/// (spec §4 invariant).
///
/// Wire semantics mirror ESPSomfy-RTS `SomfyRemote::sendCommand`
/// (src/Somfy.cpp:3934-3946): the key byte is `0xA0 | (code & 0x0F)` derived
/// from the same rolling code placed in the frame, and each successive
/// transmission uses `code + 1`.
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
