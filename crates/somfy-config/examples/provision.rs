//! Build the flash image that provisions a board's Wi-Fi credentials.
//!
//! ```text
//! cargo run -p somfy-config --example provision -- wificfg.bin
//! espflash erase-parts --port /dev/ttyUSB0 --partition-table crates/firmware/partitions.csv wificfg
//! espflash write-bin   --port /dev/ttyUSB0 0x202000 wificfg.bin
//! ```
//!
//! ## Why the credential is entered here rather than compiled in
//!
//! Because there is nowhere else it could go that is not worse. A constant in
//! source is a credential in git. An environment variable read by a build
//! script is a credential in a shell history and in a build cache. A command
//! line argument is a credential in a shell history and in `ps` output. This
//! reads both values from **standard input**, so the only places they exist
//! are the operator's terminal, the file this writes, and the board's flash.
//!
//! ## What it does not do
//!
//! It does not encrypt anything. The record it writes is readable with
//! `espflash read-flash`, and so is the file it produces — delete the file
//! once the board has it. See this crate's module docs for why an obfuscation
//! layer here would be worse than saying so.
//!
//! The record is written with sequence number 0, so it is the newest record in
//! an **erased** region and nothing else. Provisioning a board that already
//! holds credentials therefore needs the `erase-parts` step above; without it
//! the old record has a higher sequence number and stays newest.

use std::io::{self, BufRead, Write};

use somfy_config::{ConfigRecord, WifiCredentials};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "wificfg.bin".to_string());

    // Prompts on stderr so `... | xxd` and friends still work, and so the
    // prompt is never mistaken for part of the output.
    let ssid = read_line("SSID: ")?;
    let psk = read_line("passphrase (empty for an open network): ")?;

    // The same constructor the firmware uses, so a value this accepts is a
    // value the device will accept, and a typo is caught here rather than
    // three flashes later as a board that will not associate.
    //
    // Reported through `Display` rather than by returning the error: the
    // default `Termination` for `Result` prints `Debug`, and "psk is 5 bytes;
    // at least 8 are needed" is the sentence the operator can act on.
    let credentials = WifiCredentials::new(&ssid, &psk).inspect_err(|error| {
        eprintln!("refusing to write: {error}");
    })?;
    let record = ConfigRecord {
        seq: 0,
        wifi: Some(credentials),
    };

    std::fs::write(&path, record.encode())?;
    // Deliberately reports the length rather than the passphrase: this line
    // ends up in terminal scrollback.
    eprintln!(
        "wrote {} bytes to {path} for SSID '{ssid}' ({} passphrase)",
        somfy_config::CONFIG_RECORD_LEN,
        if psk.is_empty() {
            "open network, no".to_string()
        } else {
            format!("{}-character", psk.chars().count())
        },
    );
    eprintln!("delete {path} once the board has it — it is not encrypted");
    Ok(())
}

/// One line from standard input, without its newline.
///
/// No echo suppression: this is a build-host tool, and a terminal that hides
/// the passphrase would also hide a typo in it. The recovery from a typo is
/// another `erase-parts` and another `write-bin`, which is cheap; the recovery
/// from a board that silently will not associate is an afternoon.
fn read_line(prompt: &str) -> io::Result<String> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}
