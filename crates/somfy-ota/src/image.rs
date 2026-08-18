//! The ESP-IDF application image format, as something that can be checked one
//! page at a time.
//!
//! # Why a streaming verifier and not a parser
//!
//! The image is over a megabyte and the device's whole heap is under seventy
//! kilobytes, so it is never in memory. It arrives as a sequence of pages, each
//! of which is written to flash and then forgotten. A verifier that needed the
//! whole file would therefore have to run *after* the write, which is the one
//! ordering an update must not have: by then the only way to reject the image
//! is to have already put it somewhere.
//!
//! So [`Verifier`] is fed the same pages the flash gets, in order, and refuses
//! at the earliest byte that can carry the refusal. The most valuable refusals
//! land in the first page: a file that is not an image at all is rejected at
//! byte 24, and one built for a different chip at byte 14 — both before a
//! sector has been erased.
//!
//! # What it checks, and what that is worth
//!
//! | Check | Catches | Refused at |
//! |---|---|---|
//! | Magic byte `0xE9` | an ELF, a `.tar`, a UI bundle, an empty body | byte 24 |
//! | [`Header::chip_id`] against this chip | the ESP32 build uploaded to an ESP32-S3 | byte 24 |
//! | Segment count in `1..=16` | a header that is not one | byte 24 |
//! | App-descriptor magic `0xABCD5432` | a bootloader image, a partition table | byte 112 |
//! | The segment walk reaching exactly `Content-Length` | truncation, extra bytes | the last byte |
//! | The image's own checksum byte | corruption anywhere in the segment data | the last byte |
//!
//! **What slips through, said plainly.** The checksum is one byte — the XOR of
//! every segment byte with a seed — so a corruption that survives TCP's own
//! sixteen-bit checksum has about a one-in-256 chance of surviving this one too.
//! Nothing here is a signature: there is no key, the endpoint is not
//! authenticated, and an attacker who can reach it can upload whatever they
//! like. What this establishes is *integrity against accident*, which is what
//! the failure modes of a manual upload over a LAN actually are.
//!
//! The image also carries a **SHA-256 of itself** in its last thirty-two bytes
//! ([`Header::hash_appended`], set on every image `espflash` produces), and the
//! ESP-IDF bootloader verifies it before booting. This crate does not: a
//! software SHA-256 is about 110 bytes of live state, and on the chip this
//! firmware is tightest on that is paid four times over in Wi-Fi driver
//! headroom. The consequence is bounded and worth stating — a corruption that
//! passes everything above is written, marked bootable, and then **refused by
//! the bootloader on the next boot, which falls back to the slot that was
//! running**. It costs one reboot, not a device.
//!
//! # Provenance
//!
//! Every constant below was read off real images built from this repository
//! rather than from a header file; `docs/provenance.md` records the images and
//! the day. The three chip ids, the checksum seed, the sixteen-byte alignment
//! rule and the descriptor's field offsets were all reproduced from those bytes
//! before they were written down.

use heapless::String;

/// The first byte of every ESP-IDF image.
pub const IMAGE_MAGIC: u8 = 0xE9;

/// Bytes of fixed header at the start of an image.
pub const HEADER_BYTES: usize = 24;

/// Bytes of header in front of each segment's data.
pub const SEGMENT_HEADER_BYTES: usize = 8;

/// Where the application descriptor starts.
///
/// It is the first thing in the first segment's data, and the first segment's
/// data starts after the image header and that segment's own header — so this
/// is [`HEADER_BYTES`] + [`SEGMENT_HEADER_BYTES`] rather than a number.
pub const DESCRIPTOR_OFFSET: usize = HEADER_BYTES + SEGMENT_HEADER_BYTES;

/// The application descriptor's own magic word.
pub const DESCRIPTOR_MAGIC: u32 = 0xABCD_5432;

/// Where the descriptor's `version` field starts, relative to the descriptor.
const DESCRIPTOR_VERSION_AT: usize = 16;

/// Where the descriptor's `project_name` field starts, relative to it.
const DESCRIPTOR_PROJECT_AT: usize = 48;

/// How long each of those two fields is.
pub const DESCRIPTOR_FIELD_BYTES: usize = 32;

/// How much of the image's start has to be buffered to check everything that
/// can be checked up front: the header, the descriptor's magic, and the two
/// fields worth printing back at the operator.
const HEAD_BYTES: usize = DESCRIPTOR_OFFSET + DESCRIPTOR_PROJECT_AT + DESCRIPTOR_FIELD_BYTES;

/// The seed the image's own checksum starts from.
///
/// `0xEF`, and it is the ROM's, not a choice. Reproduced from three real images
/// — see the module docs.
pub const CHECKSUM_SEED: u8 = 0xEF;

/// The block the checksum byte is made to land at the end of.
///
/// After the last segment, an image is padded with zeros until the *next* byte
/// written — the checksum — is the last byte of a sixteen-byte block.
pub const CHECKSUM_ALIGN: usize = 16;

/// Bytes of appended SHA-256, when [`Header::hash_appended`] is set.
pub const APPENDED_DIGEST_BYTES: usize = 32;

/// The most segments an image may declare.
///
/// ESP-IDF's own ceiling. A count outside `1..=MAX_SEGMENTS` is a header this
/// is not, which matters because the walk below is driven by that number.
pub const MAX_SEGMENTS: u8 = 16;

/// A chip this firmware is built for.
///
/// Only the three, deliberately. An image built for an ESP32-C6 is not a thing
/// this project can produce, so modelling its id would be modelling a value
/// that could only ever arrive by mistake — and [`ImageError::WrongChip`]
/// reports the raw number, which tells an operator more than a name this crate
/// guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    /// ESP32, Xtensa.
    Esp32,
    /// ESP32-S3, Xtensa.
    Esp32S3,
    /// ESP32-C3, RISC-V.
    Esp32C3,
}

impl Chip {
    /// The `chip_id` an image built for this chip carries.
    ///
    /// Read off real images rather than copied from a header: 0, 9 and 5, in
    /// that order. See the module docs.
    pub const fn id(self) -> u16 {
        match self {
            Chip::Esp32 => 0x0000,
            Chip::Esp32S3 => 0x0009,
            Chip::Esp32C3 => 0x0005,
        }
    }

    /// The name to print, which is the name `espflash --chip` takes.
    pub const fn name(self) -> &'static str {
        match self {
            Chip::Esp32 => "esp32",
            Chip::Esp32S3 => "esp32s3",
            Chip::Esp32C3 => "esp32c3",
        }
    }
}

/// The fixed header at the front of an image.
///
/// Only the fields anything here acts on. The SPI mode, speed and size, the
/// entry address and the revision bounds are all read by the bootloader and
/// none of them is a reason to refuse an upload — a mismatched SPI setting is a
/// board that flashes and runs, and a revision bound is the bootloader's to
/// enforce with information this firmware does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// How many segments follow.
    pub segment_count: u8,
    /// Which chip the image was built for. Compare against [`Chip::id`].
    pub chip_id: u16,
    /// Whether the last thirty-two bytes are a SHA-256 of everything before
    /// them. Every image `espflash` produces sets it; the bootloader checks it.
    pub hash_appended: bool,
}

impl Header {
    /// Read a header out of the first [`HEADER_BYTES`] of an image.
    ///
    /// Returns `None` only when `bytes` is short, which the caller has already
    /// excluded — the length check is here rather than a panic because a
    /// verifier that could panic on a network input is a reboot an attacker can
    /// ask for.
    pub fn parse(bytes: &[u8]) -> Option<Header> {
        let bytes: &[u8; HEADER_BYTES] = bytes.get(..HEADER_BYTES)?.try_into().ok()?;
        Some(Header {
            segment_count: bytes[1],
            chip_id: u16::from_le_bytes([bytes[12], bytes[13]]),
            hash_appended: bytes[23] == 1,
        })
    }
}

/// Why an upload was refused.
///
/// Every variant names a number the operator can compare against something they
/// have, which is the admission test: "this image is for chip 0 and this board
/// is chip 9" is a sentence somebody can act on, and "invalid image" is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// The first byte is not [`IMAGE_MAGIC`]. Almost always an ELF (`0x7F`) —
    /// `cargo build`'s output rather than `espflash save-image`'s.
    NotAnImage {
        /// What the first byte actually was.
        first: u8,
    },
    /// Built for a different chip.
    WrongChip {
        /// The `chip_id` in the image.
        found: u16,
        /// The `chip_id` this board answers to.
        expected: u16,
    },
    /// A segment count of zero or above [`MAX_SEGMENTS`].
    BadSegmentCount {
        /// What the header declared.
        found: u8,
    },
    /// No ESP-IDF application descriptor where one has to be. A bootloader
    /// image or a partition table reaches this, having passed the magic byte.
    NotAnApp {
        /// The word found where [`DESCRIPTOR_MAGIC`] belongs.
        magic: u32,
    },
    /// The image is larger than the slot it would be written to.
    TooLarge {
        /// The declared length.
        bytes: usize,
        /// What the slot holds.
        slot: usize,
    },
    /// The declared length was reached before the image's structure ended.
    Truncated {
        /// How many bytes the structure still needed.
        missing: usize,
    },
    /// The image's structure ended before the declared length was reached, or
    /// more bytes arrived than were declared.
    LengthMismatch {
        /// Where the structure ended.
        walked: usize,
        /// What `Content-Length` said.
        declared: usize,
    },
    /// The image's own checksum byte does not match its segment data.
    BadChecksum {
        /// What the segments compute to.
        computed: u8,
        /// What the image says.
        found: u8,
    },
}

/// Where the walk currently is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Reading the fixed header.
    Header,
    /// Reading a segment's own header. Carries how many segments are left
    /// *including* this one.
    SegmentHeader {
        /// Segments still to come, this one included.
        left: u8,
    },
    /// Reading a segment's data.
    SegmentData {
        /// Bytes of this segment still to come.
        remaining: usize,
        /// Segments after this one.
        left: u8,
    },
    /// Reading the zero padding before the checksum.
    Padding {
        /// Padding bytes still to come.
        remaining: usize,
    },
    /// Reading the one checksum byte.
    Checksum,
    /// Reading the appended SHA-256, which this crate does not verify.
    Digest {
        /// Bytes still to come.
        remaining: usize,
    },
    /// The structure is complete.
    Done,
}

/// An image that passed every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// How long it was, which is both what was declared and what was walked —
    /// they had to agree to get here.
    pub len: usize,
    /// The chip it was built for.
    pub chip: Chip,
    /// The descriptor's `version` field, which is the crate version the image
    /// was built from.
    pub version: String<DESCRIPTOR_FIELD_BYTES>,
    /// The descriptor's `project_name` field.
    pub project: String<DESCRIPTOR_FIELD_BYTES>,
}

/// Checks an image as it streams past.
///
/// Feed it exactly the bytes that go to flash, in order, then call
/// [`Verifier::finish`]. It holds [`HEAD_BYTES`] of the image's start and a
/// handful of counters, and nothing else — see the module docs for why that
/// bound is the design rather than an optimisation.
#[derive(Debug)]
pub struct Verifier {
    /// The chip this board is, which the image has to agree with.
    chip: Chip,
    /// What `Content-Length` said.
    declared: usize,
    /// Bytes fed so far.
    pos: usize,
    /// The start of the image, kept until the descriptor has been read out of
    /// it.
    head: [u8; HEAD_BYTES],
    /// How much of `head` is filled.
    head_len: usize,
    /// The running XOR over segment data.
    checksum: u8,
    /// Whether the image declared an appended digest.
    hash_appended: bool,
    /// Where the walk is.
    phase: Phase,
    /// Scratch for a header that arrived split across two pages.
    partial: [u8; HEADER_BYTES],
    /// How much of `partial` is filled.
    partial_len: usize,
}

impl Verifier {
    /// Start checking an image of `declared` bytes for `chip`, bound for a slot
    /// of `slot_bytes`.
    ///
    /// The size refusal happens here, before a byte has been read, because it
    /// is the one refusal `Content-Length` alone can settle.
    pub fn new(chip: Chip, declared: usize, slot_bytes: usize) -> Result<Verifier, ImageError> {
        if declared > slot_bytes {
            return Err(ImageError::TooLarge {
                bytes: declared,
                slot: slot_bytes,
            });
        }
        Ok(Verifier {
            chip,
            declared,
            pos: 0,
            head: [0; HEAD_BYTES],
            head_len: 0,
            checksum: CHECKSUM_SEED,
            hash_appended: false,
            phase: Phase::Header,
            partial: [0; HEADER_BYTES],
            partial_len: 0,
        })
    }

    /// How many bytes are still expected.
    pub const fn remaining(&self) -> usize {
        self.declared.saturating_sub(self.pos)
    }

    /// Feed the next slice of the image.
    ///
    /// Slices may be any length and need not align with anything — the walk
    /// keeps its own scratch for a header split across two of them. The first
    /// refusal is returned and the verifier must not be fed again.
    pub fn feed(&mut self, mut bytes: &[u8]) -> Result<(), ImageError> {
        if self.pos + bytes.len() > self.declared {
            return Err(ImageError::LengthMismatch {
                walked: self.pos + bytes.len(),
                declared: self.declared,
            });
        }

        // Kept before the walk consumes it, because the descriptor sits inside
        // the first segment's data and the walk has no reason to look at it.
        let take = bytes.len().min(HEAD_BYTES - self.head_len);
        self.head[self.head_len..self.head_len + take].copy_from_slice(&bytes[..take]);
        self.head_len += take;

        // **Before the walk, not after**, and the ordering is the whole reason
        // this is a separate function. The commonest wrong upload is an ELF —
        // `cargo build`'s output rather than `espflash save-image`'s — whose
        // second byte is `E`, 69, which the walk would read as a segment count
        // and refuse as [`ImageError::BadSegmentCount`]. That is true and
        // useless. Checking the magic first turns it into
        // [`ImageError::NotAnImage`], which names the byte and the mistake.
        //
        // It is safe to run first because the walk cannot have got past the
        // header on an earlier slice without these having fired on that slice:
        // both need the same twenty-four bytes.
        self.check_head()?;

        while !bytes.is_empty() {
            let consumed = self.step(bytes)?;
            if consumed == 0 {
                // The phase needs more than this slice carries; the scratch has
                // taken it.
                break;
            }
            bytes = &bytes[consumed..];
        }
        Ok(())
    }

    /// Advance one phase against `bytes`, returning how much it took.
    ///
    /// Zero means the phase needs more bytes than this slice has and has copied
    /// what there was into scratch.
    fn step(&mut self, bytes: &[u8]) -> Result<usize, ImageError> {
        match self.phase {
            Phase::Header => {
                let Some(taken) = self.fill_partial(bytes, HEADER_BYTES) else {
                    self.pos += bytes.len();
                    return Ok(0);
                };
                let header = Header::parse(&self.partial).ok_or(ImageError::Truncated {
                    missing: HEADER_BYTES,
                })?;
                self.hash_appended = header.hash_appended;
                if header.segment_count == 0 || header.segment_count > MAX_SEGMENTS {
                    return Err(ImageError::BadSegmentCount {
                        found: header.segment_count,
                    });
                }
                self.phase = Phase::SegmentHeader {
                    left: header.segment_count,
                };
                self.partial_len = 0;
                self.pos += taken;
                Ok(taken)
            }
            Phase::SegmentHeader { left } => {
                let Some(taken) = self.fill_partial(bytes, SEGMENT_HEADER_BYTES) else {
                    self.pos += bytes.len();
                    return Ok(0);
                };
                let length = u32::from_le_bytes([
                    self.partial[4],
                    self.partial[5],
                    self.partial[6],
                    self.partial[7],
                ]) as usize;
                // A length that cannot fit what was declared is a header this
                // is not, and letting it through would make the walk chase a
                // number out of a corrupt field for the rest of the upload.
                //
                // **`saturating_add`, and it is not decoration.** `length` comes
                // straight off the wire and can be `0xFFFF_FFFF`; `usize` is 32
                // bits on every target this ships to, so a plain `+` here
                // overflows — a panic under the dev profile's overflow checks,
                // which is a reboot an attacker can ask for, and a nonsense
                // number under release. **The host tests cannot reach it**,
                // because `usize` is 64 bits there and the same input simply
                // adds up, so this is a case where the guard has to carry itself
                // rather than being pinned by a test.
                if length > self.declared {
                    return Err(ImageError::LengthMismatch {
                        walked: self
                            .pos
                            .saturating_add(SEGMENT_HEADER_BYTES)
                            .saturating_add(length),
                        declared: self.declared,
                    });
                }
                self.partial_len = 0;
                self.pos += taken;
                // Same reason the zero-length padding is skipped below: a
                // declared segment length of zero would enter a phase that can
                // consume nothing, which the loop above reads as "out of
                // bytes". Real images have no empty segments; a corrupt header
                // can declare one, and it must not wedge the walk.
                self.phase = if length > 0 {
                    Phase::SegmentData {
                        remaining: length,
                        left: left - 1,
                    }
                } else {
                    self.after_segment(left - 1, self.pos)
                };
                Ok(taken)
            }
            Phase::SegmentData { remaining, left } => {
                let taken = bytes.len().min(remaining);
                for byte in &bytes[..taken] {
                    self.checksum ^= *byte;
                }
                let remaining = remaining - taken;
                self.phase = if remaining > 0 {
                    Phase::SegmentData { remaining, left }
                } else if left > 0 {
                    Phase::SegmentHeader { left }
                } else {
                    // The padding runs to the byte before the checksum, and the
                    // checksum is the last byte of a sixteen-byte block. It is
                    // legitimately **zero** long when the last segment happens
                    // to end one byte short of a block, so the phase has to be
                    // skipped rather than entered with nothing to do: a phase
                    // that consumes no bytes reads to the loop above as "this
                    // slice is exhausted", and the walk would stall a byte from
                    // the end of every image that landed that way.
                    self.after_segment(0, self.pos + taken)
                };
                self.pos += taken;
                Ok(taken)
            }
            Phase::Padding { remaining } => {
                let taken = bytes.len().min(remaining);
                let remaining = remaining - taken;
                self.phase = if remaining > 0 {
                    Phase::Padding { remaining }
                } else {
                    Phase::Checksum
                };
                self.pos += taken;
                Ok(taken)
            }
            Phase::Checksum => {
                let found = bytes[0];
                if found != self.checksum {
                    return Err(ImageError::BadChecksum {
                        computed: self.checksum,
                        found,
                    });
                }
                self.phase = if self.hash_appended {
                    Phase::Digest {
                        remaining: APPENDED_DIGEST_BYTES,
                    }
                } else {
                    Phase::Done
                };
                self.pos += 1;
                Ok(1)
            }
            Phase::Digest { remaining } => {
                let taken = bytes.len().min(remaining);
                let remaining = remaining - taken;
                self.phase = if remaining > 0 {
                    Phase::Digest { remaining }
                } else {
                    Phase::Done
                };
                self.pos += taken;
                Ok(taken)
            }
            Phase::Done => Err(ImageError::LengthMismatch {
                walked: self.pos + bytes.len(),
                declared: self.declared,
            }),
        }
    }

    /// What follows a segment that has just ended at `at`, with `left` segments
    /// still to come.
    fn after_segment(&self, left: u8, at: usize) -> Phase {
        if left > 0 {
            return Phase::SegmentHeader { left };
        }
        let padding = CHECKSUM_ALIGN - 1 - (at % CHECKSUM_ALIGN);
        if padding > 0 {
            Phase::Padding { remaining: padding }
        } else {
            Phase::Checksum
        }
    }

    /// Gather `want` bytes into the scratch, returning how many were taken from
    /// `bytes` once the scratch is full and `None` while it is not.
    fn fill_partial(&mut self, bytes: &[u8], want: usize) -> Option<usize> {
        let take = bytes.len().min(want - self.partial_len);
        self.partial[self.partial_len..self.partial_len + take].copy_from_slice(&bytes[..take]);
        self.partial_len += take;
        (self.partial_len == want).then_some(take)
    }

    /// The header and descriptor checks, run as soon as enough has arrived.
    fn check_head(&self) -> Result<(), ImageError> {
        if self.head_len >= HEADER_BYTES {
            if self.head[0] != IMAGE_MAGIC {
                return Err(ImageError::NotAnImage {
                    first: self.head[0],
                });
            }
            let header = Header::parse(&self.head).ok_or(ImageError::Truncated {
                missing: HEADER_BYTES,
            })?;
            if header.chip_id != self.chip.id() {
                return Err(ImageError::WrongChip {
                    found: header.chip_id,
                    expected: self.chip.id(),
                });
            }
        }
        if self.head_len >= HEAD_BYTES {
            let magic = u32::from_le_bytes([
                self.head[DESCRIPTOR_OFFSET],
                self.head[DESCRIPTOR_OFFSET + 1],
                self.head[DESCRIPTOR_OFFSET + 2],
                self.head[DESCRIPTOR_OFFSET + 3],
            ]);
            if magic != DESCRIPTOR_MAGIC {
                return Err(ImageError::NotAnApp { magic });
            }
        }
        Ok(())
    }

    /// Every byte has been fed. Check the totals.
    pub fn finish(self) -> Result<Accepted, ImageError> {
        if self.head_len < HEAD_BYTES {
            return Err(ImageError::Truncated {
                missing: HEAD_BYTES - self.head_len,
            });
        }
        if self.phase != Phase::Done {
            return Err(ImageError::Truncated {
                missing: self.declared - self.pos,
            });
        }
        if self.pos != self.declared {
            return Err(ImageError::LengthMismatch {
                walked: self.pos,
                declared: self.declared,
            });
        }
        Ok(Accepted {
            len: self.declared,
            chip: self.chip,
            version: field(&self.head, DESCRIPTOR_VERSION_AT),
            project: field(&self.head, DESCRIPTOR_PROJECT_AT),
        })
    }
}

/// One NUL-padded 32-byte descriptor field, as a string.
///
/// Anything that is not printable ASCII is dropped rather than replaced: the
/// only consumer is a console line and a `curl` response, and a descriptor
/// field carrying arbitrary bytes is a file this verifier has already decided
/// is an image, so the interesting question is what to *show* rather than
/// whether to refuse.
fn field(head: &[u8; HEAD_BYTES], at: usize) -> String<DESCRIPTOR_FIELD_BYTES> {
    let bytes = &head[DESCRIPTOR_OFFSET + at..DESCRIPTOR_OFFSET + at + DESCRIPTOR_FIELD_BYTES];
    let mut out = String::new();
    for byte in bytes {
        if *byte == 0 {
            break;
        }
        if byte.is_ascii_graphic() || *byte == b' ' {
            // Cannot fail: at most DESCRIPTOR_FIELD_BYTES pushes into a string
            // of exactly that capacity.
            let _ = out.push(*byte as char);
        }
    }
    out
}
