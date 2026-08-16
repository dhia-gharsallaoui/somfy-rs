//! The bytes one config slot holds, and what it takes to believe them.
//!
//! Deliberately the same shape as `somfy_store::Record` — fixed length, magic,
//! version, CRC-32 over the whole thing — because it lives on the same kind of
//! region and fails in the same ways. What differs is the consequence: losing
//! a rolling code costs a physical re-pairing procedure at a shade, while
//! losing this record costs a Wi-Fi connection and a re-provisioning step. The
//! checks are the same because the *evidence* is the same; only the urgency is
//! different.
//!
//! ## These bytes are not secrets at rest
//!
//! The passphrase is stored in the clear. Flash encryption is not enabled, and
//! this crate does not pretend to a protection it does not provide: anyone
//! holding the board can read the network's passphrase out of it with
//! `espflash read-flash`. That is stated here rather than mitigated with an
//! obfuscation scheme, because an obfuscation scheme would change nothing
//! except how confident the reader felt.

use crate::credentials::{CredentialError, Field, WifiCredentials, MAX_PSK_LEN, MAX_SSID_LEN};

/// Bytes in one config record, and therefore in one slot of the config ring.
///
/// 256 for the same three reasons `somfy_store::RECORD_LEN` is: it divides a
/// 4 KB flash sector exactly, it is a whole number of 4-byte flash words, and
/// it is exactly one SPI NOR page — so every record is a single whole-page
/// program between erases, which is the pattern flash endurance figures are
/// quoted for.
pub const CONFIG_RECORD_LEN: usize = 256;

/// Marks a slot as this format's. Spells `RTSW` in a hex dump — RTS Wi-Fi —
/// and is deliberately distinct from the rolling-code store's `RTSC`, so a
/// region mounted at the wrong offset is reported rather than half-read.
const MAGIC: u32 = u32::from_le_bytes(*b"RTSW");

/// Bumped when the layout below changes. A record carrying a different version
/// is reported as such rather than as damage, so a later implementation can
/// migrate instead of erasing everything it does not recognise.
const VERSION: u16 = 1;

/// Bit 0 of `flags`: this record carries Wi-Fi credentials. Clear means the
/// operator cleared them, which is a different fact from a blank region.
const FLAG_WIFI: u8 = 1 << 0;

const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

// Field offsets. Spelled out rather than computed so the layout can be read
// off the file and compared against a hex dump.
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_FLAGS: usize = 6;
const OFF_SSID_LEN: usize = 7;
const OFF_PSK_LEN: usize = 8;
const OFF_SEQ: usize = 12;
const OFF_SSID: usize = 16;
const OFF_PSK: usize = OFF_SSID + MAX_SSID_LEN;
const OFF_CRC: usize = CONFIG_RECORD_LEN - 4;

// The two variable-length fields must fit between the header and the checksum
// with room to spare, or a longer SSID would silently overwrite the passphrase.
// Compile-time rather than a test, because it is arithmetic over constants and
// a test would only assert what the compiler already knows.
const _: () = assert!(
    OFF_PSK + MAX_PSK_LEN <= OFF_CRC,
    "the credential fields must fit inside the record"
);
// And the checksum must be the last four bytes, or `decode` would verify a
// window that does not cover the record.
const _: () = assert!(
    OFF_CRC + 4 == CONFIG_RECORD_LEN,
    "the checksum must occupy the last four bytes of the record"
);

/// Why a slot's bytes are not a config record.
///
/// [`Blank`](RecordError::Blank) is its own variant for the same reason it is
/// in the rolling-code store: an erased slot is the ordinary state of every
/// slot the ring has not reached, and a reader that cannot tell "never
/// written" from "damaged" cannot tell a first boot from data loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    /// Every byte is erased. The slot has never been written.
    Blank,
    /// Not this format's magic. Foreign data, or a write torn before the
    /// header landed.
    Magic,
    /// The checksum does not match the bytes — a torn write, or bit rot.
    Checksum,
    /// A record of some other version of this format.
    Version(u16),
    /// The flags byte has a bit set that this version does not define.
    Flags(u8),
    /// A stored length does not fit the field it describes. These lengths come
    /// off a device, so they are checked rather than trusted.
    Length {
        /// The field whose length was wrong.
        field: Field,
        /// The length the record claimed.
        len: usize,
    },
    /// A field's bytes are not UTF-8, so they are not a name or a passphrase
    /// anything downstream could use.
    NotUtf8(Field),
    /// The record decoded, and the credentials it carries would have been
    /// refused had they been entered by hand.
    Credentials(CredentialError),
}

/// One slot's worth of bytes: a sequence number and the configuration it
/// stamps.
///
/// The sequence number orders records around the ring — the same role, and the
/// same wrapping comparison, as in the rolling-code store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRecord {
    /// Monotonic write counter, wrapping at [`u32::MAX`].
    pub seq: u32,
    /// The network to join, or `None` for "no network configured" — which is
    /// a value an operator can write, not the absence of a record.
    pub wifi: Option<WifiCredentials>,
}

impl ConfigRecord {
    /// Serialise into the exact bytes a slot holds.
    ///
    /// Everything unused is zero-filled, so equal records produce identical
    /// bytes — which is what lets a store prove a write landed by reading it
    /// back and comparing — and so a hex dump of flash is readable rather than
    /// full of whatever happened to be in the buffer.
    pub fn encode(&self) -> [u8; CONFIG_RECORD_LEN] {
        let mut bytes = [0u8; CONFIG_RECORD_LEN];
        bytes[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&VERSION.to_le_bytes());
        bytes[OFF_SEQ..OFF_SEQ + 4].copy_from_slice(&self.seq.to_le_bytes());

        if let Some(wifi) = &self.wifi {
            bytes[OFF_FLAGS] = FLAG_WIFI;
            let (ssid, psk) = (wifi.ssid().as_bytes(), wifi.psk().as_bytes());
            // Both lengths are bounded by `WifiCredentials`' own capacities,
            // which are the field widths here.
            bytes[OFF_SSID_LEN] = ssid.len() as u8;
            bytes[OFF_PSK_LEN] = psk.len() as u8;
            bytes[OFF_SSID..OFF_SSID + ssid.len()].copy_from_slice(ssid);
            bytes[OFF_PSK..OFF_PSK + psk.len()].copy_from_slice(psk);
        }

        let checksum = CRC.checksum(&bytes[..OFF_CRC]);
        bytes[OFF_CRC..].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    /// Read a slot's bytes back, or say precisely why they are not a record.
    ///
    /// The checksum is verified **before** any field is interpreted, so a torn
    /// write is reported as [`RecordError::Checksum`] rather than as whatever
    /// its half-written header happens to spell.
    pub fn decode(bytes: &[u8; CONFIG_RECORD_LEN]) -> Result<ConfigRecord, RecordError> {
        if bytes.iter().all(|byte| *byte == 0xFF) {
            return Err(RecordError::Blank);
        }
        if u32::from_le_bytes(word(bytes, OFF_MAGIC)) != MAGIC {
            return Err(RecordError::Magic);
        }
        if u32::from_le_bytes(word(bytes, OFF_CRC)) != CRC.checksum(&bytes[..OFF_CRC]) {
            return Err(RecordError::Checksum);
        }

        let version = u16::from_le_bytes([bytes[OFF_VERSION], bytes[OFF_VERSION + 1]]);
        if version != VERSION {
            return Err(RecordError::Version(version));
        }

        let flags = bytes[OFF_FLAGS];
        if flags & !FLAG_WIFI != 0 {
            return Err(RecordError::Flags(flags));
        }

        let seq = u32::from_le_bytes(word(bytes, OFF_SEQ));
        let ssid_len = bytes[OFF_SSID_LEN] as usize;
        let psk_len = bytes[OFF_PSK_LEN] as usize;

        if flags & FLAG_WIFI == 0 {
            // A record with no credentials must carry no credential bytes
            // either, or the two halves of it disagree about what it says.
            if ssid_len != 0 {
                return Err(RecordError::Length {
                    field: Field::Ssid,
                    len: ssid_len,
                });
            }
            if psk_len != 0 {
                return Err(RecordError::Length {
                    field: Field::Psk,
                    len: psk_len,
                });
            }
            return Ok(ConfigRecord { seq, wifi: None });
        }

        if ssid_len > MAX_SSID_LEN {
            return Err(RecordError::Length {
                field: Field::Ssid,
                len: ssid_len,
            });
        }
        if psk_len > MAX_PSK_LEN {
            return Err(RecordError::Length {
                field: Field::Psk,
                len: psk_len,
            });
        }

        let ssid = field_str(&bytes[OFF_SSID..OFF_SSID + ssid_len], Field::Ssid)?;
        let psk = field_str(&bytes[OFF_PSK..OFF_PSK + psk_len], Field::Psk)?;

        // Straight back through the same constructor bytes entered by hand go
        // through, so a record cannot deliver credentials the validator would
        // have refused — including the empty SSID that associates with nothing.
        let wifi = WifiCredentials::new(ssid, psk).map_err(RecordError::Credentials)?;
        Ok(ConfigRecord {
            seq,
            wifi: Some(wifi),
        })
    }
}

/// One field's bytes as a `&str`, or which field was not UTF-8.
fn field_str(bytes: &[u8], field: Field) -> Result<&str, RecordError> {
    core::str::from_utf8(bytes).map_err(|_| RecordError::NotUtf8(field))
}

/// Four bytes at `at`, as an array. Panic-free by construction: every call
/// site passes a fixed offset within [`CONFIG_RECORD_LEN`].
fn word(bytes: &[u8; CONFIG_RECORD_LEN], at: usize) -> [u8; 4] {
    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ConfigRecord {
        ConfigRecord {
            seq: 7,
            wifi: Some(WifiCredentials::new("example-network", "PLACEHOLDER_PSK").expect("valid")),
        }
    }

    /// Re-stamp a record's bytes and re-checksum, as a writer of some other
    /// version of this region would. Every test below needs this, which is why
    /// they are here rather than in `tests/record.rs`.
    fn tampered(edit: impl FnOnce(&mut [u8; CONFIG_RECORD_LEN])) -> [u8; CONFIG_RECORD_LEN] {
        let mut bytes = record().encode();
        edit(&mut bytes);
        let checksum = CRC.checksum(&bytes[..OFF_CRC]);
        bytes[OFF_CRC..].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    /// The one thing here that could be silently wrong: whether the chosen CRC
    /// parameters are the ones everyone means by "CRC-32". This is the
    /// standard check value — the checksum of the ASCII digits 1 to 9.
    #[test]
    fn the_checksum_is_the_standard_crc32() {
        assert_eq!(CRC.checksum(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn a_record_of_another_version_names_the_version() {
        let bytes = tampered(|bytes| {
            bytes[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&9u16.to_le_bytes())
        });
        assert_eq!(ConfigRecord::decode(&bytes), Err(RecordError::Version(9)));
    }

    #[test]
    fn an_undefined_flag_bit_is_rejected() {
        let bytes = tampered(|bytes| bytes[OFF_FLAGS] |= 0x80);
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::Flags(FLAG_WIFI | 0x80))
        );
    }

    #[test]
    fn a_length_beyond_the_field_is_rejected() {
        let bytes = tampered(|bytes| bytes[OFF_SSID_LEN] = 200);
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::Length {
                field: Field::Ssid,
                len: 200,
            })
        );
        let bytes = tampered(|bytes| bytes[OFF_PSK_LEN] = 200);
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::Length {
                field: Field::Psk,
                len: 200,
            })
        );
    }

    /// A "no network configured" record whose credential bytes disagree with
    /// its flag is not a record that says nothing — it is a record that says
    /// two different things.
    #[test]
    fn a_cleared_record_carrying_a_length_is_rejected() {
        let bytes = tampered(|bytes| {
            bytes[OFF_FLAGS] = 0;
            bytes[OFF_PSK_LEN] = 0;
        });
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::Length {
                field: Field::Ssid,
                len: "example-network".len(),
            })
        );
    }

    #[test]
    fn a_field_that_is_not_utf8_names_the_field() {
        let bytes = tampered(|bytes| bytes[OFF_SSID] = 0xFF);
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::NotUtf8(Field::Ssid))
        );
    }

    /// Decoded values go through exactly the validation hand-entered ones do,
    /// so flash cannot deliver an SSID that associates with nothing.
    #[test]
    fn a_record_carrying_an_empty_ssid_is_refused() {
        let bytes = tampered(|bytes| bytes[OFF_SSID_LEN] = 0);
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::Credentials(CredentialError::Empty(
                Field::Ssid
            )))
        );
    }
}
