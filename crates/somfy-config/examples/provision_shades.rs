//! Build the flash image that provisions a board's shades.
//!
//! ```text
//! cargo run -p somfy-config --example provision_shades -- shades.bin
//! espflash erase-parts --port /dev/ttyUSB0 --partition-table crates/firmware/partitions.csv shades
//! espflash write-bin   --port /dev/ttyUSB0 0x204000 shades.bin
//! ```
//!
//! A sibling of `provision`, not a part of it: the shade table lives in its own
//! flash region because it does not fit beside the Wi-Fi and MQTT settings —
//! that record is 512 bytes with room for four shades after its last field, and
//! the registry holds 32. Two regions means two files and two `write-bin`
//! steps, and it also means re-provisioning shades never risks the credentials
//! and vice versa.
//!
//! ## Everything is read from standard input
//!
//! For the reason `provision`'s docs give: a command-line argument is a value
//! in a shell history and in `ps` output. None of these are secrets, but the
//! rolling code is the one value in the whole system that must not be typed
//! twice by accident, and one tool that reads one way is easier to be careful
//! with than two that read two ways.
//!
//! ## The rolling code is the dangerous field
//!
//! A motor stores the last rolling code it accepted and **rejects anything at
//! or below it as a replay**. The value entered here is the *next-to-send*
//! code, and it is applied only when the controller's rolling-code store holds
//! nothing for that address — see `somfy_store::seed_if_absent`. Two rules
//! follow, and this tool states both at the prompt:
//!
//! - For a motor that has been driven by another controller, enter a value
//!   **above** the last code that controller sent. `somfy-migrate` recovers it
//!   from a backup file as `next_code`, already `+1`-corrected.
//! - For a motor you are going to pair fresh, any value will do: the pairing
//!   procedure teaches the motor whatever the transmitter sends.
//!
//! ## Order is identity
//!
//! Shade ids come from the order below — first entry is `ShadeId(0)`, and
//! Home Assistant's entity for it is `shade_0`. Appending is safe. Removing or
//! reordering renumbers everything after the change, which in Home Assistant
//! means new entities and retained orphans left under the old ones.

use std::io::{self, BufRead, Write};

use somfy_config::{ShadeError, ShadeRecord, StoredShade, SHADE_RECORD_LEN, SHADE_TABLE_CAPACITY};
use somfy_domain::{ShadeConfig, ShadeKind, TiltMode};
use somfy_rts::RollingCode;

/// Factory-default travel times, the same ones `ShadeConfig::new` applies.
/// Offered as defaults rather than demanded because a measured value is
/// something a person calibrates later, and 10 s is what a shade ships with.
const DEFAULT_TRAVEL_MS: u32 = 10_000;
const DEFAULT_TILT_MS: u32 = 7_000;

/// The shade kinds this firmware models, as a person types them.
const KINDS: [(&str, ShadeKind); 7] = [
    ("roller", ShadeKind::Roller),
    ("blind", ShadeKind::Blind),
    ("drapery-left", ShadeKind::DraperyLeft),
    ("awning", ShadeKind::Awning),
    ("shutter", ShadeKind::Shutter),
    ("drapery-right", ShadeKind::DraperyRight),
    ("drapery-center", ShadeKind::DraperyCenter),
];

/// The tilt modes, likewise. **None of them drives a tilt axis yet** — see
/// `ShadeConfig::tilt_mode`; the value is carried so a later firmware has it.
const TILT_MODES: [(&str, TiltMode); 5] = [
    ("none", TiltMode::None),
    ("tilt-motor", TiltMode::TiltMotor),
    ("integrated", TiltMode::Integrated),
    ("tilt-only", TiltMode::TiltOnly),
    ("euro", TiltMode::EuroMode),
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "shades.bin".to_string());

    let mut record = ShadeRecord {
        seq: 0,
        shades: heapless::Vec::new(),
    };

    eprintln!(
        "Shades are provisioned in order: the first is ShadeId(0) and Home Assistant \n\
         calls it 'shade_0'. Appending to an existing list is safe; reordering or \n\
         removing renames every entity after the change. Enter an empty name to finish.\n"
    );

    while record.shades.len() < SHADE_TABLE_CAPACITY {
        let index = record.shades.len();
        let name = read_line(&format!("[{index}] name (empty to finish): "))?;
        if name.is_empty() {
            break;
        }
        let shade = read_shade(&name, &record)?;
        // Infallible: the loop condition is the capacity.
        record.shades.push(shade).map_err(|_| "the table is full")?;
    }
    if record.shades.len() == SHADE_TABLE_CAPACITY {
        eprintln!("that is the {SHADE_TABLE_CAPACITY}-shade limit the registry holds");
    }

    std::fs::write(&path, record.encode())?;

    eprintln!("\nwrote {SHADE_RECORD_LEN} bytes to {path}:");
    if record.shades.is_empty() {
        eprintln!(
            "  no shades — a table an operator can mean, and not the same thing as an \
             erased region.\n  A board carrying it receives and decodes and can be \
             commanded to move nothing."
        );
    }
    for (index, shade) in record.shades.iter().enumerate() {
        eprintln!(
            "  ShadeId({index}) '{}' address {} ({:#08X}), {:?}/{:?}, up {} ms, down {} ms, \
             tilt {} ms, seed rolling code {}",
            shade.config.name,
            shade.config.address,
            shade.config.address,
            shade.config.kind,
            shade.config.tilt_mode,
            shade.config.up_time_ms,
            shade.config.down_time_ms,
            shade.config.tilt_time_ms,
            shade.initial_code.0,
        );
    }
    eprintln!(
        "\nthe seed is applied only where the board's rolling-code store has nothing for \n\
         that address; an address it already knows keeps the code it has."
    );
    Ok(())
}

/// One shade, or the first refusal that stops the whole file being written.
///
/// Reported through `Display` rather than by returning the error: the default
/// `Termination` for `Result` prints `Debug`, and "up_time_ms may not be zero"
/// is the sentence the operator can act on.
fn read_shade(
    name: &str,
    written: &ShadeRecord,
) -> Result<StoredShade, Box<dyn std::error::Error>> {
    let address = read_address(written)?;

    // Through `ShadeError::Domain` so the refusal is a sentence rather than a
    // `Debug` spelling of an enum variant.
    let mut config = ShadeConfig::new(name, address)
        .map_err(ShadeError::Domain)
        .inspect_err(|error| eprintln!("refusing to write: {error}"))?;
    config.kind = read_choice("kind", &KINDS, ShadeKind::Roller)?;
    config.tilt_mode = read_choice("tilt mode", &TILT_MODES, TiltMode::None)?;
    config.up_time_ms = read_millis("full travel up", DEFAULT_TRAVEL_MS)?;
    config.down_time_ms = read_millis("full travel down", DEFAULT_TRAVEL_MS)?;
    config.tilt_time_ms = read_millis("full tilt", DEFAULT_TILT_MS)?;

    let code = read_line(
        "  next rolling code to send — ABOVE the last code any other controller sent\n  \
         to this motor, or anything at all for a motor you will pair fresh: ",
    )?;
    let code: u16 = code.parse().inspect_err(|error| {
        eprintln!("refusing to write: {code:?} is not a rolling code (0..=65535): {error}");
    })?;

    // The same constructor the firmware decodes through, so a value this
    // accepts is a value the device will accept, and a typo is caught here
    // rather than as a shade that vanishes at the next boot.
    let shade = StoredShade::new(config, RollingCode(code))
        .inspect_err(|error| eprintln!("refusing to write: {error}"))?;
    Ok(shade)
}

/// A radio address, refused if it is already in the table.
///
/// The duplicate check is here as well as in `ShadeRecord::decode` because the
/// device's answer to a duplicate is to refuse the *whole* table, and finding
/// that out after a flash is three steps too late.
fn read_address(written: &ShadeRecord) -> Result<u32, Box<dyn std::error::Error>> {
    let entered = read_line("  radio address (decimal, or 0x-prefixed hex): ")?;
    let address = match entered
        .strip_prefix("0x")
        .or_else(|| entered.strip_prefix("0X"))
    {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => entered.parse(),
    }
    .inspect_err(|error| {
        eprintln!("refusing to write: {entered:?} is not an address: {error}");
    })?;

    if let Some(clash) = written
        .shades
        .iter()
        .position(|shade| shade.config.address == address)
    {
        Err(format!(
            "refusing to write: address {address} is already ShadeId({clash}) '{}'",
            written.shades[clash].config.name,
        ))?;
    }
    Ok(address)
}

/// One of a fixed set of names, or the default when the operator just pressed
/// return.
fn read_choice<T: Copy>(
    what: &str,
    choices: &[(&str, T)],
    default: T,
) -> Result<T, Box<dyn std::error::Error>> {
    let names: Vec<&str> = choices.iter().map(|(name, _)| *name).collect();
    let entered = read_line(&format!("  {what} [{}] ({}): ", names[0], names.join(", "),))?;
    if entered.is_empty() {
        return Ok(default);
    }
    match choices.iter().find(|(name, _)| *name == entered) {
        Some((_, value)) => Ok(*value),
        None => Err(format!(
            "refusing to write: {entered:?} is not one of {}",
            names.join(", "),
        ))?,
    }
}

/// A travel time in milliseconds, or the default.
fn read_millis(what: &str, default: u32) -> Result<u32, Box<dyn std::error::Error>> {
    let entered = read_line(&format!("  {what} time in ms [{default}]: "))?;
    if entered.is_empty() {
        return Ok(default);
    }
    let millis: u32 = entered.parse().inspect_err(|error| {
        eprintln!("refusing to write: {entered:?} is not a number of milliseconds: {error}");
    })?;
    Ok(millis)
}

/// One line from standard input, without its newline.
fn read_line(prompt: &str) -> io::Result<String> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}
