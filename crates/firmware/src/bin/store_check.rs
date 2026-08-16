//! Bring-up binary for the rolling-code store. **Touches flash and nothing
//! else** — no SPI, no radio, no RMT, nothing on the air.
//!
//! The store's whole claim is that a committed code survives losing power and
//! survives reflashing. Neither can be proved on the host, and neither can be
//! proved by the store reporting on itself, so this binary does the one thing
//! that settles it: it prints what is in flash, commits the next code, and
//! prints it back. Reset the board and the count continues; reflash and it
//! still continues. If it ever restarts from nothing, the store is broken and
//! the number on the serial line says so.
//!
//! ```bash
//! cargo build --release --features chip-s3 --target xtensa-esp32s3-none-elf --bin store-check
//! espflash flash --port /dev/ttyUSB0 target/xtensa-esp32s3-none-elf/release/store-check
//! espflash monitor --port /dev/ttyUSB0 --non-interactive
//! ```

#![no_std]
#![no_main]

#[path = "../store.rs"]
mod store;

use esp_hal::main;
use esp_storage::FlashStorage;
use somfy_rts::RollingCode;
use somfy_store::RollingCodeStore;
use store::{FlashStore, StoreError};

// Without this the image has no ESP-IDF application descriptor, and espflash
// refuses to write it. See the note on the same line in `main.rs`.
esp_bootloader_esp_idf::esp_app_desc!();

/// The synthetic address `main.rs` transmits at, reused here so nothing in this
/// repository ever writes a counter for a real paired shade by accident. This
/// binary does not key the radio at all, so the address is only ever a table
/// key.
const CHECK_ADDRESS: u32 = 0x00C0DE;

/// Where a brand-new region starts counting. Only ever used for an explicit
/// first `commit` below — the store itself never invents a value, which is the
/// whole point of `load` reporting `Ok(None)`.
const SEED_CODE: u16 = 1;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("PANIC: {}", info);
    #[allow(clippy::empty_loop)]
    loop {}
}

#[main]
fn entry() -> ! {
    match check() {
        Ok(()) => esp_println::println!("store check complete"),
        Err(error) => esp_println::println!("store check failed: {:?}", error),
    }
    #[allow(clippy::empty_loop)]
    loop {}
}

fn check() -> Result<(), StoreError> {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let mut store = FlashStore::mount(FlashStorage::new(peripherals.FLASH))?;

    let (base, slots, slot_len) = store.geometry();
    esp_println::println!(
        "store: partition '{}' at {:#010X}, {} slots of {} bytes",
        store::PARTITION_LABEL,
        base,
        slots,
        slot_len,
    );

    // Printed before anything is written, because it is the only chance to see
    // what the *previous* run left behind. `damaged` above zero after a clean
    // shutdown would mean flash trouble; after a deliberate power cut mid-write
    // it is exactly what should be there.
    let survey = store.survey()?;
    esp_println::println!(
        "store: survey slots={} valid={} blank={} damaged={} newest_seq={:?} addresses={}",
        survey.slots,
        survey.valid,
        survey.blank,
        survey.damaged,
        survey.newest_seq,
        survey.addresses,
    );

    let loaded = store.load(CHECK_ADDRESS)?;
    esp_println::println!("store: load({:#08X}) = {:?}", CHECK_ADDRESS, loaded);

    // A missing record is seeded here, in the caller, deliberately visibly —
    // never inside the store. See `RollingCodeStore`'s docs: a store that
    // answers with a starting value when it cannot find a record replays codes
    // the motor has already accepted.
    //
    // And seeding only when the region is *blank*. This binary exists to catch
    // a store that lost its codes; a harness that responds to any empty read by
    // cheerfully starting again at 1 would report success on precisely the
    // failure it was built to detect. `load` already refuses a region that is
    // damaged and holds no record, so this only has to cover the remaining
    // case: damage sitting alongside a readable record for some *other*
    // address, which is still worth stopping for on a device nobody power-cut.
    let next = match loaded {
        Some(code) => RollingCode(code.0.wrapping_add(1)),
        None if survey.damaged > 0 => {
            esp_println::println!(
                "store: NOT seeding — {} damaged slot(s) present, so an empty read \
                 may be lost data rather than a fresh region",
                survey.damaged,
            );
            return Err(StoreError::Unreadable {
                damaged: survey.damaged,
                slots: survey.slots,
            });
        }
        None => {
            esp_println::println!("store: region is blank — seeding");
            RollingCode(SEED_CODE)
        }
    };

    store.commit(CHECK_ADDRESS, next)?;
    esp_println::println!("store: commit({:#08X}, {}) ok", CHECK_ADDRESS, next.0);

    // Re-read through a fresh scan rather than trusting `commit`'s own
    // verification: this is the number to compare against the next boot's, and
    // it should be the number just committed.
    let reloaded = store.load(CHECK_ADDRESS)?;
    esp_println::println!("store: load({:#08X}) = {:?}", CHECK_ADDRESS, reloaded);
    if reloaded != Some(next) {
        return Err(StoreError::NotDurable);
    }

    Ok(())
}
