//! Holds `partitions.csv` to the layout the rest of the firmware is built on.
//!
//! The table is data, not code: nothing in it reaches the compiler, so an edit
//! that breaks it compiles, links, lints and passes the whole three-chip matrix
//! green. The first thing that would notice is a device — and for the invariant
//! that matters most here, the device would notice by having overwritten the
//! rolling codes of motors in daily use, which costs a physical re-pairing at
//! each shade. That is not a failure worth discovering on hardware, so the
//! table is checked where CI can see it.
//!
//! Parsing is `esp-idf-part`, the crate espflash itself uses, so what is
//! asserted below is what the flashing tool will read rather than a second
//! opinion about the same file. Its own `validate` covers the generic ESP-IDF
//! rules — app partitions on 64 KB boundaries, `data, ota` sized exactly
//! 0x2000, overlaps, duplicate names, the 0x9000 floor. Everything this file
//! adds is specific to this project.

use esp_idf_part::{AppType, DataType, Partition, PartitionTable, SubType, Type};

/// Where each pinned data region must stay, and what it costs to move it.
///
/// `rollcode` is the reason this build script exists. Its address has been
/// 0x200000 since the region existed, so that is where every already
/// provisioned board keeps its rolling codes; a table that moves it does not
/// migrate them, it abandons them, and the store cannot tell an abandoned
/// region from a fresh one well enough to matter — a motor rejects any code at
/// or below the last it accepted, and undoing that means re-pairing at the
/// shade. The other two are pinned for the weaker reason that moving them
/// costs a re-provisioning over a cable.
const PINNED: &[(&str, u32, u32, &str)] = &[
    (
        "rollcode",
        0x0020_0000,
        0x2000,
        "the rolling codes of motors in service live here; moving this region \
         abandons them and costs a physical re-pairing at every shade",
    ),
    (
        "wificfg",
        0x0020_2000,
        0x2000,
        "moving this region discards the stored Wi-Fi and broker settings and \
         costs a re-provisioning over a cable",
    ),
    (
        "shades",
        0x0020_4000,
        0x2000,
        "moving this region discards the stored shade table and costs a \
         re-provisioning over a cable",
    ),
];

/// The largest flash this table is allowed to assume.
///
/// It ends on exactly this boundary, which is deliberate and is the reason
/// `ota_1` is not simply extended into the spare half of the ESP32-S3's 8 MB:
/// only that one board is known to carry 8 MB, and the single-app table this
/// one replaces fit a 4 MB part. A layout that quietly needs 8 MB would be
/// found by an ESP32 or ESP32-C3 refusing to flash, long after the edit.
const MAX_FLASH: u64 = 4 * 1024 * 1024;

fn main() {
    println!("cargo:rerun-if-changed=partitions.csv");
    println!("cargo:rerun-if-changed=build.rs");

    let csv = std::fs::read_to_string("partitions.csv")
        .unwrap_or_else(|e| panic!("cannot read crates/firmware/partitions.csv: {e}"));
    let table = PartitionTable::try_from_str(&csv)
        .unwrap_or_else(|e| panic!("crates/firmware/partitions.csv does not parse: {e}"));
    table
        .validate()
        .unwrap_or_else(|e| panic!("crates/firmware/partitions.csv is not a valid table: {e}"));

    check_pinned(&table);
    check_ab_slots(&table);
    check_ota_data(&table);
    check_no_factory(&table);
    check_fits_four_megabytes(&table);
}

/// Every region that must not move is where it was.
fn check_pinned(table: &PartitionTable) {
    for (name, offset, size, cost) in PINNED {
        let Some(part) = table.find(name) else {
            fail(format_args!(
                "'{name}' is missing from partitions.csv.\n\
                 The firmware looks this region up by label and refuses to run without it, \
                 so a board flashed with this table would stop at boot.\n\
                 Why it matters: {cost}."
            ));
        };
        if part.offset() != *offset || part.size() != *size {
            fail(format_args!(
                "'{name}' has moved: partitions.csv puts it at {:#010X} for {} bytes, and it \
                 must be at {offset:#010X} for {size} bytes.\n\
                 Why it matters: {cost}.\n\
                 If this move is genuinely intended, it needs a migration in \
                 docs/hardware-checklist.md before this line is edited — not after.",
                part.offset(),
                part.size(),
            ));
        }
    }
}

/// Exactly two app slots, and the same size, because A/B is only safe when an
/// image that fits the slot it was written to also fits the other one.
fn check_ab_slots(table: &PartitionTable) {
    let slots: Vec<&Partition> = table
        .partitions()
        .iter()
        .filter(|p| matches!(p.subtype(), SubType::App(AppType::Ota_0 | AppType::Ota_1)))
        .collect();

    if slots.len() != 2 {
        fail(format_args!(
            "partitions.csv defines {} of the two OTA app slots 'ota_0' and 'ota_1'.\n\
             Over-the-air updates write the slot that is not running and then hand the \
             bootloader a choice, so there is nothing to choose between with fewer than two.",
            slots.len(),
        ));
    }

    if slots[0].size() != slots[1].size() {
        fail(format_args!(
            "the two OTA app slots are different sizes: '{}' is {} bytes and '{}' is {} bytes.\n\
             An image built to fill the larger slot flashes into it happily and then bricks the \
             device on the *next* update, when it is written to the smaller one — a failure that \
             arrives one release after the mistake and looks nothing like it.",
            slots[0].name(),
            slots[0].size(),
            slots[1].name(),
            slots[1].size(),
        ));
    }
}

/// The bootloader's record of which slot to start has to exist and be the right
/// shape. `esp_bootloader_esp_idf::ota::Ota::new` refuses any other size, and
/// `esp-idf-part` already enforces the 0x2000 — this is about presence.
fn check_ota_data(table: &PartitionTable) {
    if table
        .find_by_subtype(Type::Data, SubType::Data(DataType::Ota))
        .is_none()
    {
        fail(format_args!(
            "partitions.csv has no 'data, ota' partition.\n\
             That region is the bootloader's record of which app slot to start and which \
             images have been confirmed good. Without it the two slots below it are just two \
             copies of the firmware with no way to switch between them, and no rollback."
        ));
    }
}

/// No `factory`, because the bootloader prefers it over both OTA slots whenever
/// the OTA record is blank — which is what a freshly flashed board has.
fn check_no_factory(table: &PartitionTable) {
    if let Some(part) = table.find_by_subtype(Type::App, SubType::App(AppType::Factory)) {
        fail(format_args!(
            "partitions.csv still defines a factory app partition ('{}').\n\
             A blank OTA record selects 'factory' when one exists, so every board would boot \
             the factory image and an update written to a slot would simply never run — with \
             nothing reporting a fault, because nothing failed.",
            part.name(),
        ));
    }
}

/// The whole table still fits the smallest board this firmware targets.
fn check_fits_four_megabytes(table: &PartitionTable) {
    let end = table
        .partitions()
        .iter()
        .map(|p| p.offset() as u64 + p.size() as u64)
        .max()
        .unwrap_or(0);
    if end > MAX_FLASH {
        fail(format_args!(
            "partitions.csv runs to {end:#X}, past the {MAX_FLASH:#X} this table is allowed to \
             assume.\n\
             Only the ESP32-S3 board here is known to carry 8 MB, and espflash reports a table \
             that overruns the flash only when someone flashes a board that small — which is \
             the wrong moment to find out."
        ));
    }
}

/// A build-script failure that reads as prose rather than as a stack trace.
fn fail(message: std::fmt::Arguments<'_>) -> ! {
    panic!("\n\ncrates/firmware/partitions.csv: {message}\n\n");
}
