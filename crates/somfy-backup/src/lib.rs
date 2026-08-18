//! `RTSB` — the configuration backup this device exports, and reads back.
//!
//! # What is in it, and what is deliberately not
//!
//! **In it:** the shade table, the estate (rooms, room assignments, groups),
//! and **the rolling codes** — which are the whole reason a backup is worth
//! having. Losing a rolling code costs a physical re-pairing at each motor, and
//! spec §12 names "backup export in the UI" as the mitigation for exactly that.
//!
//! **Not in it: the Wi-Fi passphrase and the broker password.** That is not an
//! oversight and it is not squeamishness. `somfy_api::settings` is built around
//! one structural rule — *no outbound type has a field a secret could be
//! written into* — and that rule is what makes an unauthenticated LAN API an
//! actuation risk rather than a credential-disclosure one. Authentication is
//! deferred (design spec §7.3), so the rule is the whole of the defence. An
//! export is a `GET`. An export carrying secrets would therefore be precisely
//! "the LAN API can be asked to read the passphrase out", written in one line,
//! and no amount of care elsewhere would undo it.
//!
//! **What is in it instead** is the *name* of each secret's owner: the SSID and
//! the broker's address, plus a flag for each saying whether a secret was set.
//! A person restoring onto a fresh board is then told exactly which two values
//! to retype rather than left to guess which network the old board was on. See
//! [`BackupMeta`].
//!
//! A restore therefore writes the shade table, the estate and the rolling
//! codes, and leaves the network settings alone. That is also why the format
//! carries no credential record at all rather than a redacted one: a
//! `somfy_config::ConfigRecord` with its passphrase removed decodes as an
//! **open network**, which is not "absent", it is *wrong*.
//!
//! # Why the two records are carried verbatim
//!
//! `shades` and `estate` are the exact bytes of the `RTSS` and `RTSE` flash
//! records, byte for byte, checksum included. Three things follow, and each of
//! them is a class of bug this format does not have:
//!
//! 1. **The decoder is the one the boot path already uses.** A restored table
//!    goes back through `ShadeRecord::for_each`, `StoredShade::new`,
//!    `ShadeConfig::new`, `FrameWidth::from_raw` and `Shade::link_remote`'s
//!    rules — the same functions that decide whether flash may deliver a shade
//!    — so a backup cannot carry a shade the device would refuse to run.
//! 2. **There is no second serialiser to drift.** Nothing here writes a shade;
//!    it copies one.
//! 3. **Their own version gates keep working.** `RTSS` is on version 4 and
//!    reads 1, 2 and 3; a backup taken from an older release restores through
//!    the same migration the boot path performs.
//!
//! # Rolling codes travel beside the table, not inside it
//!
//! The embedded `RTSS` record carries each shade's `initial_code`, which is the
//! **seed** the table was provisioned with and not what the counter has reached.
//! The live codes live in [`Codes`], a separate fixed block of
//! address-and-code pairs read out of the rolling-code store at the moment of
//! export.
//!
//! Keeping them apart was not the first design and it is much the better one.
//! Patching live codes *into* the shade record would mean rewriting that
//! record's own checksum, which means a pass over two kilobytes before the
//! first byte can be sent — on a device that streams this file out of flash a
//! few hundred bytes at a time and has nowhere to hold two kilobytes. It would
//! also blur two things that are deliberately separate everywhere else in this
//! firmware: the shade **table** is written debounced, and a rolling **code** is
//! written synchronously before every transmission, because a code that did not
//! reach flash costs a re-pairing at the motor.
//!
//! ## A restore cannot walk a code backwards
//!
//! That is the property this whole file exists for, and it is not enforced
//! here. `somfy_store::seed_if_absent` cannot express an overwrite at all — its
//! commit sits inside the branch where the read said there was nothing stored,
//! and no parameter reaches the other one. So a restore onto a board that
//! already has a code for an address **keeps the stored code**, which is the one
//! the motor has actually been driven with. A month-old file cannot move a
//! counter backwards, because nothing on the restore path is able to write a
//! code over one that exists.
//!
//! The consequence worth stating: restoring onto the *same* board is close to a
//! no-op for codes, and restoring onto a *fresh* board plants them. Those are
//! the two cases a backup is for.
//!
//! # Why this is a hand-written fixed layout, again
//!
//! CLAUDE.md's evaluation table rules twice on `postcard` for the flash
//! records, and both rulings carry to this container unchanged — it was
//! re-checked rather than assumed. The record is **fixed-length**, because the
//! firmware streams it out of a staging flash region in fixed-size chunks and
//! reads it back at a known offset; its two largest fields are *already*
//! encoded bytes, which a serde format would have to treat as opaque arrays and
//! frame anyway; and the framing that would remain — magic, version, flags,
//! length, checksum — **is** this format, so `postcard` would replace the ~60
//! lines of byte-shuffling below and none of the validation.
//!
//! It is also the fifth member of a family: `RTSC`, `RTSW`, `RTSS` and `RTSE`
//! all begin with a little-endian magic and a `u16` version and end with a
//! CRC-32/ISO-HDLC over everything before it. A varint container among four
//! fixed ones would cost the property that a hex dump can be read against the
//! file that wrote it — which is the reason the other four are shaped this way.

#![cfg_attr(not(test), no_std)]

use core::net::Ipv4Addr;

use somfy_config::{CredentialError, Field, ESTATE_RECORD_LEN, MAX_SSID_LEN, SHADE_RECORD_LEN};

/// Longest dotted-quad IPv4 address: `255.255.255.255`.
///
/// The field is padded to 16 so that every offset below stays a multiple of
/// four, which is what lets a reader check the layout arithmetic in their head
/// and what keeps the two 2 KiB records word-aligned inside the container.
pub const MAX_BROKER_LEN: usize = 15;

/// Address-and-code pairs the container can carry.
///
/// `somfy_domain::MAX_SHADES`, so a full registry's codes fit. Note that
/// `somfy_store::Record` itself holds at most thirty — a 256-byte record with a
/// 12-byte header, a 4-byte checksum and 8 bytes per entry — so a thirty-first
/// shade has nowhere to keep a code on the device either. Sizing this block at
/// the registry's capacity rather than the store's is deliberate: the block is
/// a *file* format and outliving that limitation costs 16 bytes.
pub const MAX_CODES: usize = 32;

/// Bytes per code entry: a 24-bit address in a `u32`, a `u16` code, and two
/// bytes of padding.
///
/// Padded to eight rather than packed to six so that every offset in this
/// format stays a multiple of four. That is worth two bytes an entry: it keeps
/// the two 2 KiB records word-aligned inside the container, which is what lets
/// the firmware hand `esp-storage` an aligned buffer rather than have it copy
/// through a 4 KiB sector buffer on this device's tightest stack.
const CODE_LEN: usize = 8;

/// Bytes of the container before the two records.
pub const HEADER_LEN: usize = 64 + MAX_CODES * CODE_LEN;

/// The whole container, in bytes.
///
/// Fixed, and that is a property the firmware relies on twice: the staging
/// region's occupancy is decided by comparing a `Content-Length` against this,
/// and the boot-side reader knows where the estate record starts without having
/// parsed anything.
pub const BACKUP_LEN: usize = HEADER_LEN + SHADE_RECORD_LEN + ESTATE_RECORD_LEN + CRC_LEN;

/// Marks a container this project wrote. Little-endian, like the four flash
/// records' magics, so a hex dump reads `RTSB` left to right.
const MAGIC: u32 = u32::from_le_bytes(*b"RTSB");

/// The version this build writes and the only one it reads.
///
/// There is exactly one, and no board carries an older container — the format
/// is introduced with the screen that produces it. A second version arrives
/// with a reader for the first, exactly as `RTSS` has four.
const VERSION: u16 = 1;

/// Bytes of checksum, at the end.
const CRC_LEN: usize = 4;

/// `CRC_32_ISO_HDLC`, as all four flash records use.
///
/// A `static` rather than a `const`, which is not a style choice: the firmware
/// streams a container out of flash a few hundred bytes at a time and keeps a
/// `crc::Digest` across those calls, and a `Digest` borrows the `Crc` it came
/// from. A `const` is materialised afresh at each use, so the borrow could not
/// outlive the expression. This is immutable, so it lands in read-only data —
/// flash on these parts — rather than in the DRAM the Wi-Fi driver's heap is
/// carved from.
pub static CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

// Field offsets. Spelled out rather than computed inline so that the layout in
// the table below and the code that reads it cannot disagree.
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_FLAGS: usize = 6;
const OFF_SSID_LEN: usize = 7;
const OFF_BROKER_LEN: usize = 8;
// 9..12 padding, zero.
const OFF_LENGTH: usize = 12;
const OFF_CODE_COUNT: usize = 9;
const OFF_SSID: usize = 16;
const OFF_BROKER: usize = 48;
/// Where the address-and-code block starts.
pub const OFF_CODES: usize = 64;
/// Where the embedded `RTSS` shade record starts.
pub const OFF_SHADES: usize = HEADER_LEN;
/// Where the embedded `RTSE` estate record starts.
pub const OFF_ESTATE: usize = OFF_SHADES + SHADE_RECORD_LEN;
const OFF_CRC: usize = OFF_ESTATE + ESTATE_RECORD_LEN;

/// A passphrase was stored for [`BackupMeta::ssid`].
const FLAG_PSK: u8 = 0b0000_0001;
/// A password was stored for [`BackupMeta::broker`].
const FLAG_BROKER_PASSWORD: u8 = 0b0000_0010;
/// Every bit this build understands. A container setting any other is refused
/// rather than masked — the same rule `somfy_config::ConfigRecord` applies to
/// its own flags, and for the same reason: an unknown bit means the writer knew
/// something this reader does not.
const FLAGS_KNOWN: u8 = FLAG_PSK | FLAG_BROKER_PASSWORD;

// The layout, asserted rather than commented. Each of these has failed in some
// codebase somewhere; here they are three lines.
const _: () = assert!(
    OFF_SSID + MAX_SSID_LEN <= OFF_BROKER,
    "the SSID overruns the broker field"
);
const _: () = assert!(
    OFF_BROKER + MAX_BROKER_LEN <= HEADER_LEN,
    "the broker overruns the header"
);
const _: () = assert!(
    OFF_CRC + CRC_LEN == BACKUP_LEN,
    "the checksum must be the last four bytes"
);

/// The non-secret half of a backup: which network and which broker, and whether
/// each had a secret.
///
/// **There is no field here a secret could be written into**, which is the same
/// structural rule `somfy_api::settings` states and the reason this type has
/// two booleans where a naive design would have had two strings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackupMeta {
    /// The network the exporting device was joined to, if it had one.
    pub ssid: Option<heapless::String<MAX_SSID_LEN>>,
    /// Whether that credential had a passphrase.
    ///
    /// `false` with an `ssid` present means an **open network**, which is a
    /// configuration and not an omission — and is exactly why this is a
    /// separate flag rather than "the passphrase field is empty".
    pub psk_was_set: bool,
    /// The broker the exporting device published to, if it had one.
    pub broker: Option<Ipv4Addr>,
    /// Whether that broker connection had a password.
    pub broker_password_was_set: bool,
}

/// The rolling codes a backup carries, as address-and-code pairs.
///
/// Read out of the rolling-code store at export and handed to
/// `somfy_store::seed_if_absent` one at a time at restore. A fixed array rather
/// than a `Vec` because the container's block is fixed and the firmware fills it
/// in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codes {
    entries: [(u32, u16); MAX_CODES],
    len: usize,
}

impl Default for Codes {
    fn default() -> Codes {
        Codes::new()
    }
}

impl Codes {
    /// An empty block, which is what a device with no shades exports.
    pub const fn new() -> Codes {
        Codes {
            entries: [(0, 0); MAX_CODES],
            len: 0,
        }
    }

    /// Add one address's code, if there is room.
    ///
    /// Returns whether it was taken. **A refusal is a lost rolling code**, so
    /// the caller has to report it rather than drop it — which is why this
    /// answers rather than saturating silently. It cannot happen on this
    /// firmware: the registry holds [`MAX_CODES`] shades and no more.
    #[must_use]
    pub fn push(&mut self, address: u32, code: u16) -> bool {
        if self.len == MAX_CODES {
            return false;
        }
        self.entries[self.len] = (address, code);
        self.len += 1;
        true
    }

    /// How many pairs the block holds.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the block is empty, which is an ordinary state.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The pairs, in the order they were added.
    pub fn iter(&self) -> impl Iterator<Item = (u32, u16)> + '_ {
        self.entries[..self.len].iter().copied()
    }
}

/// A decoded container, borrowing the two records rather than copying them.
///
/// Borrowed because both are two kilobytes and the one caller that decodes a
/// backup — the firmware's boot-side applier — has the bytes on its stack
/// already. Copying them would double a four-kilobyte working set on the one
/// device in this project that measures its stack in single kilobytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup<'a> {
    /// What the file says about the network settings it could not carry.
    pub meta: BackupMeta,
    /// The rolling codes, one per address the exporting device had one for.
    ///
    /// Applied through `somfy_store::seed_if_absent`, which is what makes a
    /// restore unable to move a counter backwards. See this module's docs.
    pub codes: Codes,
    /// The `RTSS` shade record, verbatim. Decode it with
    /// `somfy_config::ShadeRecord`.
    pub shades: &'a [u8; SHADE_RECORD_LEN],
    /// The `RTSE` estate record, verbatim. Decode it with
    /// `somfy_config::EstateRecord`.
    pub estate: &'a [u8; ESTATE_RECORD_LEN],
}

/// Why a container was refused.
///
/// Every variant refuses the **whole** file. That is the same rule
/// `ShadeRecord::for_each` enforces on the device: ids come from position, so
/// importing what parses and dropping what does not renumbers the shades after
/// the gap, which in Home Assistant is half an installation quietly renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupError {
    /// Every byte is `0xFF`. A staging region that has been erased and not
    /// written, which is an ordinary state and not damage.
    Blank,
    /// The first four bytes are not `RTSB`. Overwhelmingly the wrong file — a
    /// firmware image belongs at the update route, and a C++ ESPSomfy-RTS
    /// backup is text and is read by `somfy_migrate` instead.
    Magic,
    /// A container from a release this build does not read.
    Version(u16),
    /// The length field does not say [`BACKUP_LEN`].
    ///
    /// Checked **before** the checksum so that a truncated upload is reported
    /// as truncated rather than as corrupt: both are "upload it again", but only
    /// one of them tells the operator their transfer was cut short.
    Length(u32),
    /// The checksum does not match. Corruption, or a file edited by hand.
    Checksum,
    /// A flag bit this build does not define.
    ///
    /// Refused rather than masked, which is the rule `somfy_config::ConfigRecord`
    /// applies to its own flags and for the same reason: an unknown bit means
    /// the writer knew something this reader does not.
    Flags(u8),
    /// A length byte larger than the field it describes.
    FieldLength {
        /// Which field.
        field: MetaField,
        /// What it claimed.
        len: u8,
    },
    /// A text field that is not UTF-8.
    NotUtf8(MetaField),
    /// The broker field is not a dotted-quad IPv4 address.
    BrokerMalformed,
    /// The code block claims more pairs than it has room for.
    CodeCount(u8),
    /// A code block entry names an address the radio protocol cannot carry.
    ///
    /// Refused rather than skipped: a code planted against the wrong address is
    /// a shade that stops obeying, and there is no way to tell from here which
    /// of the two fields was wrong.
    CodeAddress(u32),
    /// The SSID is one `somfy_config::WifiCredentials::new` would refuse.
    ///
    /// Carried through the same constructor a typed-in SSID goes through, so a
    /// file cannot deliver a network name no other path could have stored.
    Credentials(CredentialError),
}

/// Which of the container's two text fields an error is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaField {
    /// The Wi-Fi network name.
    Ssid,
    /// The broker's address.
    Broker,
}

impl core::fmt::Display for MetaField {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MetaField::Ssid => formatter.write_str("ssid"),
            MetaField::Broker => formatter.write_str("broker address"),
        }
    }
}

impl core::fmt::Display for BackupError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BackupError::Blank => formatter.write_str("nothing has been uploaded"),
            BackupError::Magic => formatter.write_str("not a somfy-rs backup"),
            BackupError::Version(version) => {
                write!(
                    formatter,
                    "backup version {version} is not readable by this firmware"
                )
            }
            BackupError::Length(len) => write!(
                formatter,
                "the backup says it is {len} bytes and a backup is {BACKUP_LEN}",
            ),
            BackupError::Checksum => formatter.write_str("the backup's checksum does not match"),
            BackupError::Flags(raw) => write!(formatter, "unknown backup flags {raw:#04x}"),
            BackupError::FieldLength { field, len } => {
                write!(
                    formatter,
                    "the {field} claims {len} bytes, which does not fit its field"
                )
            }
            BackupError::NotUtf8(field) => write!(formatter, "the {field} is not valid UTF-8"),
            BackupError::CodeCount(count) => {
                write!(
                    formatter,
                    "the backup claims {count} rolling codes and a backup holds {MAX_CODES}"
                )
            }
            BackupError::CodeAddress(address) => {
                write!(formatter, "the backup carries a rolling code for address {address:#08x}, which is not a Somfy RTS address")
            }
            BackupError::BrokerMalformed => {
                formatter.write_str("the broker address is not a dotted-quad IPv4 address")
            }
            BackupError::Credentials(error) => write!(formatter, "{error}"),
        }
    }
}

impl core::error::Error for BackupError {}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Write the container's header.
///
/// Separate from the records so that an exporter can emit the file in pieces:
/// the firmware streams a backup out of flash a few hundred bytes at a time and
/// never holds four kilobytes anywhere. See [`checksum`], which is the other
/// half of that split.
pub fn write_header(meta: &BackupMeta, codes: &Codes, out: &mut [u8; HEADER_LEN]) {
    out.fill(0);
    out[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&MAGIC.to_le_bytes());
    out[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&VERSION.to_le_bytes());

    let mut flags = 0u8;
    if meta.psk_was_set {
        flags |= FLAG_PSK;
    }
    if meta.broker_password_was_set {
        flags |= FLAG_BROKER_PASSWORD;
    }
    out[OFF_FLAGS] = flags;

    if let Some(ssid) = &meta.ssid {
        let bytes = ssid.as_bytes();
        // Cannot exceed the field: the source is a `String<MAX_SSID_LEN>` and
        // the field is `MAX_SSID_LEN` wide.
        out[OFF_SSID_LEN] = bytes.len() as u8;
        out[OFF_SSID..OFF_SSID + bytes.len()].copy_from_slice(bytes);
    }

    if let Some(broker) = meta.broker {
        let mut text = heapless::String::<MAX_BROKER_LEN>::new();
        // Cannot fail: the widest dotted quad is `255.255.255.255`, which is
        // `MAX_BROKER_LEN` exactly.
        let _ = write_ipv4(&mut text, broker);
        let bytes = text.as_bytes();
        out[OFF_BROKER_LEN] = bytes.len() as u8;
        out[OFF_BROKER..OFF_BROKER + bytes.len()].copy_from_slice(bytes);
    }

    out[OFF_LENGTH..OFF_LENGTH + 4].copy_from_slice(&(BACKUP_LEN as u32).to_le_bytes());

    // Cannot exceed the block: `Codes::push` refuses past `MAX_CODES`, which is
    // what the field describes.
    out[OFF_CODE_COUNT] = codes.len() as u8;
    for (index, (address, code)) in codes.iter().enumerate() {
        let at = OFF_CODES + index * CODE_LEN;
        out[at..at + 4].copy_from_slice(&address.to_le_bytes());
        out[at + 4..at + 6].copy_from_slice(&code.to_le_bytes());
        // The two padding bytes stay zero, which is what makes equal
        // configurations encode identically.
    }
}

/// Format an address without `alloc`.
///
/// `Ipv4Addr`'s own `Display` would do, and it is reached through
/// `core::fmt::Write` here rather than through `format!` for the ordinary
/// `no_std` reason: there is nowhere to put a `String`.
fn write_ipv4(out: &mut heapless::String<MAX_BROKER_LEN>, address: Ipv4Addr) -> core::fmt::Result {
    use core::fmt::Write as _;
    write!(out, "{address}")
}

/// The checksum a container carries, over its header and its two records.
///
/// Taken over the three pieces rather than over one assembled buffer, for the
/// same reason [`write_header`] exists: the exporter never assembles one.
pub fn checksum(
    header: &[u8; HEADER_LEN],
    shades: &[u8; SHADE_RECORD_LEN],
    estate: &[u8; ESTATE_RECORD_LEN],
) -> u32 {
    let mut digest = CRC.digest();
    digest.update(header);
    digest.update(shades);
    digest.update(estate);
    digest.finalize()
}

/// Assemble a whole container.
///
/// The convenience the tests and the host tools use. **The firmware does not
/// call it** — four kilobytes is more than its web-server path has anywhere to
/// put — which is why the three functions it does call are the primitives above.
pub fn encode(
    meta: &BackupMeta,
    codes: &Codes,
    shades: &[u8; SHADE_RECORD_LEN],
    estate: &[u8; ESTATE_RECORD_LEN],
) -> [u8; BACKUP_LEN] {
    let mut header = [0u8; HEADER_LEN];
    write_header(meta, codes, &mut header);
    let crc = checksum(&header, shades, estate);

    let mut out = [0u8; BACKUP_LEN];
    out[..HEADER_LEN].copy_from_slice(&header);
    out[OFF_SHADES..OFF_SHADES + SHADE_RECORD_LEN].copy_from_slice(shades);
    out[OFF_ESTATE..OFF_ESTATE + ESTATE_RECORD_LEN].copy_from_slice(estate);
    out[OFF_CRC..].copy_from_slice(&crc.to_le_bytes());
    out
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Whether these bytes begin like a container.
///
/// **A cheap check on a prefix, and the reason it exists is the upload path**:
/// the firmware refuses a file that is obviously not a backup before it writes
/// a byte of flash, so an operator who picks the wrong file gets an immediate
/// `400` naming it rather than a staged upload, a restart and a refusal on the
/// far side. It says nothing about whether the file is *valid* — [`decode`]
/// decides that, and it is the only thing that does.
pub fn looks_like_backup(prefix: &[u8]) -> bool {
    prefix.len() >= 4 && prefix[..4] == MAGIC.to_le_bytes()
}

/// Read a container.
///
/// Checks in the order a reader would want them reported: blank, magic,
/// version, length, checksum, then the fields. **The checksum is verified
/// before any field is interpreted**, which is the rule all four flash records
/// follow — a value read out of a container that does not check out is not a
/// value, it is a guess.
pub fn decode(bytes: &[u8; BACKUP_LEN]) -> Result<Backup<'_>, BackupError> {
    if bytes.iter().all(|&byte| byte == 0xFF) {
        return Err(BackupError::Blank);
    }
    if bytes[OFF_MAGIC..OFF_MAGIC + 4] != MAGIC.to_le_bytes() {
        return Err(BackupError::Magic);
    }
    let version = u16::from_le_bytes([bytes[OFF_VERSION], bytes[OFF_VERSION + 1]]);
    if version != VERSION {
        return Err(BackupError::Version(version));
    }
    let length = u32::from_le_bytes(
        bytes[OFF_LENGTH..OFF_LENGTH + 4]
            .try_into()
            .expect("a four-byte window of a fixed array"),
    );
    if length as usize != BACKUP_LEN {
        return Err(BackupError::Length(length));
    }

    let stored = u32::from_le_bytes(
        bytes[OFF_CRC..OFF_CRC + 4]
            .try_into()
            .expect("a four-byte window of a fixed array"),
    );
    if stored != CRC.checksum(&bytes[..OFF_CRC]) {
        return Err(BackupError::Checksum);
    }

    let flags = bytes[OFF_FLAGS];
    if flags & !FLAGS_KNOWN != 0 {
        return Err(BackupError::Flags(flags));
    }

    let ssid = read_ssid(bytes)?;
    let broker = read_broker(bytes)?;
    let codes = read_codes(bytes)?;

    Ok(Backup {
        codes,
        meta: BackupMeta {
            ssid,
            psk_was_set: flags & FLAG_PSK != 0,
            broker,
            broker_password_was_set: flags & FLAG_BROKER_PASSWORD != 0,
        },
        shades: bytes[OFF_SHADES..OFF_SHADES + SHADE_RECORD_LEN]
            .try_into()
            .expect("a fixed window of a fixed array"),
        estate: bytes[OFF_ESTATE..OFF_ESTATE + ESTATE_RECORD_LEN]
            .try_into()
            .expect("a fixed window of a fixed array"),
    })
}

fn read_ssid(
    bytes: &[u8; BACKUP_LEN],
) -> Result<Option<heapless::String<MAX_SSID_LEN>>, BackupError> {
    let len = bytes[OFF_SSID_LEN];
    if len == 0 {
        return Ok(None);
    }
    if usize::from(len) > MAX_SSID_LEN {
        return Err(BackupError::FieldLength {
            field: MetaField::Ssid,
            len,
        });
    }
    let text = core::str::from_utf8(&bytes[OFF_SSID..OFF_SSID + usize::from(len)])
        .map_err(|_| BackupError::NotUtf8(MetaField::Ssid))?;
    // **Back through the same constructor a typed-in SSID goes through**, so a
    // file cannot deliver a network name no other path could have stored. The
    // constructed value is discarded; only the judgement is wanted, and the
    // passphrase it is judged with is empty because this format carries none —
    // an empty passphrase is legal (an open network), so this checks the SSID
    // alone, which is what it is here to do.
    somfy_config::WifiCredentials::new(text, "").map_err(|error| {
        // A passphrase rule cannot fire on an empty passphrase, so anything
        // reported here is about the SSID. Mapped rather than asserted, because
        // a `panic!` in a decoder reached from a boot path is a boot loop.
        if error.field() == Field::Psk {
            BackupError::NotUtf8(MetaField::Ssid)
        } else {
            BackupError::Credentials(error)
        }
    })?;
    let mut ssid = heapless::String::new();
    // Cannot fail: `len` is bounded by the capacity above.
    let _ = ssid.push_str(text);
    Ok(Some(ssid))
}

/// The largest address a Somfy RTS frame can carry, and the one value above it
/// that the domain refuses.
///
/// 24 bits, and `0xFF_FFFF` is refused by `somfy_domain` as a broadcast-shaped
/// value — the same two rules `somfy_config::EstateRecord` applies to a group's
/// address. Restated here rather than imported because this crate's job is to
/// refuse a *file*, and depending on the domain for one comparison would be a
/// dependency for a constant.
const MAX_RTS_ADDRESS: u32 = 0x00FF_FFFF;

fn read_codes(bytes: &[u8; BACKUP_LEN]) -> Result<Codes, BackupError> {
    let count = bytes[OFF_CODE_COUNT];
    if usize::from(count) > MAX_CODES {
        return Err(BackupError::CodeCount(count));
    }
    let mut codes = Codes::new();
    for index in 0..usize::from(count) {
        let at = OFF_CODES + index * CODE_LEN;
        let address = u32::from_le_bytes(
            bytes[at..at + 4]
                .try_into()
                .expect("a four-byte window of a fixed array"),
        );
        if address == 0 || address >= MAX_RTS_ADDRESS {
            return Err(BackupError::CodeAddress(address));
        }
        let code = u16::from_le_bytes([bytes[at + 4], bytes[at + 5]]);
        // Cannot refuse: the loop is bounded by `count`, which is bounded by
        // `MAX_CODES` above.
        let _ = codes.push(address, code);
    }
    Ok(codes)
}

fn read_broker(bytes: &[u8; BACKUP_LEN]) -> Result<Option<Ipv4Addr>, BackupError> {
    let len = bytes[OFF_BROKER_LEN];
    if len == 0 {
        return Ok(None);
    }
    if usize::from(len) > MAX_BROKER_LEN {
        return Err(BackupError::FieldLength {
            field: MetaField::Broker,
            len,
        });
    }
    let text = core::str::from_utf8(&bytes[OFF_BROKER..OFF_BROKER + usize::from(len)])
        .map_err(|_| BackupError::NotUtf8(MetaField::Broker))?;
    let address: Ipv4Addr = text.parse().map_err(|_| BackupError::BrokerMalformed)?;
    Ok(Some(address))
}
