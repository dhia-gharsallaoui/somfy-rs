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
//! The Wi-Fi passphrase and the broker password are stored in the clear. Flash
//! encryption is not enabled, and this crate does not pretend to a protection
//! it does not provide: anyone holding the board can read them out with
//! `espflash read-flash`. That is stated here rather than mitigated with an
//! obfuscation scheme, because an obfuscation scheme would change nothing
//! except how confident the reader felt.

use core::net::Ipv4Addr;

use crate::credentials::{CredentialError, Field, WifiCredentials, MAX_PSK_LEN, MAX_SSID_LEN};
use crate::mqtt::{
    MqttField, MqttSettings, MqttSettingsError, MAX_BROKER_PASSWORD_LEN, MAX_BROKER_USERNAME_LEN,
    MAX_TOPIC_ROOT_LEN,
};

/// Bytes in one config record, and therefore in one slot of the config ring.
///
/// 512 keeps the two flash constraints that matter: it is a whole number of
/// 4-byte flash words, and it divides a 4 KB erase sector exactly (eight
/// records per sector, sixteen across the two-sector `wificfg` region).
///
/// What it gives up, relative to the 256 this was before the MQTT settings
/// arrived, is that a record is no longer a *single* SPI NOR page program. That
/// was a nicety rather than a guarantee: a page program is not atomic against a
/// power cut either, which is exactly why every record carries a CRC and why
/// the ring never overwrites the newest one. A record torn across the page
/// boundary is reported as [`RecordError::Checksum`], which is what a record
/// torn inside one page already did.
pub const CONFIG_RECORD_LEN: usize = 512;

/// Marks a slot as this format's. Spells `RTSW` in a hex dump — RTS Wi-Fi —
/// and is deliberately distinct from the rolling-code store's `RTSC`, so a
/// region mounted at the wrong offset is reported rather than half-read.
const MAGIC: u32 = u32::from_le_bytes(*b"RTSW");

/// Bumped when the layout below changes. A record carrying a different version
/// is reported as such rather than as damage, so a later implementation can
/// migrate instead of erasing everything it does not recognise.
///
/// **Version 2 adds the MQTT settings and moves every field**, so a region
/// written by version 1 is not readable here. Because the record length changed
/// with it, a version 1 record read as 512 bytes fails its checksum before its
/// version is looked at, and is reported as damage rather than as an old
/// format. That is acceptable exactly once, in a store the plan marks as a
/// stopgap and whose only writer is the owner provisioning a board by hand: the
/// remedy is to re-provision, and the survey line at boot says the region needs
/// it.
const VERSION: u16 = 2;

/// Bit 0 of `flags`: this record carries Wi-Fi credentials. Clear means the
/// operator cleared them, which is a different fact from a blank region.
const FLAG_WIFI: u8 = 1 << 0;

/// Bit 1 of `flags`: this record carries MQTT settings. Clear means no broker
/// is configured, which is a device that receives and decodes and publishes
/// nothing — a supported configuration, not a broken one.
const FLAG_MQTT: u8 = 1 << 1;

/// Every flag bit this version defines.
const FLAGS_KNOWN: u8 = FLAG_WIFI | FLAG_MQTT;

const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

// Field offsets. Spelled out rather than computed so the layout can be read
// off the file and compared against a hex dump.
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_FLAGS: usize = 6;
const OFF_SSID_LEN: usize = 7;
const OFF_PSK_LEN: usize = 8;
const OFF_USERNAME_LEN: usize = 9;
const OFF_PASSWORD_LEN: usize = 10;
const OFF_PREFIX_LEN: usize = 11;
const OFF_SEQ: usize = 12;
const OFF_ROOT_LEN: usize = 16;
// 17 is unused padding, so the port and the address below land on their natural
// alignment in a hex dump and are readable at a glance.
const OFF_PORT: usize = 18;
const OFF_ADDRESS: usize = 20;
const OFF_SSID: usize = 24;
const OFF_PSK: usize = OFF_SSID + MAX_SSID_LEN;
const OFF_USERNAME: usize = OFF_PSK + MAX_PSK_LEN;
const OFF_PASSWORD: usize = OFF_USERNAME + MAX_BROKER_USERNAME_LEN;
const OFF_PREFIX: usize = OFF_PASSWORD + MAX_BROKER_PASSWORD_LEN;
const OFF_ROOT: usize = OFF_PREFIX + MAX_TOPIC_ROOT_LEN;
const OFF_CRC: usize = CONFIG_RECORD_LEN - 4;

// Every variable-length field must fit between the header and the checksum with
// room to spare, or a longer value in one would silently overwrite the next.
// Compile-time rather than a test, because it is arithmetic over constants and
// a test would only assert what the compiler already knows.
const _: () = assert!(
    OFF_ROOT + MAX_TOPIC_ROOT_LEN <= OFF_CRC,
    "the stored fields must fit inside the record"
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
    /// A stored length in the MQTT half does not fit the field it describes.
    MqttLength {
        /// The field whose length was wrong.
        field: MqttField,
        /// The length the record claimed.
        len: usize,
    },
    /// A field's bytes are not UTF-8, so they are not a name or a passphrase
    /// anything downstream could use.
    NotUtf8(Field),
    /// A field in the MQTT half is not UTF-8.
    MqttNotUtf8(MqttField),
    /// The record decoded, and the credentials it carries would have been
    /// refused had they been entered by hand.
    Credentials(CredentialError),
    /// The record decoded, and the MQTT settings it carries would have been
    /// refused had they been entered by hand.
    Mqtt(MqttSettingsError),
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
    /// The broker to talk to and the namespaces to talk in, or `None` for "no
    /// broker configured". A device with `None` here receives and decodes and
    /// publishes nothing, which is a supported configuration.
    pub mqtt: Option<MqttSettings>,
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
            bytes[OFF_FLAGS] |= FLAG_WIFI;
            let (ssid, psk) = (wifi.ssid().as_bytes(), wifi.psk().as_bytes());
            // Both lengths are bounded by `WifiCredentials`' own capacities,
            // which are the field widths here.
            bytes[OFF_SSID_LEN] = ssid.len() as u8;
            bytes[OFF_PSK_LEN] = psk.len() as u8;
            bytes[OFF_SSID..OFF_SSID + ssid.len()].copy_from_slice(ssid);
            bytes[OFF_PSK..OFF_PSK + psk.len()].copy_from_slice(psk);
        }

        if let Some(mqtt) = &self.mqtt {
            bytes[OFF_FLAGS] |= FLAG_MQTT;
            bytes[OFF_ADDRESS..OFF_ADDRESS + 4].copy_from_slice(&mqtt.address().octets());
            bytes[OFF_PORT..OFF_PORT + 2].copy_from_slice(&mqtt.port().to_le_bytes());
            // Every length below is bounded by `MqttSettings`' own capacities,
            // which are the field widths here.
            for (offset_len, offset, value) in [
                (OFF_USERNAME_LEN, OFF_USERNAME, mqtt.username()),
                (OFF_PASSWORD_LEN, OFF_PASSWORD, mqtt.password()),
                (OFF_PREFIX_LEN, OFF_PREFIX, mqtt.discovery_prefix()),
                (OFF_ROOT_LEN, OFF_ROOT, mqtt.state_root()),
            ] {
                bytes[offset_len] = value.len() as u8;
                bytes[offset..offset + value.len()].copy_from_slice(value.as_bytes());
            }
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
        if flags & !FLAGS_KNOWN != 0 {
            return Err(RecordError::Flags(flags));
        }

        let seq = u32::from_le_bytes(word(bytes, OFF_SEQ));
        Ok(ConfigRecord {
            seq,
            wifi: decode_wifi(bytes, flags)?,
            mqtt: decode_mqtt(bytes, flags)?,
        })
    }
}

/// The Wi-Fi half, or `None` if the record says there is none.
fn decode_wifi(
    bytes: &[u8; CONFIG_RECORD_LEN],
    flags: u8,
) -> Result<Option<WifiCredentials>, RecordError> {
    let ssid_len = bytes[OFF_SSID_LEN] as usize;
    let psk_len = bytes[OFF_PSK_LEN] as usize;

    if flags & FLAG_WIFI == 0 {
        // A record with no credentials must carry no credential bytes either,
        // or the two halves of it disagree about what it says.
        for (field, len) in [(Field::Ssid, ssid_len), (Field::Psk, psk_len)] {
            if len != 0 {
                return Err(RecordError::Length { field, len });
            }
        }
        return Ok(None);
    }

    for (field, len, limit) in [
        (Field::Ssid, ssid_len, MAX_SSID_LEN),
        (Field::Psk, psk_len, MAX_PSK_LEN),
    ] {
        if len > limit {
            return Err(RecordError::Length { field, len });
        }
    }

    let ssid = field_str(&bytes[OFF_SSID..OFF_SSID + ssid_len], Field::Ssid)?;
    let psk = field_str(&bytes[OFF_PSK..OFF_PSK + psk_len], Field::Psk)?;

    // Straight back through the same constructor bytes entered by hand go
    // through, so a record cannot deliver credentials the validator would have
    // refused — including the empty SSID that associates with nothing.
    WifiCredentials::new(ssid, psk)
        .map(Some)
        .map_err(RecordError::Credentials)
}

/// The MQTT half, or `None` if the record says there is none.
fn decode_mqtt(
    bytes: &[u8; CONFIG_RECORD_LEN],
    flags: u8,
) -> Result<Option<MqttSettings>, RecordError> {
    let fields = [
        (
            MqttField::Username,
            OFF_USERNAME_LEN,
            OFF_USERNAME,
            MAX_BROKER_USERNAME_LEN,
        ),
        (
            MqttField::Password,
            OFF_PASSWORD_LEN,
            OFF_PASSWORD,
            MAX_BROKER_PASSWORD_LEN,
        ),
        (
            MqttField::DiscoveryPrefix,
            OFF_PREFIX_LEN,
            OFF_PREFIX,
            MAX_TOPIC_ROOT_LEN,
        ),
        (
            MqttField::StateRoot,
            OFF_ROOT_LEN,
            OFF_ROOT,
            MAX_TOPIC_ROOT_LEN,
        ),
    ];

    if flags & FLAG_MQTT == 0 {
        // Same rule as the Wi-Fi half: a record that says "no broker" must not
        // also carry broker bytes, or it says two different things.
        for (field, offset_len, _, _) in fields {
            let len = bytes[offset_len] as usize;
            if len != 0 {
                return Err(RecordError::MqttLength { field, len });
            }
        }
        // The address *and the port*: both are broker bytes, and the rule is
        // that a record saying "no broker" must not carry any. `encode` zeroes
        // them, so a non-zero one means a foreign writer or a partly applied
        // clear — which is the case this check exists for.
        for (field, offset, len) in [
            (MqttField::Address, OFF_ADDRESS, 4),
            (MqttField::Port, OFF_PORT, 2),
        ] {
            if bytes[offset..offset + len].iter().any(|byte| *byte != 0) {
                return Err(RecordError::MqttLength { field, len });
            }
        }
        return Ok(None);
    }

    let mut text: [&str; 4] = [""; 4];
    for (index, (field, offset_len, offset, limit)) in fields.into_iter().enumerate() {
        let len = bytes[offset_len] as usize;
        if len > limit {
            return Err(RecordError::MqttLength { field, len });
        }
        text[index] = core::str::from_utf8(&bytes[offset..offset + len])
            .map_err(|_| RecordError::MqttNotUtf8(field))?;
    }

    let address = Ipv4Addr::from(word(bytes, OFF_ADDRESS));
    let port = u16::from_le_bytes([bytes[OFF_PORT], bytes[OFF_PORT + 1]]);

    // Through the same constructor a hand-entered setting goes through, for
    // the same reason as the Wi-Fi half: flash must not be able to deliver a
    // broker address nothing can connect to or a namespace pair that would put
    // availability on Home Assistant's own birth topic.
    MqttSettings::new(address, port, text[0], text[1], text[2], text[3])
        .map(Some)
        .map_err(RecordError::Mqtt)
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
    use crate::mqtt::{DEFAULT_DISCOVERY_PREFIX, DEFAULT_STATE_ROOT};

    fn record() -> ConfigRecord {
        ConfigRecord {
            seq: 7,
            wifi: Some(WifiCredentials::new("example-network", "PLACEHOLDER_PSK").expect("valid")),
            mqtt: Some(
                MqttSettings::new(
                    Ipv4Addr::new(192, 0, 2, 10),
                    1883,
                    "somfy",
                    "PLACEHOLDER_BROKER_PASSWORD",
                    DEFAULT_DISCOVERY_PREFIX,
                    DEFAULT_STATE_ROOT,
                )
                .expect("valid"),
            ),
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
            Err(RecordError::Flags(FLAGS_KNOWN | 0x80))
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
        let bytes = tampered(|bytes| bytes[OFF_ROOT_LEN] = 200);
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::MqttLength {
                field: MqttField::StateRoot,
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
            bytes[OFF_FLAGS] &= !FLAG_WIFI;
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

    /// And the same for the MQTT half, including its address — a record that
    /// says "no broker" while still carrying one is the state a partly applied
    /// clear would leave.
    #[test]
    fn a_cleared_broker_carrying_bytes_is_rejected() {
        let bytes = tampered(|bytes| bytes[OFF_FLAGS] &= !FLAG_MQTT);
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::MqttLength {
                field: MqttField::Username,
                len: "somfy".len(),
            })
        );

        let bytes = tampered(|bytes| {
            bytes[OFF_FLAGS] &= !FLAG_MQTT;
            bytes[OFF_PORT..OFF_PORT + 2].copy_from_slice(&0u16.to_le_bytes());
            for (field_len, field, limit) in [
                (OFF_USERNAME_LEN, OFF_USERNAME, MAX_BROKER_USERNAME_LEN),
                (OFF_PASSWORD_LEN, OFF_PASSWORD, MAX_BROKER_PASSWORD_LEN),
                (OFF_PREFIX_LEN, OFF_PREFIX, MAX_TOPIC_ROOT_LEN),
                (OFF_ROOT_LEN, OFF_ROOT, MAX_TOPIC_ROOT_LEN),
            ] {
                bytes[field_len] = 0;
                bytes[field..field + limit].fill(0);
            }
        });
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::MqttLength {
                field: MqttField::Address,
                len: 4,
            })
        );

        // And the port on its own, which the check used to miss entirely.
        let bytes = tampered(|bytes| {
            bytes[OFF_FLAGS] &= !FLAG_MQTT;
            bytes[OFF_ADDRESS..OFF_ADDRESS + 4].fill(0);
            for (field_len, field, limit) in [
                (OFF_USERNAME_LEN, OFF_USERNAME, MAX_BROKER_USERNAME_LEN),
                (OFF_PASSWORD_LEN, OFF_PASSWORD, MAX_BROKER_PASSWORD_LEN),
                (OFF_PREFIX_LEN, OFF_PREFIX, MAX_TOPIC_ROOT_LEN),
                (OFF_ROOT_LEN, OFF_ROOT, MAX_TOPIC_ROOT_LEN),
            ] {
                bytes[field_len] = 0;
                bytes[field..field + limit].fill(0);
            }
        });
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::MqttLength {
                field: MqttField::Port,
                len: 2,
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
        let bytes = tampered(|bytes| bytes[OFF_USERNAME] = 0xFF);
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::MqttNotUtf8(MqttField::Username))
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

    /// The same for the broker: a stored port of zero addresses nothing, and a
    /// record must not be able to deliver one.
    #[test]
    fn a_record_carrying_a_broker_the_validator_would_refuse_is_refused() {
        let bytes =
            tampered(|bytes| bytes[OFF_PORT..OFF_PORT + 2].copy_from_slice(&0u16.to_le_bytes()));
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::Mqtt(MqttSettingsError::PortZero))
        );

        let bytes =
            tampered(|bytes| bytes[OFF_ADDRESS..OFF_ADDRESS + 4].copy_from_slice(&[127, 0, 0, 1]));
        assert_eq!(
            ConfigRecord::decode(&bytes),
            Err(RecordError::Mqtt(MqttSettingsError::Unroutable(
                Ipv4Addr::LOCALHOST
            )))
        );
    }
}
