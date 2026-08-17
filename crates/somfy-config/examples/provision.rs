//! Build the flash image that provisions a board's Wi-Fi credentials and its
//! MQTT settings.
//!
//! ```text
//! cargo run -p somfy-config --example provision -- wificfg.bin
//! espflash erase-parts --port /dev/ttyUSB0 --partition-table crates/firmware/partitions.csv wificfg
//! espflash write-bin   --port /dev/ttyUSB0 0x202000 wificfg.bin
//! ```
//!
//! ## Why the secrets are entered here rather than compiled in
//!
//! Because there is nowhere else they could go that is not worse. A constant in
//! source is a credential in git. An environment variable read by a build
//! script is a credential in a shell history and in a build cache. A command
//! line argument is a credential in a shell history and in `ps` output. This
//! reads every value from **standard input**, so the only places they exist
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
//! holds a configuration therefore needs the `erase-parts` step above; without
//! it the old record has a higher sequence number and stays newest.
//!
//! ## Leaving either half out
//!
//! An empty SSID writes "no network configured"; an empty broker address writes
//! "no broker configured". Both are values the record can hold, and a board
//! carrying either still receives and decodes — the radio does not depend on
//! the network. See `crates/firmware/src/net.rs`.

use std::io::{self, BufRead, Write};
use std::net::Ipv4Addr;

use somfy_config::{
    ConfigRecord, MqttSettings, WifiCredentials, CONFIG_RECORD_LEN, DEFAULT_DISCOVERY_PREFIX,
    DEFAULT_STATE_ROOT,
};

/// The port every unencrypted MQTT broker uses unless it has been moved.
const DEFAULT_PORT: u16 = 1883;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "wificfg.bin".to_string());

    let wifi = read_wifi()?;
    let mqtt = read_mqtt()?;

    let record = ConfigRecord { seq: 0, wifi, mqtt };

    std::fs::write(&path, record.encode())?;
    // Deliberately reports lengths and never the secrets themselves: this line
    // ends up in terminal scrollback.
    eprintln!(
        "wrote {CONFIG_RECORD_LEN} bytes to {path} — {}, {}",
        match &record.wifi {
            Some(wifi) => format!("SSID '{}'", wifi.ssid()),
            None => "no network configured".to_string(),
        },
        match &record.mqtt {
            Some(mqtt) => format!(
                "broker {}:{} ({}), discovery_prefix '{}', state_root '{}'",
                mqtt.address(),
                mqtt.port(),
                if mqtt.is_anonymous() {
                    "anonymous"
                } else {
                    "authenticated"
                },
                mqtt.discovery_prefix(),
                mqtt.state_root(),
            ),
            None => "no broker configured".to_string(),
        },
    );
    eprintln!("delete {path} once the board has it — it is not encrypted");
    Ok(())
}

/// The Wi-Fi half, or `None` if the operator left the SSID empty.
///
/// Reported through `Display` rather than by returning the error: the default
/// `Termination` for `Result` prints `Debug`, and "psk is 5 bytes; at least 8
/// are needed" is the sentence the operator can act on.
fn read_wifi() -> Result<Option<WifiCredentials>, Box<dyn std::error::Error>> {
    let ssid = read_line("SSID (empty for no network): ")?;
    if ssid.is_empty() {
        return Ok(None);
    }
    let psk = read_line("passphrase (empty for an open network): ")?;
    // The same constructor the firmware uses, so a value this accepts is a
    // value the device will accept, and a typo is caught here rather than
    // three flashes later as a board that will not associate.
    let credentials = WifiCredentials::new(&ssid, &psk).inspect_err(|error| {
        eprintln!("refusing to write: {error}");
    })?;
    Ok(Some(credentials))
}

/// The MQTT half, or `None` if the operator left the broker address empty.
///
/// The address is an IPv4 address and not a host name because the firmware has
/// no resolver — see `somfy_config::MqttSettings`. Asking for one here is what
/// stops a name being typed, stored, and silently never connected to.
fn read_mqtt() -> Result<Option<MqttSettings>, Box<dyn std::error::Error>> {
    let address = read_line("broker IPv4 address (empty for no broker): ")?;
    if address.is_empty() {
        return Ok(None);
    }
    let address: Ipv4Addr = address.parse().inspect_err(|error| {
        eprintln!("refusing to write: {address:?} is not an IPv4 address: {error}");
    })?;

    let port = read_line(&format!("broker port [{DEFAULT_PORT}]: "))?;
    let port: u16 = if port.is_empty() {
        DEFAULT_PORT
    } else {
        port.parse().inspect_err(|error| {
            eprintln!("refusing to write: {port:?} is not a port number: {error}");
        })?
    };

    let username = read_line("broker username (empty for anonymous): ")?;
    let password = if username.is_empty() {
        String::new()
    } else {
        read_line("broker password: ")?
    };

    // Both default rather than being demanded. Home Assistant supports exactly
    // one discovery prefix and it is global to the installation, so an estate
    // that has not moved it should not be asked to type it.
    let discovery_prefix = read_or_default(
        &format!("discovery_prefix [{DEFAULT_DISCOVERY_PREFIX}]: "),
        DEFAULT_DISCOVERY_PREFIX,
    )?;
    let state_root = read_or_default(
        &format!("state_root [{DEFAULT_STATE_ROOT}]: "),
        DEFAULT_STATE_ROOT,
    )?;

    let settings = MqttSettings::new(
        address,
        port,
        &username,
        &password,
        &discovery_prefix,
        &state_root,
    )
    .inspect_err(|error| eprintln!("refusing to write: {error}"))?;
    Ok(Some(settings))
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

/// One line, or the default when the operator just pressed return.
fn read_or_default(prompt: &str, default: &str) -> io::Result<String> {
    let value = read_line(prompt)?;
    Ok(if value.is_empty() {
        default.to_string()
    } else {
        value
    })
}
