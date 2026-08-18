//! Build the flash images that provision a board's shades and the estate
//! around them.
//!
//! ```text
//! # answer the prompts, one shade at a time
//! cargo run -p somfy-config --example provision_shades -- shades.bin
//!
//! # or read them out of the controller you are replacing
//! cargo run -p somfy-config --example provision_shades -- --from-backup device.backup shades.bin
//!
//! espflash erase-parts --port /dev/ttyUSB0 --partition-table crates/firmware/partitions.csv shades estate
//! espflash write-bin   --port /dev/ttyUSB0 0x204000 shades.bin
//! espflash write-bin   --port /dev/ttyUSB0 0x208000 estate.bin
//! ```
//!
//! A sibling of `provision`, not a part of it: the shade table lives in its own
//! flash region because it does not fit beside the Wi-Fi and MQTT settings —
//! that record is 512 bytes with room for four shades after its last field, and
//! the registry holds 32. Separate regions mean separate files and separate
//! `write-bin` steps, and they also mean re-provisioning shades never risks the
//! credentials and vice versa.
//!
//! ## Two images, always, and they are flashed together
//!
//! `shades.bin` is the table; `estate.bin` is the rooms, which room each shade
//! is in, and the groups. **Both are written on every run**, including the
//! interactive one, where the estate is empty.
//!
//! That is not a formality. A group's membership and a room assignment are
//! stored as **rows of the shade table**, so an estate written beside a
//! *different* table names the wrong shades — and a table replaced without its
//! estate leaves the old one pointing at whatever now occupies those rows.
//! Writing both from one import is what makes "these two files are the
//! installation" true, and it is why `--estate` exists as a path rather than as
//! an opt-in.
//!
//! What is still **not** here: the network and the broker. Wi-Fi credentials are
//! never migrated — the operator re-enters them — and the broker settings are a
//! third region and a different tool, `provision --from-backup`.
//!
//! ## Every value is read from standard input, and the two paths are two files
//!
//! For the reason `provision`'s docs give: a command-line argument is a value
//! in a shell history and in `ps` output. None of these are secrets, but the
//! rolling code is the one value in the whole system that must not be typed
//! twice by accident, and one tool that reads one way is easier to be careful
//! with than two that read two ways. Both **paths** on the command line are
//! filenames rather than values — the file `--from-backup` names holds real
//! addresses and rolling codes, so treat it as one and do not publish it.
//!
//! ## The rolling code is the dangerous field
//!
//! A motor stores the last rolling code it accepted and **rejects anything at
//! or below it as a replay**. The value carried here is the *next-to-send*
//! code, and it is applied only when the controller's rolling-code store holds
//! nothing for that address — see `somfy_store::seed_if_absent`. Enter one too
//! low and the motor ignores every frame, which looks exactly like a broken
//! transmitter and is fixed only by walking to the shade and pairing it again.
//!
//! Which is the argument for `--from-backup`: the controller being replaced
//! already knows that number, and its exported backup carries it. Everything
//! about the import path — what it refuses, and what it makes a person confirm
//! — follows from that one field, and lives in [`import`].
//!
//! At the prompts, two rules follow instead, and this tool states both:
//!
//! - For a motor that has been driven by another controller, enter a value
//!   **above** the last code that controller sent.
//! - For a motor you are going to pair fresh, any value will do: the pairing
//!   procedure teaches the motor whatever the transmitter sends.
//!
//! ## Order is identity
//!
//! Shade ids come from the order below — first entry is `ShadeId(0)`, and
//! Home Assistant's entity for it is `shade_0`. Appending is safe. Removing or
//! reordering renumbers everything after the change, which in Home Assistant
//! means new entities and retained orphans left under the old ones. An import
//! takes its order from the backup, and not from the ids the old controller
//! happened to assign.
//!
//! **This is a limitation of the record, not of the registry.** The registry
//! can now be told an id — `somfy_domain::Registry::add_shade_with_id` — but
//! this format stores none and the firmware that reads it still places shades
//! by position, so nothing here can offer an id column yet. It would be worse
//! than the rule above if it did: a reorder this tool accepted and the board
//! silently undid. See `somfy_config::shade`'s module docs for the order the
//! two halves have to land in.

mod import;

use std::ffi::OsString;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use somfy_config::{
    Announced, EstateRecord, ShadeError, ShadeRecord, StoredShade, ESTATE_RECORD_LEN,
    SHADE_RECORD_LEN, SHADE_TABLE_CAPACITY,
};
use somfy_domain::{PairingState, RemoteIdentity, ShadeConfig, ShadeId, ShadeKind, TiltMode};
use somfy_rts::RollingCode;

/// Whether a provisioned shade starts out needing to be paired, decided by the
/// one thing that actually answers the question: **where its address came
/// from.**
///
/// An address this controller's allocator produced is one **no motor has ever
/// heard**, so the shade will not move until somebody stands at it with a
/// working remote — it is awaiting confirmation, and the device offers to walk
/// them through it. An address that came from anywhere else — a backup, or a
/// number the operator read off the controller being replaced — is one a motor
/// already obeys, so the setup was completed on that other controller and there
/// is nothing here to finish.
///
/// The alternative was asking. It was rejected because the honest form of the
/// question is "has a motor been taught this address?", the tool has just
/// finished telling the operator the answer, and a prompt whose right answer is
/// already on screen is a prompt people get wrong.
///
/// **The error direction, since both are reachable**: called wrongly
/// `AwaitingConfirmation`, an imported shade appears under "finish setting up"
/// and one press of *it already works* clears it. Called wrongly
/// `ConfirmedByOperator`, a freshly allocated shade is announced to Home
/// Assistant and silently obeys nothing, which is the failure this whole flow
/// exists to end. So the test is the one that cannot get the second case wrong.
fn provisioned_pairing_state(address: u32) -> PairingState {
    if RemoteIdentity::is_allocated(address) {
        PairingState::AwaitingConfirmation
    } else {
        PairingState::ConfirmedByOperator
    }
}

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

/// What the two paths are, spelled the way a person types them.
const USAGE: &str = "\
usage:
  provision_shades [OUT] [--estate EST]                    answer the prompts, one shade at a time
  provision_shades --from-backup FILE [OUT] [--estate EST] read them from an exported backup

OUT defaults to shades.bin and EST to estate.bin. TWO images are always written,
one per flash region, and they must be flashed together: a group's membership and
a room assignment are rows of the shade table, so an estate written beside a
different table points at the wrong shades.

FILE is a backup exported from the controller being replaced; it carries real
radio addresses and rolling codes, so treat it as a key.";

/// The extension a backup is exported with, and therefore the one thing the
/// output must never be named. See [`Args::parse`].
const BACKUP_EXTENSION: &str = "backup";

/// Where the shades come from and where the images go.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    /// A backup to import, or `None` to ask.
    backup: Option<PathBuf>,
    /// The shade-region image to write.
    out: PathBuf,
    /// The estate-region image to write — rooms, room assignments and groups.
    ///
    /// Always written, including by the interactive path, where it is empty.
    /// That is not a formality: this tool replaces a whole shade table, and an
    /// estate left over from a previous import would still be naming rows by
    /// position. Writing an empty one is what makes "these two files are the
    /// installation" true.
    estate: PathBuf,
}

/// What [`Args::parse`] decided: run, or print the usage and stop without it
/// being an error.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Parsed {
    /// Arguments to run with.
    Run(Args),
    /// `--help`. Not a failure, so it must not exit like one.
    Usage,
}

impl Args {
    /// Parse the arguments after the program name.
    ///
    /// Deliberately tiny and hand-written: two forms, one flag, and a
    /// dependency whose job is to render `--help` is a dependency this does not
    /// need. `OsString` rather than `String` because both arguments are
    /// filenames, filenames are not required to be UTF-8, and
    /// `std::env::args()` *panics* on one that is not — in the only entry point
    /// this tool has.
    ///
    /// ## Two ways to destroy the backup, both refused here
    ///
    /// The backup is the only recoverable source of the rolling codes in it,
    /// and it must be exported fresh to be worth anything — so if the image is
    /// written over it, the recovery is pairing every shade by hand. Two
    /// spellings of that reach this function:
    ///
    /// - the same path given twice, which is the typo;
    /// - `--from-backup *.backup` with more than one match, which the shell
    ///   expands to `--from-backup a.backup b.backup` — so `b.backup` lands in
    ///   the output slot and is truncated to 2,048 bytes. Path equality does
    ///   **not** catch this one, which is why the output may not be named like
    ///   a backup at all.
    ///
    /// Unknown options are refused rather than treated as filenames for a third
    /// version of the same hazard: a mistyped `--from-backups` falling through
    /// to the output slot would overwrite the file it was meant to read.
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Parsed, String> {
        const FLAG: &str = "--from-backup";
        const FLAG_EQ: &str = "--from-backup=";
        const ESTATE: &str = "--estate";
        const ESTATE_EQ: &str = "--estate=";

        let mut backup: Option<PathBuf> = None;
        let mut out: Option<PathBuf> = None;
        let mut estate: Option<PathBuf> = None;
        let mut args = args.into_iter();

        while let Some(arg) = args.next() {
            // Only the flags have to be text. A path that is not UTF-8 falls
            // through to the positional arm with its bytes intact.
            match arg.to_str() {
                Some("--help" | "-h") => return Ok(Parsed::Usage),
                Some(text) if text.starts_with(FLAG_EQ) => {
                    set_once(&mut backup, text[FLAG_EQ.len()..].into(), FLAG)?;
                }
                Some(FLAG) => {
                    let path = args
                        .next()
                        .ok_or_else(|| format!("{FLAG} needs the path of a backup file"))?;
                    set_once(&mut backup, path.into(), FLAG)?;
                }
                Some(text) if text.starts_with(ESTATE_EQ) => {
                    set_once(&mut estate, text[ESTATE_EQ.len()..].into(), ESTATE)?;
                }
                Some(ESTATE) => {
                    let path = args
                        .next()
                        .ok_or_else(|| format!("{ESTATE} needs the path of an image to write"))?;
                    set_once(&mut estate, path.into(), ESTATE)?;
                }
                Some(text) if text.starts_with('-') => {
                    return Err(format!("{text:?} is not an option this tool has"));
                }
                _ => set_once(&mut out, arg.into(), "the output file")?,
            }
        }

        for (path, what) in [
            (backup.as_ref(), FLAG),
            (out.as_ref(), "the output file"),
            (estate.as_ref(), ESTATE),
        ] {
            if path.is_some_and(|path| path.as_os_str().is_empty()) {
                return Err(format!("{what} was given an empty path"));
            }
        }
        let out = out.unwrap_or_else(|| PathBuf::from("shades.bin"));
        let estate = estate.unwrap_or_else(|| PathBuf::from("estate.bin"));

        if out == estate {
            return Err(format!(
                "{} is named as both images; they are two flash regions and the second write \
                 would replace the first",
                out.display(),
            ));
        }

        // Both images are held to both rules the single one used to be: an
        // image must not overwrite the backup it was read from, and must not be
        // *named* like a backup, since `--from-backup *.backup` with two
        // matches puts a real one in an output slot.
        for image in [&out, &estate] {
            if backup.as_ref() == Some(image) {
                return Err(format!(
                    "{} is both the backup to read and an image to write; the image would \
                     overwrite the backup, and a backup cannot be re-exported after the fact \
                     without the rolling codes having moved on",
                    image.display(),
                ));
            }
            if backup.is_some() && image.extension().is_some_and(|ext| ext == BACKUP_EXTENSION) {
                return Err(format!(
                    "an image would be written to {}, which is named like a backup. Refusing, \
                     because `--from-backup *.{BACKUP_EXTENSION}` with more than one match puts \
                     a real backup in the output slot — and that backup is the only copy of the \
                     rolling codes it holds",
                    image.display(),
                ));
            }
        }

        Ok(Parsed::Run(Args {
            backup,
            out,
            estate,
        }))
    }
}

/// Fill a slot that may only be filled once.
///
/// A second output file is refused, so a second `--from-backup` is too: silently
/// letting the last one win is how a command that names two different backups
/// imports one of them and says nothing about the other.
fn set_once(slot: &mut Option<PathBuf>, path: PathBuf, what: &str) -> Result<(), String> {
    match slot.replace(path) {
        Some(_) => Err(format!("{what} was given twice")),
        None => Ok(()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match Args::parse(std::env::args_os().skip(1))
        .inspect_err(|error| eprintln!("{error}\n\n{USAGE}"))?
    {
        Parsed::Run(args) => args,
        // Asking for the usage is not a failure and must not exit like one.
        Parsed::Usage => {
            eprintln!("{USAGE}");
            return Ok(());
        }
    };

    let (record, estate) = match &args.backup {
        Some(path) => from_backup(path)?,
        None => (from_prompts()?, EstateRecord::empty(0)),
    };

    std::fs::write(&args.out, record.encode())?;
    eprintln!("\nwrote {SHADE_RECORD_LEN} bytes to {}", args.out.display());
    std::fs::write(&args.estate, estate.encode())?;
    eprintln!(
        "wrote {ESTATE_RECORD_LEN} bytes to {} — {}",
        args.estate.display(),
        describe_estate(&estate),
    );
    // The import path has already shown the table — it has to, because that is
    // what a confirmation is a confirmation *of*. Printing it twice would make
    // the copy above look like a different table from the one just written.
    if args.backup.is_none() {
        describe(&record);
    }
    eprintln!(
        "\nflash both, and together — the estate names shades by their row in the table:\n\
         \x20 espflash erase-parts --partition-table crates/firmware/partitions.csv shades estate\n\
         \x20 espflash write-bin 0x204000 {}\n\
         \x20 espflash write-bin 0x208000 {}",
        args.out.display(),
        args.estate.display(),
    );
    eprintln!(
        "\nthe seed is applied only where the board's rolling-code store has nothing for \n\
         that address; an address it already knows keeps the code it has."
    );
    Ok(())
}

/// One line saying what an estate image holds, for the write confirmation.
fn describe_estate(estate: &EstateRecord) -> String {
    if estate.is_empty() {
        return "no rooms and no groups, which is what the prompts produce".to_string();
    }
    let assigned = estate.room_of.iter().flatten().count();
    format!(
        "{} room(s), {} shade(s) assigned to one, {} group(s)",
        estate.rooms.len(),
        assigned,
        estate.groups.len(),
    )
}

/// A shade table and the estate around it, read out of an exported backup.
///
/// Nothing is written until this returns, which is what lets the misalignment
/// case ask before it acts rather than apologise afterwards.
fn from_backup(path: &Path) -> Result<(ShadeRecord, EstateRecord), Box<dyn std::error::Error>> {
    let shown = path.display();
    let bytes = std::fs::read(path)
        .inspect_err(|error| eprintln!("refusing to write: cannot read {shown}: {error}"))?;

    let imported = import::read_backup(&bytes)
        .inspect_err(|refusal| eprintln!("refusing to write: {refusal}"))?;

    eprintln!(
        "read {} shade{} from {shown} (backup format version {}).",
        imported.shades.len(),
        if imported.shades.len() == 1 { "" } else { "s" },
        imported.version,
    );
    // What came across into the second image, and what still could not.
    let assigned = imported.estate.room_of.iter().flatten().count();
    for (count, one, many) in [
        (imported.estate.rooms.len(), "room", "rooms"),
        (assigned, "shade put in a room", "shades put in rooms"),
        (imported.estate.groups.len(), "group", "groups"),
    ] {
        if count > 0 {
            let what = if count == 1 { one } else { many };
            eprintln!("  {count} {what} written to the estate image.");
        }
    }
    if imported.favourites > 0 {
        let plural = if imported.favourites == 1 { "" } else { "s" };
        eprintln!(
            "  {} 'my' favourite{plural} not written — there is no field to provision one \
             into; the motors keep theirs, and this controller will not know it until you \
             set it again.",
            imported.favourites,
        );
    }

    // The half of a linked remote that *is* recoverable, and the half that is
    // not, said in one line because the distinction is the whole point:
    // recognising a wall remote's frames needs its address, which the backup
    // carries; transmitting *as* one would need its rolling code, which the old
    // controller kept outside the backup — and this controller never does that.
    if !imported.links.is_empty() {
        eprintln!(
            "  {} linked remote(s) written. A wall remote's presses are the only thing that \
             can correct a shade's position estimate, and only its address is needed to \
             hear them — the rolling codes are not in the backup and are not wanted.",
            imported.links.len(),
        );
    }

    let misaligned = imported.misaligned().then_some(imported.skipped_resyncs);
    let record = ShadeRecord {
        seq: 0,
        announced: Announced::NONE,
        links: imported.links,
        shades: imported.shades,
    };
    let estate = imported.estate;
    eprintln!();
    describe(&record);

    // After the table, not before it: the whole point is that these are the
    // last lines under the shade they are about, rather than a block that
    // scrolls away above thirty entries. A shade imported as something it is
    // not will be driven wrongly and say nothing about it.
    if !imported.warnings.is_empty() {
        eprintln!(
            "\n{} value(s) could not be carried across as they stand:",
            imported.warnings.len()
        );
        for warning in &imported.warnings {
            eprintln!(
                "  !! {} '{}': {}",
                warning.subject, warning.name, warning.caveat,
            );
        }
    }

    if let Some(records) = misaligned {
        confirm_misaligned(records)?;
    }
    Ok((record, estate))
}

/// Make the operator look at a table that may be subtly wrong, and say so.
///
/// This is the case the whole import path is careful for. A record whose fields
/// did not align produces values that are *plausible* — a name with a comma in
/// it shifts every field after it, so the rolling code above may be some other
/// field's number. Nothing about that is visible in the output, which is why
/// the only safe handling is to stop and make a person compare it.
fn confirm_misaligned(records: u16) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!(
        "\n  !!!! {records} record(s) in this backup did not align.\n\n\
         \x20 Fields were left over where the reader expected the record to end — an \n\
         \x20 unescaped comma inside a name shifts every field after it. So a value above \n\
         \x20 may be plausible and still wrong, and that includes a rolling code, which is \n\
         \x20 the one field that cannot be corrected afterwards: a motor rejects any code \n\
         \x20 at or below the last it accepted, and recovering means pairing by hand at the \n\
         \x20 shade.\n\n\
         \x20 Check every line above against the old controller before continuing.\n"
    );
    let answer = read_line("  type 'yes' to write them anyway: ")?;
    if answer != "yes" {
        eprintln!("nothing was written.");
        Err("aborted")?;
    }
    Ok(())
}

/// A shade table entered a field at a time.
fn from_prompts() -> Result<ShadeRecord, Box<dyn std::error::Error>> {
    let mut record = ShadeRecord {
        seq: 0,
        announced: Announced::NONE,
        links: heapless::Vec::new(),
        shades: heapless::Vec::new(),
    };

    eprintln!(
        "Shades are provisioned in order: the first is ShadeId(0) and Home Assistant \n\
         calls it 'shade_0'. Appending to an existing list is safe; reordering or \n\
         removing renames every entity after the change. Enter an empty name to finish.\n\
         \n\
         If you are replacing another controller, --from-backup reads all of this \n\
         out of its exported backup, rolling codes included.\n"
    );

    let identity = read_identity()?;

    while record.shades.len() < SHADE_TABLE_CAPACITY {
        let index = record.shades.len();
        let name = read_line(&format!("[{index}] name (empty to finish): "))?;
        if name.is_empty() {
            break;
        }
        let shade = read_shade(&name, &record, identity, index)?;
        // Infallible: the loop condition is the capacity.
        record.shades.push(shade).map_err(|_| "the table is full")?;
    }
    if record.shades.len() == SHADE_TABLE_CAPACITY {
        eprintln!("that is the {SHADE_TABLE_CAPACITY}-shade limit the registry holds");
    }
    Ok(record)
}

/// The controller's own virtual-remote identity, so each shade can be offered
/// an address nobody else allocates.
///
/// ## Why this is asked at all
///
/// Because the alternative is what happened: a table imported from another
/// controller carries **that controller's** remote addresses, and a board
/// flashed with it transmits as a remote the other controller is still
/// transmitting as. Each keeps its own rolling-code counter, neither knows what
/// the other has sent, and the first to fall behind starts sending codes the
/// motor has already accepted and rejects as replays. The motor stops answering
/// it, and stays that way until somebody re-pairs at the shade.
///
/// An address from here belongs to this board and to no other, so pairing a
/// motor to it ends that. What it costs is a pairing procedure per shade —
/// `docs/hardware-checklist.md` has the sequence.
///
/// Optional, and empty is a legitimate answer: importing a table to run
/// alongside the controller it came from is exactly what somebody does while
/// migrating, and typing addresses by hand is what somebody does when the
/// motors were paired to remotes that already exist.
fn read_identity() -> Result<Option<RemoteIdentity>, Box<dyn std::error::Error>> {
    let entered = read_line(
        "controller MAC, to allocate radio addresses from — the board prints it at boot \n\
         as 'pairing: this controller's remote addresses start at ...' (empty to enter \n\
         each address by hand): ",
    )?;
    if entered.is_empty() {
        return Ok(None);
    }
    let mac = parse_mac(&entered).inspect_err(|error| {
        eprintln!("refusing to write: {error}");
    })?;
    let identity = RemoteIdentity::from_mac(mac);
    eprintln!(
        "  addresses will be allocated from {:#08X} — check that against the board's \n\
         \x20 boot line before flashing. Two boards printing the same value is a bug.\n",
        identity.base(),
    );
    Ok(Some(identity))
}

/// Six bytes from `aa:bb:cc:dd:ee:ff`, `aa-bb-cc-dd-ee-ff`, or `aabbccddeeff`.
///
/// Refuses anything else rather than guessing. A MAC read wrongly produces a
/// perfectly plausible address that simply is not this board's, and the symptom
/// — a motor that ignores the controller — looks exactly like a broken
/// transmitter.
fn parse_mac(text: &str) -> Result<[u8; 6], String> {
    let digits: String = text
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.' | ' '))
        .collect();
    if digits.len() != 12 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "{text:?} is not a MAC address — six hex bytes are needed, \
             as aa:bb:cc:dd:ee:ff or aabbccddeeff"
        ));
    }
    let mut mac = [0u8; 6];
    for (index, byte) in mac.iter_mut().enumerate() {
        // Infallible: the length and the character class were both just
        // checked, so every two-character window is a hex byte.
        *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16)
            .map_err(|error| format!("{text:?} is not a MAC address: {error}"))?;
    }
    Ok(mac)
}

/// Every shade in the table, one line each — the only view a person gets of
/// what is about to be, or has just been, written.
fn describe(record: &ShadeRecord) {
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
        // Said out loud per shade, because it decides whether the shade appears
        // in Home Assistant at all. A silent "no entity" is exactly the thing
        // this flow exists to stop being a surprise.
        if !shade.config.pairing_state.is_confirmed() {
            eprintln!(
                "    ^ this address is one this controller invented, so no motor knows it \
                 yet. The shade will not be announced to Home Assistant until it has been \
                 paired and the web UI's setup flow confirms it moves."
            );
        }
    }
}

/// One shade, or the first refusal that stops the whole file being written.
///
/// Reported through `Display` rather than by returning the error: the default
/// `Termination` for `Result` prints `Debug`, and "up_time_ms may not be zero"
/// is the sentence the operator can act on.
fn read_shade(
    name: &str,
    written: &ShadeRecord,
    identity: Option<RemoteIdentity>,
    index: usize,
) -> Result<StoredShade, Box<dyn std::error::Error>> {
    let address = read_address(written, suggest_address(identity, written, index))?;

    // Through `ShadeError::Domain` so the refusal is a sentence rather than a
    // `Debug` spelling of an enum variant.
    let mut config = ShadeConfig::new(name, address)
        .map_err(ShadeError::Domain)
        .inspect_err(|error| eprintln!("refusing to write: {error}"))?;
    config.pairing_state = provisioned_pairing_state(address);
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

/// The address this controller would allocate to the shade at `index`, if a
/// controller identity was given.
///
/// The rows already written are what "already taken" means: a table part
/// imported and part entered can hold another controller's addresses, and
/// allocating over one of them would recreate the very collision the identity
/// exists to end. `RemoteIdentity::address_for` steps past them.
///
/// `None` when no identity was given, and — in principle — when every candidate
/// is taken, which the registry's own capacity makes unreachable. Either way the
/// operator is asked for an address instead of being handed a wrong one.
fn suggest_address(
    identity: Option<RemoteIdentity>,
    written: &ShadeRecord,
    index: usize,
) -> Option<u32> {
    let id = ShadeId(u8::try_from(index).ok()?);
    identity?.address_for(id, |address| {
        written
            .shades
            .iter()
            .any(|shade| shade.config.address == address)
    })
}

/// A radio address, refused if it is already in the table.
///
/// The duplicate check is here as well as in `ShadeRecord::decode` because the
/// device's answer to a duplicate is to refuse the *whole* table, and finding
/// that out after a flash is three steps too late.
///
/// `suggested` is the address this controller would allocate, offered as the
/// value an empty line takes. It is a default rather than the only option
/// because a motor already paired to a physical remote has an address of its
/// own, and this tool must be able to record that.
fn read_address(
    written: &ShadeRecord,
    suggested: Option<u32>,
) -> Result<u32, Box<dyn std::error::Error>> {
    let prompt = match suggested {
        Some(address) => format!("  radio address [{address:#08X}, this controller's own]: "),
        None => "  radio address (decimal, or 0x-prefixed hex): ".to_string(),
    };
    let entered = read_line(&prompt)?;
    if entered.is_empty() {
        if let Some(address) = suggested {
            return Ok(address);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        Args::parse(args.iter().map(OsString::from))
    }

    fn run(backup: Option<&str>, out: &str) -> Result<Parsed, String> {
        Ok(Parsed::Run(Args {
            backup: backup.map(PathBuf::from),
            out: PathBuf::from(out),
            estate: PathBuf::from("estate.bin"),
        }))
    }

    #[test]
    fn no_arguments_means_prompts_and_the_default_file() {
        assert_eq!(parse(&[]), run(None, "shades.bin"));
    }

    #[test]
    fn a_bare_path_is_still_the_output_file() {
        assert_eq!(parse(&["out.bin"]), run(None, "out.bin"));
    }

    #[test]
    fn a_backup_can_be_named_with_a_space_or_an_equals() {
        for form in [
            vec!["--from-backup", "device.backup"],
            vec!["--from-backup=device.backup"],
        ] {
            assert_eq!(
                parse(&form),
                run(Some("device.backup"), "shades.bin"),
                "{form:?}",
            );
        }
    }

    #[test]
    fn the_output_file_can_be_given_on_either_side_of_the_flag() {
        let expected = run(Some("device.backup"), "out.bin");
        assert_eq!(
            parse(&["--from-backup", "device.backup", "out.bin"]),
            expected
        );
        assert_eq!(
            parse(&["out.bin", "--from-backup", "device.backup"]),
            expected
        );
    }

    #[test]
    fn a_backup_flag_with_nothing_after_it_is_refused() {
        assert!(parse(&["--from-backup"]).is_err());
    }

    /// Asking for the usage is not a failure and must not exit like one.
    #[test]
    fn help_asks_for_the_usage_rather_than_failing() {
        assert_eq!(parse(&["--help"]), Ok(Parsed::Usage));
        assert_eq!(parse(&["-h"]), Ok(Parsed::Usage));
    }

    /// The reason unknown options are refused rather than taken as filenames:
    /// a mistyped flag falling through to the output slot would overwrite the
    /// file it was meant to read.
    #[test]
    fn an_option_this_tool_does_not_have_is_refused() {
        assert!(parse(&["--from-backups", "device.backup"]).is_err());
        assert!(parse(&["-f", "device.backup"]).is_err());
    }

    /// Two of anything is a command whose author meant two different things,
    /// and picking one silently is how the other goes unmentioned.
    #[test]
    fn giving_either_path_twice_is_refused() {
        assert!(parse(&["one.bin", "two.bin"]).is_err());
        assert!(parse(&["--from-backup", "a.backup", "--from-backup", "b.backup"]).is_err());
        assert!(parse(&["--from-backup=a.backup", "--from-backup=b.backup"]).is_err());
    }

    #[test]
    fn an_empty_path_is_refused_rather_than_carried_to_the_open() {
        assert!(parse(&["--from-backup="]).is_err());
        assert!(parse(&[""]).is_err());
    }

    /// The backup is the only copy of the codes in it, so writing the image
    /// over it is refused rather than done — by name, and by the shape of a
    /// name, since a glob that matched twice puts a *different* backup in the
    /// output slot and path equality would wave it through.
    #[test]
    fn writing_the_image_over_a_backup_is_refused() {
        assert!(parse(&["--from-backup", "device.backup", "device.backup"]).is_err());
        assert!(parse(&["--from-backup=device.backup", "device.backup"]).is_err());
        // `--from-backup *.backup`, expanded by the shell to two matches.
        assert!(parse(&["--from-backup", "a.backup", "b.backup"]).is_err());
        // A file merely named after one is fine — it is the extension that is
        // refused, and only while a backup is being read.
        assert!(parse(&["--from-backup", "a.backup", "backup.bin"]).is_ok());
        assert!(parse(&["shades.backup"]).is_ok());
    }

    /// A filename is a bag of bytes on this platform, and `std::env::args`
    /// panics on one that is not UTF-8. Both slots have to take it.
    #[test]
    fn a_path_that_is_not_utf8_is_a_path_like_any_other() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;

            let odd = OsString::from_vec(vec![b'o', 0xFF, b'.', b'b', b'i', b'n']);
            let parsed = Args::parse([OsString::from("--from-backup=d.backup"), odd.clone()])
                .expect("a non-UTF-8 filename is still a filename");
            assert_eq!(
                parsed,
                Parsed::Run(Args {
                    backup: Some(PathBuf::from("d.backup")),
                    out: PathBuf::from(odd),
                    estate: PathBuf::from("estate.bin"),
                })
            );
        }
    }

    /// Both forms have to appear where a person will look for them.
    #[test]
    fn the_usage_text_names_both_paths() {
        assert!(USAGE.contains("--from-backup"));
        assert!(USAGE.contains("shades.bin"));
    }

    // -----------------------------------------------------------------------
    // The controller identity, and the addresses allocated from it
    // -----------------------------------------------------------------------

    /// The three spellings a person actually types, all reaching the same six
    /// bytes.
    #[test]
    fn a_mac_is_accepted_in_the_three_forms_people_write_it_in() {
        let expected = [0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03];
        for form in ["aa:bb:cc:01:02:03", "AA-BB-CC-01-02-03", "aabbcc010203"] {
            assert_eq!(parse_mac(form), Ok(expected), "{form}");
        }
    }

    /// Anything else is refused rather than guessed at. A MAC read wrongly
    /// produces a plausible address that is not this board's, and the symptom is
    /// a motor that ignores the controller.
    #[test]
    fn a_malformed_mac_is_refused() {
        for form in [
            "",
            "aa:bb:cc:01:02",
            "aa:bb:cc:01:02:03:04",
            "zz:bb:cc:01:02:03",
        ] {
            assert!(parse_mac(form).is_err(), "{form:?} was accepted");
        }
    }

    fn record_with(addresses: &[u32]) -> ShadeRecord {
        let mut record = ShadeRecord {
            seq: 0,
            announced: Announced::NONE,
            links: heapless::Vec::new(),
            shades: heapless::Vec::new(),
        };
        for (index, address) in addresses.iter().enumerate() {
            let config = ShadeConfig::new(&format!("S{index}"), *address).unwrap();
            record
                .shades
                .push(StoredShade::new(config, RollingCode(1)).unwrap())
                .unwrap();
        }
        record
    }

    /// With no identity there is nothing to suggest, and the operator is asked
    /// for an address rather than handed one.
    #[test]
    fn without_an_identity_nothing_is_suggested() {
        assert_eq!(suggest_address(None, &record_with(&[]), 0), None);
    }

    /// Consecutive shades get consecutive addresses in an empty table.
    #[test]
    fn consecutive_shades_get_consecutive_addresses() {
        let identity = RemoteIdentity::from_mac([0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03]);
        let empty = record_with(&[]);
        let first = suggest_address(Some(identity), &empty, 0).unwrap();
        assert_eq!(suggest_address(Some(identity), &empty, 1), Some(first + 1));
        assert_eq!(suggest_address(Some(identity), &empty, 2), Some(first + 2));
    }

    /// An address already in the table — an imported row carrying another
    /// controller's address, typically — is stepped over rather than
    /// duplicated. A duplicate makes the device refuse the **whole** table.
    #[test]
    fn a_suggestion_never_repeats_an_address_already_written() {
        let identity = RemoteIdentity::from_mac([0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03]);
        let would_be = suggest_address(Some(identity), &record_with(&[]), 0).unwrap();
        let clashing = record_with(&[would_be]);
        let suggestion = suggest_address(Some(identity), &clashing, 0).unwrap();
        assert_ne!(suggestion, would_be);
    }

    /// Every suggestion is an address `ShadeConfig` will take, so pressing
    /// return can never produce a row the device refuses.
    #[test]
    fn every_suggestion_is_an_address_the_domain_accepts() {
        let identity = RemoteIdentity::from_mac([0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF]);
        let empty = record_with(&[]);
        for index in 0..SHADE_TABLE_CAPACITY {
            let address = suggest_address(Some(identity), &empty, index).unwrap();
            assert!(ShadeConfig::new("probe", address).is_ok(), "{address:#08X}");
        }
    }
}
