//! How wide an entity can get on the wire, measured rather than counted.
//!
//! # Why this is pinned beside the DTOs
//!
//! The firmware serialises one entity at a time into a **fixed buffer** in an
//! embedded connection task, and a buffer one byte short is not an error it can
//! report: `serde_json_core::to_slice` returns `Err`, and a caller that reads
//! that as "zero bytes written" answers `200 OK` with an empty body — or, worse,
//! emits a JSON array with a separator and no element between the commas.
//!
//! So the bound lives beside the DTO, where adding a field moves it, and it is
//! **measured**. A hand-counted figure was wrong by 160 bytes and would have let
//! one shade permanently break the list endpoint the operator needs in order to
//! delete it.
//!
//! # What makes the worst case worse than it looks
//!
//! The name is a `heapless::String<32>` and JSON escapes a control character as
//! `\u00XX` — six bytes for one. Nothing in `ShadeConfig::new` or
//! `CreateShadeDto::to_config` forbids a control character in a name, and
//! `picoserve`'s inbound unescape buffer is exactly 32 bytes, which is enough to
//! deliver thirty-two of them. So thirty-two is the bound, not a hypothetical:
//! a client really can store such a name, and it is written to flash.

use heapless::Vec;
use somfy_api::{GroupDto, RoomDto, ShadeDto, SHADE_JSON_MAX_BYTES};
use somfy_domain::{PlannedTx, Pos, Shade, ShadeCommand, ShadeConfig, ShadeId};

/// Thirty-two characters that each cost six bytes escaped.
fn worst_name() -> heapless::String<32> {
    let mut name = heapless::String::new();
    for _ in 0..32 {
        name.push('\u{1}').expect("thirty-two of thirty-two");
    }
    name
}

#[test]
fn a_shade_never_serialises_wider_than_the_declared_bound() {
    let mut config = ShadeConfig::new("x", 0xFF_FFFE).expect("a legal address");
    config.name = worst_name();
    config.up_time_ms = u32::MAX;
    config.down_time_ms = u32::MAX;
    config.tilt_time_ms = u32::MAX;

    // Moving, with a favourite set: `myPosition` present rather than `null`,
    // and `direction` at its widest (`-1`).
    let mut shade = Shade::new(config);
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    shade.handle(
        ShadeCommand::SetMy(Some(Pos::from_percent(100))),
        0,
        &mut out,
    );
    shade.handle(ShadeCommand::Down, 0, &mut out);
    shade.tick(1_000, &mut out);
    shade.handle(ShadeCommand::Up, 1_000, &mut out);

    let json = serde_json::to_string(&ShadeDto::from_shade(ShadeId(31), &shade))
        .expect("a DTO always serialises");
    assert!(
        json.len() <= SHADE_JSON_MAX_BYTES,
        "the widest shade serialises to {} bytes, over SHADE_JSON_MAX_BYTES of {}",
        json.len(),
        SHADE_JSON_MAX_BYTES,
    );
    // And the bound is not absurdly loose. A figure far above the real worst
    // case is a buffer paid for on every connection task in the firmware, out
    // of the DRAM its Wi-Fi driver's heap is carved from.
    assert!(
        json.len() + 128 >= SHADE_JSON_MAX_BYTES,
        "SHADE_JSON_MAX_BYTES ({SHADE_JSON_MAX_BYTES}) is more than 128 bytes above \
         the measured worst case ({})",
        json.len(),
    );
}

/// The same buffer serialises groups and rooms, so the bound has to cover them
/// too — a full membership list of thirty-two ids beside the same name.
#[test]
fn a_group_and_a_room_fit_the_same_bound() {
    let shade_ids: heapless::Vec<u8, 32> = (0..32u8).collect();
    let group = GroupDto {
        id: 15,
        name: worst_name(),
        shade_ids: shade_ids.clone(),
    };
    let room = RoomDto {
        id: 15,
        name: worst_name(),
        shade_ids,
    };

    for (what, json) in [
        ("group", serde_json::to_string(&group).unwrap()),
        ("room", serde_json::to_string(&room).unwrap()),
    ] {
        assert!(
            json.len() <= SHADE_JSON_MAX_BYTES,
            "the widest {what} serialises to {} bytes, over {SHADE_JSON_MAX_BYTES}",
            json.len(),
        );
    }
}

/// An ordinary shade is a little over half the bound, which is worth stating so
/// the figure is not read as a typical cost — and pinned, so that a DTO growing
/// a field shows up here as well as at the ceiling.
#[test]
fn an_ordinary_shade_is_a_little_over_half_the_bound() {
    let shade = Shade::new(ShadeConfig::new("Salon / Porte-fenêtre", 0x80_1234).unwrap());
    let json = serde_json::to_string(&ShadeDto::from_shade(ShadeId(3), &shade)).unwrap();
    assert_eq!(
        json.len(),
        443,
        "an ordinary shade's width moved; the ceiling above may have moved too",
    );
    assert!(json.len() < SHADE_JSON_MAX_BYTES);
}
