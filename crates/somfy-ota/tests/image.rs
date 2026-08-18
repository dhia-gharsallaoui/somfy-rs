//! The streaming image verifier, against images built the way real ones are.
//!
//! # Two kinds of fixture, on purpose
//!
//! [`build`] assembles an image the way `espflash save-image` does — header,
//! segment headers and data, zero padding to the sixteen-byte rule, the XOR
//! checksum, then a thirty-two byte tail standing in for the appended digest.
//! Everything about *behaviour* is tested against that, because a builder can
//! be asked for a truncated image, a wrong-chip image or a flipped bit, and a
//! real one cannot.
//!
//! [`REAL_S3_HEAD`] is the other half: the **first 112 bytes of an actual
//! ESP32-S3 image built from this repository**. Nothing in the builder can
//! prove the layout is right, because the builder was written from the same
//! reading of it — so one test feeds the real bytes and asserts the chip id,
//! the appended-digest flag, the descriptor magic and both descriptor strings
//! come out where this crate says they are. If any offset here were wrong by
//! four bytes, that test is the one that fails.

use somfy_ota::image::{
    Chip, Header, ImageError, Verifier, APPENDED_DIGEST_BYTES, CHECKSUM_ALIGN, CHECKSUM_SEED,
    DESCRIPTOR_MAGIC, DESCRIPTOR_OFFSET, HEADER_BYTES, IMAGE_MAGIC, MAX_SEGMENTS,
    SEGMENT_HEADER_BYTES,
};

/// The first 112 bytes of a real ESP32-S3 image, produced by
/// `espflash save-image --chip esp32s3` from this repository's firmware on
/// 2026-08-18. It carries no address, key or credential — a header, a segment
/// header, and the crate's own version and name.
const REAL_S3_HEAD: [u8; 112] = [
    0xe9, 0x05, 0x02, 0x20, 0x34, 0x87, 0x37, 0x40, 0xee, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00,
    0x00, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x20, 0x00, 0x00, 0x3c, 0xa8, 0x43, 0x04, 0x00,
    0x32, 0x54, 0xcd, 0xab, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x30, 0x2e, 0x31, 0x2e, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x66, 0x69, 0x72, 0x6d, 0x77, 0x61, 0x72, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// A slot big enough that size is never what a test is measuring.
const SLOT: usize = 0x1F_0000;

/// Assemble an image the way the flashing tool does.
///
/// `segments` gives each segment's data length. The first one is padded up to
/// at least 112 bytes and given a valid application descriptor, because that is
/// what a real first segment is.
fn build(chip: Chip, segments: &[usize], hash_appended: bool) -> Vec<u8> {
    let mut out = vec![
        IMAGE_MAGIC,
        segments.len() as u8,
        0x02, // spi_mode
        0x20, // spi_speed | spi_size
    ];
    out.extend_from_slice(&0x4037_8734u32.to_le_bytes()); // entry_addr
    out.push(0xEE); // wp_pin
    out.extend_from_slice(&[0, 0, 0]); // spi_pin_drv
    out.extend_from_slice(&chip.id().to_le_bytes());
    out.push(0); // min_chip_rev
    out.extend_from_slice(&0u16.to_le_bytes()); // min_chip_rev_full
    out.extend_from_slice(&99u16.to_le_bytes()); // max_chip_rev_full
    out.extend_from_slice(&[0, 0, 0, 0]); // reserved
    out.push(u8::from(hash_appended));
    assert_eq!(out.len(), HEADER_BYTES);

    let mut checksum = CHECKSUM_SEED;
    for (index, len) in segments.iter().copied().enumerate() {
        let len = if index == 0 { len.max(256) } else { len };
        out.extend_from_slice(&0x3C00_0020u32.to_le_bytes()); // load_addr
        out.extend_from_slice(&(len as u32).to_le_bytes());
        let mut data = vec![0u8; len];
        if index == 0 {
            data[..4].copy_from_slice(&DESCRIPTOR_MAGIC.to_le_bytes());
            data[16..21].copy_from_slice(b"0.1.0");
            data[48..56].copy_from_slice(b"firmware");
        } else {
            for (at, byte) in data.iter_mut().enumerate() {
                *byte = (at % 251) as u8;
            }
        }
        for byte in &data {
            checksum ^= *byte;
        }
        out.extend_from_slice(&data);
    }

    while (out.len() + 1) % CHECKSUM_ALIGN != 0 {
        out.push(0);
    }
    out.push(checksum);
    if hash_appended {
        out.extend_from_slice(&[0xAB; APPENDED_DIGEST_BYTES]);
    }
    out
}

/// Feed an image in slices of `chunk` bytes and report the outcome.
fn run(chip: Chip, image: &[u8], chunk: usize) -> Result<somfy_ota::image::Accepted, ImageError> {
    let mut verifier = Verifier::new(chip, image.len(), SLOT)?;
    for slice in image.chunks(chunk) {
        verifier.feed(slice)?;
    }
    verifier.finish()
}

#[test]
fn a_real_image_header_parses_to_the_offsets_this_crate_claims() {
    // Arrange: the first bytes of an image the flashing tool actually produced.
    let head = REAL_S3_HEAD;

    // Act
    let header = Header::parse(&head).expect("112 bytes is more than a header");
    let descriptor_magic = u32::from_le_bytes(
        head[DESCRIPTOR_OFFSET..DESCRIPTOR_OFFSET + 4]
            .try_into()
            .unwrap(),
    );

    // Assert
    assert_eq!(head[0], IMAGE_MAGIC);
    assert_eq!(header.segment_count, 5);
    assert_eq!(header.chip_id, Chip::Esp32S3.id());
    assert!(
        header.hash_appended,
        "espflash appends a SHA-256 to every image it builds"
    );
    assert_eq!(descriptor_magic, DESCRIPTOR_MAGIC);
    assert_eq!(DESCRIPTOR_OFFSET, HEADER_BYTES + SEGMENT_HEADER_BYTES);
}

#[test]
fn the_real_head_passes_both_up_front_checks() {
    // Arrange: the real bytes, against a declaration long enough that the
    // segment length they carry is not what is being tested.
    let mut verifier = Verifier::new(Chip::Esp32S3, SLOT, SLOT).unwrap();

    // Act: 112 bytes is exactly enough for the magic, the chip id and the
    // descriptor's magic word to all have been read.
    let outcome = verifier.feed(&REAL_S3_HEAD);

    // Assert
    assert_eq!(outcome, Ok(()), "a real image head was refused");
}

#[test]
fn the_descriptors_real_bytes_yield_their_version_and_project_name() {
    // Arrange: a synthetic image carrying the **real** descriptor, spliced in
    // whole from offset 32 to 112. Nothing about the builder can prove where
    // `version` and `project_name` live, because the builder was written from
    // the same reading of the format; these eighty bytes came off a device
    // image, so an offset wrong by four is a failure here.
    let mut image = build(Chip::Esp32S3, &[512], true);
    let descriptor = HEADER_BYTES + SEGMENT_HEADER_BYTES;
    image[descriptor..descriptor + 80].copy_from_slice(&REAL_S3_HEAD[DESCRIPTOR_OFFSET..112]);
    let checksum_at = image.len() - APPENDED_DIGEST_BYTES - 1;
    let mut checksum = CHECKSUM_SEED;
    for byte in &image[descriptor..checksum_at] {
        checksum ^= *byte;
    }
    image[checksum_at] = checksum;

    // Act
    let accepted = run(Chip::Esp32S3, &image, 64).expect("a real descriptor is a valid one");

    // Assert
    assert_eq!(accepted.version.as_str(), "0.1.0");
    assert_eq!(accepted.project.as_str(), "firmware");
}

#[test]
fn a_well_formed_image_is_accepted_whatever_the_slice_size() {
    // Arrange
    let image = build(Chip::Esp32S3, &[512, 4096, 33], true);

    // Act + Assert: 1 is the pathological case — every header arrives split.
    for chunk in [1usize, 3, 7, 16, 64, 112, 256, 1024, 4096, image.len()] {
        let accepted = run(Chip::Esp32S3, &image, chunk)
            .unwrap_or_else(|error| panic!("chunk {chunk} refused a good image: {error:?}"));
        assert_eq!(accepted.len, image.len());
        assert_eq!(accepted.chip, Chip::Esp32S3);
        assert_eq!(accepted.version.as_str(), "0.1.0");
        assert_eq!(accepted.project.as_str(), "firmware");
    }
}

#[test]
fn an_image_with_no_appended_digest_is_still_accepted() {
    // The flag is read, not assumed: an image built without one ends at its
    // checksum byte and the walk has to stop there rather than wait for
    // thirty-two bytes that are never coming.
    let image = build(Chip::Esp32S3, &[512], false);
    let accepted = run(Chip::Esp32S3, &image, 64).expect("a digestless image is well formed");
    assert_eq!(accepted.len, image.len());
}

#[test]
fn an_elf_is_refused_by_its_first_byte_rather_than_by_its_second() {
    // Arrange: what `cargo build` leaves in target/, which is the file an
    // operator reaches for first. Its second byte is `E`, 69 — a plausible
    // segment count to a walk that has not checked the magic.
    let mut elf = vec![0x7F, b'E', b'L', b'F'];
    elf.extend_from_slice(&[0u8; 512]);

    // Act
    let outcome = run(Chip::Esp32S3, &elf, 256);

    // Assert
    assert_eq!(outcome, Err(ImageError::NotAnImage { first: 0x7F }));
}

#[test]
fn the_wrong_chips_image_is_refused_and_says_which_two() {
    // Arrange: the ESP32 build, uploaded to an ESP32-S3. Both are this
    // project's own artefacts and they sit in adjacent directories.
    let image = build(Chip::Esp32, &[512], true);

    // Act
    let outcome = run(Chip::Esp32S3, &image, 256);

    // Assert
    assert_eq!(
        outcome,
        Err(ImageError::WrongChip {
            found: Chip::Esp32.id(),
            expected: Chip::Esp32S3.id(),
        })
    );
}

#[test]
fn every_chip_this_project_has_produced_images_for_has_a_distinct_id() {
    // The ids were read off real images; this is the property that makes them
    // usable as a check at all. `Chip::Esp32` is still here although the
    // firmware stopped building for it on 2026-08-18: images from before then
    // exist, so the id has to stay distinct for one to be refused by name.
    let ids = [Chip::Esp32.id(), Chip::Esp32S3.id(), Chip::Esp32C3.id()];
    assert_eq!(ids, [0x0000, 0x0009, 0x0005]);
    for (at, id) in ids.iter().enumerate() {
        for other in &ids[at + 1..] {
            assert_ne!(id, other);
        }
    }
}

#[test]
fn a_bootloader_image_passes_the_magic_and_is_refused_as_not_an_app() {
    // Arrange: a well-formed image whose first segment carries no application
    // descriptor. That is what a bootloader binary is, and it is a file sitting
    // in the same directory as the one that was wanted.
    let mut image = build(Chip::Esp32S3, &[512], true);
    let magic_at = DESCRIPTOR_OFFSET;
    image[magic_at..magic_at + 4].copy_from_slice(&0u32.to_le_bytes());
    // The checksum covers segment data, so blanking the descriptor moves it.
    let checksum_at = image.len() - APPENDED_DIGEST_BYTES - 1;
    let mut checksum = CHECKSUM_SEED;
    for byte in &image[HEADER_BYTES + SEGMENT_HEADER_BYTES..checksum_at] {
        checksum ^= *byte;
    }
    image[checksum_at] = checksum;

    // Act
    let outcome = run(Chip::Esp32S3, &image, 256);

    // Assert
    assert_eq!(outcome, Err(ImageError::NotAnApp { magic: 0 }));
}

#[test]
fn a_flipped_bit_anywhere_in_the_segments_fails_the_checksum() {
    // Arrange
    let good = build(Chip::Esp32S3, &[512, 1024], true);

    // Act + Assert: every segment-data byte, one at a time, is too slow for a
    // unit test; three placements across two segments is the shape of it.
    for at in [HEADER_BYTES + SEGMENT_HEADER_BYTES + 200, 400, 900] {
        let mut image = good.clone();
        let at = at.min(image.len() - APPENDED_DIGEST_BYTES - 2);
        image[at] ^= 0x01;
        let outcome = run(Chip::Esp32S3, &image, 256);
        assert!(
            matches!(outcome, Err(ImageError::BadChecksum { .. })),
            "a flipped bit at {at} was not caught: {outcome:?}",
        );
    }
}

#[test]
fn an_upload_that_stops_early_is_refused_rather_than_accepted_short() {
    // Arrange: the whole image is declared and only part of it arrives, which
    // is what a dropped connection looks like from this side.
    let image = build(Chip::Esp32S3, &[2048], true);
    let mut verifier = Verifier::new(Chip::Esp32S3, image.len(), SLOT).unwrap();

    // Act
    verifier.feed(&image[..1024]).expect("the head is fine");
    let outcome = verifier.finish();

    // Assert
    assert!(
        matches!(outcome, Err(ImageError::Truncated { .. })),
        "a short upload was accepted: {outcome:?}",
    );
}

#[test]
fn more_bytes_than_content_length_declared_are_refused() {
    // Arrange
    let image = build(Chip::Esp32S3, &[512], true);
    let mut verifier = Verifier::new(Chip::Esp32S3, image.len() - 16, SLOT).unwrap();

    // Act: feed the whole thing against a short declaration.
    let mut outcome = Ok(());
    for slice in image.chunks(64) {
        outcome = verifier.feed(slice);
        if outcome.is_err() {
            break;
        }
    }

    // Assert
    assert!(
        matches!(outcome, Err(ImageError::LengthMismatch { .. })),
        "an overrun was accepted: {outcome:?}",
    );
}

#[test]
fn an_image_whose_structure_ends_before_the_declared_length_is_refused() {
    // Arrange: a good image with sixteen bytes of junk stapled on. The walk
    // reaches `Done` and then finds more bytes, which is a different failure
    // from truncation and a different message.
    let mut image = build(Chip::Esp32S3, &[512], true);
    image.extend_from_slice(&[0xCC; 16]);

    // Act
    let outcome = run(Chip::Esp32S3, &image, 64);

    // Assert
    assert!(
        matches!(outcome, Err(ImageError::LengthMismatch { .. })),
        "trailing junk was accepted: {outcome:?}",
    );
}

#[test]
fn an_image_larger_than_the_slot_is_refused_before_a_byte_is_read() {
    // This is the one refusal Content-Length alone can settle, and settling it
    // there is what keeps a too-large image from erasing the target slot on its
    // way to being rejected.
    let outcome = Verifier::new(Chip::Esp32S3, SLOT + 1, SLOT);
    assert_eq!(
        outcome.err(),
        Some(ImageError::TooLarge {
            bytes: SLOT + 1,
            slot: SLOT,
        })
    );
    assert!(
        Verifier::new(Chip::Esp32S3, SLOT, SLOT).is_ok(),
        "exactly full fits"
    );
}

#[test]
fn a_segment_count_outside_the_format_is_refused() {
    // Arrange: a header that passed the magic and the chip, with a segment
    // count the format does not have. Both ends of the range.
    for count in [0u8, MAX_SEGMENTS + 1, 0xFF] {
        let mut image = build(Chip::Esp32S3, &[512], true);
        image[1] = count;

        // Act
        let outcome = run(Chip::Esp32S3, &image, 256);

        // Assert
        assert_eq!(
            outcome,
            Err(ImageError::BadSegmentCount { found: count }),
            "segment count {count} was not refused",
        );
    }
}

#[test]
fn a_segment_length_that_cannot_fit_the_declaration_is_refused_at_that_header() {
    // Arrange: a corrupt length field. Without this check the walk would spend
    // the rest of the upload counting down a number out of a damaged header,
    // and refuse at the end with a message about truncation instead of about
    // the field that was wrong.
    let mut image = build(Chip::Esp32S3, &[512], true);
    let length_at = HEADER_BYTES + 4;
    image[length_at..length_at + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());

    // Act
    let outcome = run(Chip::Esp32S3, &image, 256);

    // Assert
    assert!(
        matches!(outcome, Err(ImageError::LengthMismatch { .. })),
        "a corrupt segment length was walked: {outcome:?}",
    );
}

#[test]
fn a_last_segment_ending_one_byte_short_of_a_block_still_walks_to_the_end() {
    // The padding between the last segment and the checksum is legitimately
    // zero bytes long when the segment happens to end at `offset % 16 == 15`.
    // A phase that consumes nothing reads to the walk as "this slice is
    // exhausted", so this case used to stall a byte from the end. Search for
    // the length that produces it rather than asserting one, because the header
    // sizes in front of it are the crate's, not this test's.
    let mut found = false;
    for len in 256..320usize {
        let image = build(Chip::Esp32S3, &[len], true);
        let checksum_at = image.len() - APPENDED_DIGEST_BYTES - 1;
        let segment_end = HEADER_BYTES + SEGMENT_HEADER_BYTES + len;
        if segment_end != checksum_at {
            continue;
        }
        found = true;
        assert!(
            run(Chip::Esp32S3, &image, 64).is_ok(),
            "a zero-length padding run stalled the walk at segment length {len}",
        );
    }
    assert!(
        found,
        "no segment length in the search range produced zero padding"
    );
}
