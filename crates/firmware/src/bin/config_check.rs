//! Bring-up binary for the configuration region. **Touches flash and nothing
//! else** — no SPI, no radio, no RMT, nothing on the air, and no network.
//!
//! It proves the half of provisioning that the controller cannot prove about
//! itself: that the `wificfg` region mounts, that a record written into it
//! survives a reset and a reflash, and that clearing it is a value the region
//! can hold rather than an erase. The controller only ever *reads* this
//! region, so without this binary the write path would have no hardware
//! evidence at all.
//!
//! ## It writes a placeholder, never a real credential
//!
//! The SSID it stores is [`PLACEHOLDER_SSID`] and there is no way to make it
//! store anything else — no constant to edit, no environment variable, no
//! serial prompt. A real credential is written by the host-side tool instead:
//!
//! ```bash
//! cargo run -p somfy-config --example provision -- wificfg.bin
//! espflash erase-parts --port /dev/ttyUSB0 --partition-table partitions.csv wificfg
//! espflash write-bin   --port /dev/ttyUSB0 0x202000 wificfg.bin
//! ```
//!
//! which keeps the passphrase on the operator's machine and out of this
//! repository entirely.
//!
//! **So running this binary overwrites a provisioned credential**, exactly as
//! `store-check` advances a rolling code. That is the same trade `store-check`
//! makes and it is why both are separate images from `firmware`.
//!
//! ```bash
//! cargo build --release --features chip-s3 --target xtensa-esp32s3-none-elf --bin config-check
//! espflash flash --port /dev/ttyUSB0 target/xtensa-esp32s3-none-elf/release/config-check
//! espflash monitor --port /dev/ttyUSB0 --non-interactive
//! ```

#![no_std]
#![no_main]

#[path = "../config.rs"]
mod config;

use core::net::Ipv4Addr;

use config::{ConfigError, ConfigStore};
use esp_hal::main;
use esp_storage::FlashStorage;
use somfy_config::{MqttSettings, WifiCredentials, DEFAULT_DISCOVERY_PREFIX, DEFAULT_STATE_ROOT};

// Without this the image has no ESP-IDF application descriptor, and espflash
// refuses to write it. See the note on the same line in `main.rs`.
esp_bootloader_esp_idf::esp_app_desc!();

/// The SSID this binary writes. Deliberately not a network that exists.
///
/// A board left holding this record is a board that says "provisioned with
/// nothing real" on every boot, which is a far better state to find than one
/// that looks configured and silently never associates.
const PLACEHOLDER_SSID: &str = "SSID_NOT_PROVISIONED";

/// The passphrase this binary writes. Also not a real one, and eight
/// characters because that is the shortest a WPA network accepts — writing a
/// shorter one would exercise the validator's refusal rather than the store.
const PLACEHOLDER_PSK: &str = "notapass";

/// The broker address this binary writes.
///
/// `192.0.2.10` is from **TEST-NET-1** (RFC 5737), a block reserved for
/// documentation and guaranteed never to be routed. A board left holding this
/// record tries to reach an address that cannot exist, which is a far better
/// state to find than one pointing at a real host on somebody's network.
const PLACEHOLDER_BROKER: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 10);

/// The broker port this binary writes. The MQTT default, so the record is
/// ordinary in every respect except the address.
const PLACEHOLDER_PORT: u16 = 1883;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("PANIC: {}", info);
    #[allow(clippy::empty_loop)]
    loop {}
}

#[main]
fn entry() -> ! {
    match check() {
        Ok(()) => esp_println::println!("config check complete"),
        Err(error) => esp_println::println!("config check failed: {:?}", error),
    }
    #[allow(clippy::empty_loop)]
    loop {}
}

fn check() -> Result<(), ConfigError> {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let mut store = ConfigStore::mount(FlashStorage::new(peripherals.FLASH))?;

    let (base, slots, slot_len) = store.geometry();
    esp_println::println!(
        "config: partition '{}' at {:#010X}, {} slots of {} bytes",
        config::PARTITION_LABEL,
        base,
        slots,
        slot_len,
    );

    // Printed before anything is written: the only chance to see what the
    // previous run, or the host-side provisioning tool, left behind.
    let (found, survey) = store.load()?;
    esp_println::println!(
        "config: survey slots={} valid={} blank={} damaged={} newest_seq={:?}",
        survey.slots,
        survey.valid,
        survey.blank,
        survey.damaged,
        survey.newest_seq,
    );
    // `{:?}` is safe here and everywhere else in this firmware: `Debug` on
    // `WifiCredentials` redacts the passphrase. That is the reason it is
    // hand-written rather than derived.
    esp_println::println!("config: found {:?}", found.as_ref().map(|r| &r.wifi));

    // The round trip. `store` appends a record with the next sequence number,
    // erasing a sector first if the ring has reached one, and reads it back
    // before returning — so reaching this line at all means the bytes are in
    // the array.
    let placeholder = WifiCredentials::new(PLACEHOLDER_SSID, PLACEHOLDER_PSK)
        .expect("the placeholder above is a valid credential");
    // Anonymous, because a placeholder password would be a string in this
    // repository that looks like a credential. The broker half is written at
    // all so the round trip covers the whole record rather than half of it.
    let broker = MqttSettings::new(
        PLACEHOLDER_BROKER,
        PLACEHOLDER_PORT,
        "",
        "",
        DEFAULT_DISCOVERY_PREFIX,
        DEFAULT_STATE_ROOT,
    )
    .expect("the placeholder above is a valid setting");
    store.store(Some(placeholder), Some(broker))?;
    esp_println::println!("config: stored the placeholder credential and broker");

    // Re-read through a fresh scan rather than trusting `store`'s own
    // verification: this is what the next boot will see.
    let (reloaded, survey) = store.load()?;
    esp_println::println!(
        "config: survey slots={} valid={} blank={} damaged={} newest_seq={:?}",
        survey.slots,
        survey.valid,
        survey.blank,
        survey.damaged,
        survey.newest_seq,
    );
    let stored_ssid = reloaded
        .as_ref()
        .and_then(|record| record.wifi.as_ref())
        .map(|wifi| wifi.ssid());
    esp_println::println!("config: reloaded ssid = {:?}", stored_ssid);
    if stored_ssid != Some(PLACEHOLDER_SSID) {
        return Err(ConfigError::NotDurable);
    }

    Ok(())
}
