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
//!
//! ## Importing the broker settings from a backup
//!
//! ```text
//! cargo run -p somfy-config --example provision -- --from-backup device.backup wificfg.bin
//! ```
//!
//! The **broker half only**. Network credentials are deliberately not migrated
//! — design spec §3.4 — and the backup does not carry the broker's username or
//! password either, so both are still asked for. What it does carry is the
//! address, the port and the two topic namespaces, and the last of those is the
//! reason this path exists at all.
//!
//! ### The namespaces are not copied across; the concatenation is undone
//!
//! The controller being replaced stores two values, `rootTopic` and
//! `discoTopic`, and then **joins them at publish time**: every publish passes
//! through a helper that prepends `rootTopic`, including the discovery topic
//! built from `discoTopic`. A device with root `espsomfyrts` and disco
//! `homeassistant` therefore publishes its discovery configs to
//! `espsomfyrts/homeassistant/cover/1/config`, which is under no prefix Home
//! Assistant reads — and that single fault is why its discovery has never
//! worked in any configuration.
//!
//! So the import is a *mapping*, not a copy: `discoTopic` becomes
//! `discovery_prefix` and `rootTopic` becomes `state_root`, as two independent
//! namespaces linked only by the payload's `~`. See
//! `docs/specs/2026-08-15-mqtt-ha-discovery-requirements.md` R1.
//!
//! ### An import that cannot be made valid is refused, with the field named
//!
//! The old controller accepted combinations that cannot work here, and R3's
//! rule is that invalid configuration is refused at the point of entry rather
//! than stored and silently ineffective. Each of the three failure modes
//! observed on the deployed firmware becomes a refusal naming its field:
//! an empty `discoTopic` is an empty `discovery_prefix`; an empty `rootTopic`
//! is an empty `state_root`; and the two of them **both** set to
//! `homeassistant` — a natural way to fix the first — overlaps, which would
//! put this device's availability topic on Home Assistant's own birth and will
//! topic and mark it available while it is offline.

use std::ffi::OsString;
use std::io::{self, BufRead, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use somfy_config::{
    ConfigRecord, MqttSettings, WifiCredentials, CONFIG_RECORD_LEN, DEFAULT_DISCOVERY_PREFIX,
    DEFAULT_STATE_ROOT,
};
use somfy_migrate::{parse_backup, MigratedMqtt};

/// The port every unencrypted MQTT broker uses unless it has been moved.
const DEFAULT_PORT: u16 = 1883;

/// The URL scheme this firmware speaks, and the one the old controller
/// defaults to.
///
/// It has no TLS: the broker socket is a plain `embassy_net::tcp::TcpSocket`,
/// and there is no certificate store, no clock to check validity against at the
/// moment of the connection, and no room budgeted for mbedTLS. So `mqtts://`
/// is refused rather than downgraded — a downgrade would send the broker
/// password in the clear across somebody's network without saying so.
const PLAIN_SCHEME: &str = "mqtt://";

/// What the two paths are, spelled the way a person types them.
const USAGE: &str = "\
usage:
  provision [OUT]                          answer the prompts
  provision --from-backup FILE [OUT]       take the broker settings from a backup

OUT defaults to wificfg.bin. FILE is a backup exported from the controller being
replaced. Only the MQTT half is imported: Wi-Fi credentials are never migrated,
and the broker username and password are not in the file, so both are asked for.";

/// The extension a backup is exported with, and therefore the one thing the
/// output must never be named — `--from-backup *.backup` with more than one
/// match puts a real backup in the output slot.
const BACKUP_EXTENSION: &str = "backup";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (backup, path) = match parse_args(std::env::args_os().skip(1)) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            eprintln!("{USAGE}");
            return Ok(());
        }
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            return Err(error.into());
        }
    };

    let wifi = read_wifi()?;
    let mqtt = match &backup {
        Some(backup) => read_mqtt_from_backup(backup)?,
        None => read_mqtt()?,
    };

    let record = ConfigRecord { seq: 0, wifi, mqtt };

    std::fs::write(&path, record.encode())?;
    let path = path.display();
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

/// Parse the arguments after the program name: an optional backup, an optional
/// output path. `Ok(None)` means `--help` was asked for, which is not a failure
/// and must not exit like one.
///
/// Hand-written and small, for the reason `provision_shades`' own parser gives:
/// two forms and one flag, and a dependency whose job is to render `--help` is
/// one this does not need. `OsString` rather than `String` because both
/// arguments are filenames and `std::env::args()` *panics* on one that is not
/// UTF-8, in the only entry point this tool has.
type Parsed = Option<(Option<PathBuf>, PathBuf)>;

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Parsed, String> {
    const FLAG: &str = "--from-backup";
    const FLAG_EQ: &str = "--from-backup=";

    let mut backup: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        // Only the flag has to be text. A path that is not UTF-8 falls through
        // to the positional arm with its bytes intact.
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(None),
            Some(text) if text.starts_with(FLAG_EQ) => {
                set_once(&mut backup, text[FLAG_EQ.len()..].into(), FLAG)?
            }
            Some(FLAG) => {
                let path = args
                    .next()
                    .ok_or_else(|| format!("{FLAG} needs the path of a backup file"))?;
                set_once(&mut backup, path.into(), FLAG)?
            }
            Some(text) if text.starts_with('-') => {
                return Err(format!("{text:?} is not an option this tool has"))
            }
            _ => set_once(&mut out, arg.into(), "the output file")?,
        }
    }

    let out = out.unwrap_or_else(|| PathBuf::from("wificfg.bin"));
    if out.as_os_str().is_empty() {
        return Err("the output file was given an empty path".to_string());
    }
    if let Some(backup) = &backup {
        if backup.as_os_str().is_empty() {
            return Err(format!("{FLAG} was given an empty path"));
        }
        // The two ways to destroy the backup, both refused — the same pair
        // `provision_shades` refuses, and for the same reason: a backup carries
        // rolling codes and cannot be re-exported after the fact without them
        // having moved on.
        if backup == &out {
            return Err(format!(
                "{} is both the backup to read and the image to write",
                out.display()
            ));
        }
        if out.extension().is_some_and(|ext| ext == BACKUP_EXTENSION) {
            return Err(format!(
                "the image would be written to {}, which is named like a backup. Refusing, \
                 because `{FLAG} *.{BACKUP_EXTENSION}` with more than one match puts a real \
                 backup in the output slot",
                out.display(),
            ));
        }
    }
    Ok(Some((backup, out)))
}

/// Fill a slot that may only be filled once.
fn set_once(slot: &mut Option<PathBuf>, path: PathBuf, what: &str) -> Result<(), String> {
    match slot.replace(path) {
        Some(_) => Err(format!("{what} was given twice")),
        None => Ok(()),
    }
}

/// The MQTT half, taken from a backup rather than typed.
///
/// Everything except the credentials, which are not in the file. See this
/// module's docs for what the mapping does and why it is not a copy.
fn read_mqtt_from_backup(path: &Path) -> Result<Option<MqttSettings>, Box<dyn std::error::Error>> {
    let shown = path.display();
    let bytes = std::fs::read(path)
        .inspect_err(|error| eprintln!("refusing to write: cannot read {shown}: {error}"))?;
    // `MigrateError` is `no_std` and does not implement `std::error::Error`, so
    // the message is printed and a string stands in — the same shape the two
    // refusals below use.
    let data = parse_backup(&bytes).map_err(|error| {
        eprintln!("refusing to write: {shown} is not a readable backup ({error:?})");
        "unreadable backup"
    })?;

    let Some(migrated) = data.mqtt else {
        // Three causes, and the version tells them apart well enough to act on:
        // below 22 the file has no MQTT block at all, and at 22 or above this
        // means the net record was absent or unreadable. Either way the
        // settings are recoverable by hand, which is why this is not a refusal.
        eprintln!(
            "{shown} (format version {}) carried no broker settings — either the old \
             controller had none, or its network record could not be read. Enter them:",
            data.version,
        );
        return read_mqtt();
    };

    if migrated.hostname.is_empty() {
        eprintln!("{shown}: no broker was configured on the old controller.");
        return read_mqtt();
    }

    describe_import(&migrated, migrated.hostname.as_str());

    // The credentials, which the backup does not contain: the old controller
    // keeps the broker's username and password outside the file it exports.
    // Asked
    // for *after* the description above and before the mapping below, so a
    // person types a password only once the settings they are about to be
    // attached to are on screen.
    let username = read_line("broker username (empty for anonymous): ")?;
    let password = if username.is_empty() {
        String::new()
    } else {
        read_line("broker password: ")?
    };

    let settings = map_broker(&migrated, &username, &password)
        .inspect_err(|refusal| eprintln!("refusing to write: {refusal}"))?;
    Ok(Some(settings))
}

/// Why a backup's broker settings could not become this device's.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BrokerRefusal {
    /// A URL scheme this firmware does not speak.
    Scheme(String),
    /// A broker host that is a name rather than an address.
    NotAnAddress(String),
    /// The settings mapped and `MqttSettings::new` refused the result. The
    /// inner error names the field and the rule.
    Refused {
        /// What the old controller had in `discoTopic`.
        disco: String,
        /// What it had in `rootTopic`.
        root: String,
        /// Why the pair, or one of them, is not usable here.
        error: somfy_config::MqttSettingsError,
    },
}

impl std::fmt::Display for BrokerRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrokerRefusal::Scheme(scheme) => write!(
                f,
                "the old controller connected with {scheme:?} and this firmware speaks \
                 {PLAIN_SCHEME:?} only. There is no TLS on the broker socket, so importing \
                 this would send the broker password across the network in the clear \
                 without saying so"
            ),
            BrokerRefusal::NotAnAddress(host) => write!(
                f,
                "the old controller's broker is {host:?}, which is not an IPv4 address. \
                 This firmware has no resolver, so a name cannot be stored; look the \
                 address up and run this tool without --from-backup"
            ),
            BrokerRefusal::Refused { disco, root, error } => write!(
                f,
                "{error}.\n\
                 \x20 The old controller stored discoTopic {disco:?} and rootTopic \
                 {root:?}, which this firmware reads as discovery_prefix and state_root \
                 — two independent namespaces rather than one joined pair. Fix it on the \
                 old controller and export again, or run this tool without --from-backup \
                 and type them."
            ),
        }
    }
}

impl std::error::Error for BrokerRefusal {}

/// **The mapping, and the whole reason the import path exists.**
///
/// `discoTopic` becomes the discovery prefix **on its own** and `rootTopic`
/// becomes the state root **on its own**: the concatenation the old firmware
/// performs at publish time is undone rather than carried across. Everything
/// else here is a refusal, because the old controller accepted combinations
/// this one cannot make work and R3's rule is that those are refused at the
/// point of entry rather than stored and silently ineffective.
///
/// Pure, and separate from the prompts above for that reason: the three
/// combinations the deployed firmware could not escape are a table test rather
/// than a hardware session.
fn map_broker(
    migrated: &MigratedMqtt,
    username: &str,
    password: &str,
) -> Result<MqttSettings, BrokerRefusal> {
    // The scheme. Refused rather than downgraded — see `PLAIN_SCHEME`. An empty
    // field is what a record that never set one reads back as.
    let scheme = migrated.protocol.as_str();
    if !scheme.is_empty() && scheme != PLAIN_SCHEME {
        return Err(BrokerRefusal::Scheme(scheme.to_string()));
    }

    // The address. This firmware has no resolver — `embassy-net` is built
    // without its `dns` feature on the broker path — so a host *name* is a
    // value the record would accept and the network layer could do nothing
    // with. Refused here, where the person who can answer it is standing.
    let host = migrated.hostname.as_str();
    let address: Ipv4Addr = host
        .parse()
        .map_err(|_| BrokerRefusal::NotAnAddress(host.to_string()))?;

    MqttSettings::new(
        address,
        migrated.port,
        username,
        password,
        // The undo. Two arguments, two namespaces, in this order and never
        // joined.
        migrated.disco_topic.as_str(),
        migrated.root_topic.as_str(),
    )
    .map_err(|error| BrokerRefusal::Refused {
        disco: migrated.disco_topic.to_string(),
        root: migrated.root_topic.to_string(),
        error,
    })
}

/// Say what was found and what it will become, before anything is written.
///
/// The two namespaces are printed as a *transformation* rather than as values,
/// because that is what an operator has to check: the old controller's screen
/// shows two fields whose meanings are not the ones they will have here.
fn describe_import(migrated: &MigratedMqtt, host: &str) {
    eprintln!(
        "read the broker from the backup: {host}:{}\n\
         \x20 discoTopic {:?} becomes discovery_prefix {:?}\n\
         \x20 rootTopic {:?} becomes state_root {:?}\n\
         \x20 (the old controller joins the two at publish time; this does not)",
        migrated.port,
        migrated.disco_topic.as_str(),
        migrated.disco_topic.as_str(),
        migrated.root_topic.as_str(),
        migrated.root_topic.as_str(),
    );
    if migrated.disco_topic.as_str() != DEFAULT_DISCOVERY_PREFIX {
        eprintln!(
            "  !! the discovery prefix is not {DEFAULT_DISCOVERY_PREFIX:?}. Home Assistant \
             supports exactly one and it is global to the installation, so this only works \
             if the whole estate has been moved to it."
        );
    }
    if !migrated.publish_discovery {
        eprintln!(
            "  !! the old controller had discovery publishing switched off. This firmware \
             always publishes it, so entities will appear where that one showed none — \
             which is the point: its discovery could not be made to work."
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hstr<const N: usize>(text: &str) -> heapless::String<N> {
        heapless::String::try_from(text).expect("fixture string fits")
    }

    /// What the backup's net record hands over. `disco` and `root` are the two
    /// fields the old controller stores separately and joins at publish time.
    fn migrated(disco: &str, root: &str) -> MigratedMqtt {
        MigratedMqtt {
            protocol: hstr("mqtt://"),
            hostname: hstr("192.0.2.10"),
            port: 1883,
            publish_discovery: true,
            root_topic: hstr(root),
            disco_topic: hstr(disco),
        }
    }

    /// **The three combinations the deployed firmware could not escape**, from
    /// the evidence table in
    /// `docs/specs/2026-08-15-mqtt-ha-discovery-requirements.md`. Acceptance
    /// criterion 1 of that spec asks for exactly this: each input either
    /// produces a valid configuration here or is refused.
    ///
    /// The first is the one that migrates cleanly, and it is the whole point —
    /// what the old controller published to
    /// `espsomfyrts/homeassistant/cover/1/config` becomes two namespaces that
    /// Home Assistant can actually read.
    #[test]
    fn each_combination_the_old_firmware_could_not_escape_is_mapped_or_refused() {
        // root espsomfyrts, disco homeassistant — "ignored, not under HA's
        // prefix" there; two independent namespaces here.
        let mapped = map_broker(&migrated("homeassistant", "espsomfyrts"), "", "")
            .expect("the ordinary configuration migrates");
        assert_eq!(mapped.discovery_prefix(), "homeassistant");
        assert_eq!(mapped.state_root(), "espsomfyrts");

        // root homeassistant, disco empty — "ignored, empty component segment".
        assert!(matches!(
            map_broker(&migrated("", "homeassistant"), "", ""),
            Err(BrokerRefusal::Refused { .. })
        ));

        // root empty, disco homeassistant — "discovered, but entities
        // permanently unavailable".
        assert!(matches!(
            map_broker(&migrated("homeassistant", ""), "", ""),
            Err(BrokerRefusal::Refused { .. })
        ));
    }

    /// The fourth combination, and the reason the spec's R3 gained a
    /// cross-field rule: an operator fixing the empty-prefix failure above by
    /// setting `discoTopic = homeassistant` while leaving `rootTopic` alone
    /// gets two individually valid values whose *pair* puts this device's
    /// availability topic on Home Assistant's own birth and will topic — which
    /// marks it available while it is offline, and is worse than no
    /// availability at all.
    #[test]
    fn both_namespaces_set_to_homeassistant_is_refused_as_an_overlap() {
        let refusal = map_broker(&migrated("homeassistant", "homeassistant"), "", "")
            .expect_err("the pair overlaps");
        let BrokerRefusal::Refused { error, .. } = &refusal else {
            panic!("expected a validation refusal, got {refusal:?}");
        };
        // Named as the *state root*, because that is the one to move: the
        // discovery prefix is global to a whole Home Assistant installation.
        assert!(
            error.to_string().contains("state_root"),
            "the refusal must name the field to change: {error}"
        );
    }

    /// The guard against the bug this whole path exists to undo. If the mapping
    /// ever concatenated again, `state_root` would be the joined string and
    /// this would fail — which no round-trip test would notice, because both
    /// halves would still be present.
    #[test]
    fn the_two_namespaces_are_never_joined() {
        let mapped = map_broker(&migrated("homeassistant", "espsomfyrts"), "", "").expect("valid");
        for joined in [
            "espsomfyrts/homeassistant",
            "homeassistant/espsomfyrts",
            "/espsomfyrts",
        ] {
            assert_ne!(mapped.discovery_prefix(), joined);
            assert_ne!(mapped.state_root(), joined);
        }
    }

    /// A broker host is a *name* in the old controller — its field is 65 bytes
    /// of text and it resolves it. This firmware has no resolver on that path,
    /// so a name is refused rather than stored as something that will never
    /// connect.
    #[test]
    fn a_broker_host_that_is_a_name_is_refused() {
        let mut named = migrated("homeassistant", "espsomfyrts");
        named.hostname = hstr("broker.example");
        assert_eq!(
            map_broker(&named, "", ""),
            Err(BrokerRefusal::NotAnAddress("broker.example".to_string()))
        );
    }

    /// TLS is refused rather than downgraded: there is no certificate store and
    /// no clock to check validity against, and a silent downgrade would put the
    /// broker password on the network in the clear.
    #[test]
    fn a_tls_scheme_is_refused_and_a_missing_one_is_not() {
        let mut secure = migrated("homeassistant", "espsomfyrts");
        secure.protocol = hstr("mqtts://");
        assert_eq!(
            map_broker(&secure, "", ""),
            Err(BrokerRefusal::Scheme("mqtts://".to_string()))
        );

        // An empty field is what a record that never set one reads back as, and
        // the old controller defaults to the plain scheme, so it is not a
        // refusal.
        let mut unset = migrated("homeassistant", "espsomfyrts");
        unset.protocol = hstr("");
        assert!(map_broker(&unset, "", "").is_ok());
    }

    /// A namespace longer than this record's field is refused by length rather
    /// than truncated into a different namespace — the old controller's fields
    /// are 64 bytes and this record's are 32, so the gap is reachable.
    #[test]
    fn a_namespace_too_long_for_the_record_is_refused_rather_than_truncated() {
        let long = "a".repeat(33);
        let refusal = map_broker(&migrated(&long, "espsomfyrts"), "", "")
            .expect_err("33 bytes does not fit a 32-byte field");
        assert!(refusal.to_string().contains("discovery_prefix"));
    }

    /// The address, the port and the credentials come from three different
    /// places — the file, the file, and the operator — and all three have to
    /// reach the settings.
    #[test]
    fn the_broker_and_the_credentials_both_arrive() {
        let mut moved = migrated("homeassistant", "espsomfyrts");
        moved.hostname = hstr("198.51.100.7");
        moved.port = 8883;
        let mapped = map_broker(&moved, "somfy", "PLACEHOLDER_BROKER_PASSWORD").expect("valid");
        assert_eq!(mapped.address(), Ipv4Addr::new(198, 51, 100, 7));
        assert_eq!(mapped.port(), 8883);
        assert_eq!(mapped.username(), "somfy");
        assert!(!mapped.is_anonymous());
    }

    #[test]
    fn the_output_may_not_be_the_backup_and_may_not_be_named_like_one() {
        let parse = |args: &[&str]| parse_args(args.iter().map(OsString::from));
        assert!(parse(&["--from-backup", "d.backup", "d.backup"]).is_err());
        assert!(parse(&["--from-backup", "d.backup", "out.backup"]).is_err());
        assert!(parse(&["--from-backup", "d.backup", "out.bin"]).is_ok());
        // Without a backup there is nothing to overwrite, so the naming rule
        // does not apply.
        assert!(parse(&["out.backup"]).is_ok());
        // And the defaults.
        assert_eq!(
            parse(&[]).expect("no arguments is a valid form"),
            Some((None, PathBuf::from("wificfg.bin")))
        );
    }
}
