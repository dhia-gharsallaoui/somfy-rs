//! The `system` resource: the two bounds the firmware allocates against, and
//! the rule that keeps a backup from being a way to read the passphrase out.
//!
//! `SYSTEM_JSON_MAX_BYTES` and `RESTORE_JSON_MAX_BYTES` are buffers held across
//! a response write inside each of the web server's connection task futures —
//! four copies, out of the DRAM the Wi-Fi driver's heap is carved from. Both
//! are asserted from **both** sides here: never under the widest legal value,
//! and never more than 128 bytes over it. `crate::SHADE_JSON_MAX_BYTES` was
//! hand-counted once and was wrong by 160 bytes, which would have made one
//! shade answer with malformed JSON forever; that is the mistake this file
//! exists to make impossible for these two.

use somfy_api::{
    ApiErrorCode, ApiErrorDto, BackupContentsDto, BackupFormatDto, ChipDto, HeapDto, LogDto,
    PanicDto, ResetReasonDto, RestoreOutcomeDto, RestoreReportDto, StackDto, SystemDto,
    MAX_HOST_LEN, MAX_PANIC_TEXT_LEN, MAX_VERSION_LEN, RESTORE_JSON_MAX_BYTES,
    SYSTEM_JSON_MAX_BYTES,
};

/// A string of `len` bytes, every one of which JSON has to lengthen.
///
/// `"` and `\` are the only printable ASCII characters JSON escapes. Used for
/// the free-text fields that carry whatever an access point or a broker was
/// named, which is genuinely arbitrary.
///
/// **It is deliberately not used for the panic text**, and that difference is
/// the whole reason `SYSTEM_JSON_MAX_BYTES` is what it is: see
/// [`widest_panic_text`].
fn widest_text<const N: usize>(len: usize) -> heapless::String<N> {
    let mut text = heapless::String::new();
    for index in 0..len {
        text.push(if index % 2 == 0 { '"' } else { '\\' })
            .expect("the caller asked for no more than the capacity");
    }
    text
}

/// The widest panic text the device can actually produce.
///
/// `firmware::diag::push_sanitised` substitutes `.` for everything outside
/// `0x20..=0x7E` **and** for the two printable characters JSON escapes, so a
/// stored panic text costs exactly one JSON byte per byte. `~` is the highest
/// character it admits.
///
/// This is a claim about the firmware, asserted here because the constant it
/// sizes lives here. If that substitution is ever relaxed this test keeps
/// passing while the device overruns the buffer, so the two have to be changed
/// together; `SYSTEM_JSON_MAX_BYTES` says so in its own documentation.
fn widest_panic_text() -> heapless::String<MAX_PANIC_TEXT_LEN> {
    let mut text = heapless::String::new();
    for _ in 0..MAX_PANIC_TEXT_LEN {
        text.push('~').expect("exactly the capacity");
    }
    text
}

fn widest_system() -> SystemDto {
    SystemDto {
        chip: ChipDto::Esp32S3,
        firmware: widest_text::<MAX_VERSION_LEN>(MAX_VERSION_LEN),
        host: widest_text::<MAX_HOST_LEN>(MAX_HOST_LEN),
        uptime_s: u32::MAX,
        // The longest of the six, so the discriminant contributes its worst.
        reset_reason: ResetReasonDto::Watchdog,
        stack: StackDto {
            available: u32::MAX,
            required: u32::MAX,
            used: Some(u32::MAX),
        },
        heap: HeapDto {
            size: u32::MAX,
            used: u32::MAX,
            peak: u32::MAX,
        },
        log: LogDto {
            capacity: u32::MAX,
            bytes: u32::MAX,
            lines: u32::MAX,
            dropped: u32::MAX,
        },
        last_panic: Some(PanicDto {
            text: widest_panic_text(),
            truncated: true,
            uptime_s: u32::MAX,
            boots_since: u32::MAX,
        }),
    }
}

fn widest_restore() -> RestoreReportDto {
    RestoreReportDto {
        outcome: RestoreOutcomeDto::Refused,
        format: Some(BackupFormatDto::EspSomfyRts),
        shades: u8::MAX,
        rooms: u8::MAX,
        groups: u8::MAX,
        warnings: u8::MAX,
        error: Some(ApiErrorDto {
            code: ApiErrorCode::BackupUnsupportedVersion,
            field: None,
        }),
        row: Some(u8::MAX),
        contents: Some(BackupContentsDto {
            ssid: Some(widest_text::<32>(32)),
            psk_was_set: true,
            broker: Some(widest_text::<21>(21)),
            broker_password_was_set: true,
        }),
    }
}

#[test]
fn the_widest_system_document_fits_the_buffer_the_firmware_allocates() {
    let json = serde_json::to_string(&widest_system()).expect("serialises");
    assert!(
        json.len() <= SYSTEM_JSON_MAX_BYTES,
        "the widest system document serialises to {} bytes, over \
         SYSTEM_JSON_MAX_BYTES of {SYSTEM_JSON_MAX_BYTES}",
        json.len(),
    );
    assert!(
        json.len() + 128 >= SYSTEM_JSON_MAX_BYTES,
        "SYSTEM_JSON_MAX_BYTES ({SYSTEM_JSON_MAX_BYTES}) is more than 128 bytes above the \
         measured worst case ({}) — every byte of slack is spent four times, in Wi-Fi \
         driver headroom, on every boot",
        json.len(),
    );
}

#[test]
fn the_widest_restore_report_fits_the_buffer_the_firmware_allocates() {
    let json = serde_json::to_string(&widest_restore()).expect("serialises");
    assert!(
        json.len() <= RESTORE_JSON_MAX_BYTES,
        "the widest restore report serialises to {} bytes, over \
         RESTORE_JSON_MAX_BYTES of {RESTORE_JSON_MAX_BYTES}",
        json.len(),
    );
    assert!(
        json.len() + 128 >= RESTORE_JSON_MAX_BYTES,
        "RESTORE_JSON_MAX_BYTES ({RESTORE_JSON_MAX_BYTES}) is more than 128 bytes above the \
         measured worst case ({})",
        json.len(),
    );
}

#[test]
fn an_ordinary_system_document_is_a_fraction_of_the_bound() {
    // The figure a real board answers with, so that the bound above is
    // understood as a worst case and not as what a request costs.
    let json = serde_json::to_string(&SystemDto {
        chip: ChipDto::Esp32S3,
        firmware: "0.1.0".try_into().expect("fits"),
        host: "somfy-0011223344ff".try_into().expect("fits"),
        uptime_s: 86_400,
        reset_reason: ResetReasonDto::PowerOn,
        stack: StackDto {
            available: 66_148,
            required: 57_120,
            used: Some(55_416),
        },
        heap: HeapDto {
            size: 65_536,
            used: 47_464,
            peak: 51_096,
        },
        log: LogDto {
            capacity: 4_096,
            bytes: 3_100,
            lines: 42,
            dropped: 0,
        },
        last_panic: None,
    })
    .expect("serialises");
    assert!(
        json.len() < SYSTEM_JSON_MAX_BYTES / 2,
        "an ordinary system document is {} bytes",
        json.len(),
    );
}

// ---------------------------------------------------------------------------
// The rule the whole resource is arranged around
// ---------------------------------------------------------------------------

/// A passphrase chosen to be findable in a byte haystack. Synthetic.
const SECRET_PSK: &str = "ZZZ-wifi-secret-never-leaves-ZZZ";
/// The same, for the broker.
const SECRET_PASSWORD: &str = "QQQ-broker-secret-never-leaves-QQQ";

#[test]
fn a_backup_report_says_whether_a_secret_was_set_and_never_what_it_was() {
    // There is no field to put one in — which is the point, and is why this
    // test constructs the *widest* contents rather than trying to smuggle a
    // secret through. It fails to compile if a field is ever added that could
    // hold one, and fails at run time if a future `Serialize` writes one.
    let report = RestoreReportDto {
        contents: Some(BackupContentsDto {
            ssid: Some("example-network".try_into().expect("fits")),
            psk_was_set: true,
            broker: Some("192.0.2.10".try_into().expect("fits")),
            broker_password_was_set: true,
        }),
        ..RestoreReportDto::nothing()
    };
    let json = serde_json::to_string(&report).expect("serialises");
    assert!(!json.contains(SECRET_PSK), "{json}");
    assert!(!json.contains(SECRET_PASSWORD), "{json}");
    assert!(json.contains("\"pskWasSet\":true"), "{json}");
    assert!(json.contains("\"brokerPasswordWasSet\":true"), "{json}");
}

#[test]
fn nothing_staged_is_a_value_rather_than_an_absence() {
    // A device with no staged restore answers `outcome: "none"` and not a 404:
    // the question "has anything been uploaded" has an answer, and it is "no".
    let json = serde_json::to_string(&RestoreReportDto::nothing()).expect("serialises");
    assert!(json.contains("\"outcome\":\"none\""), "{json}");
    assert!(json.contains("\"error\":null"), "{json}");
    assert!(json.contains("\"contents\":null"), "{json}");
}

// ---------------------------------------------------------------------------
// The status table
// ---------------------------------------------------------------------------

#[test]
fn every_backup_refusal_carries_a_status_a_client_can_act_on() {
    // The two that are about the file are 400: no state on the device would
    // make those bytes acceptable.
    assert_eq!(ApiErrorCode::BackupNotRecognised.http_status(), 400);
    assert_eq!(ApiErrorCode::BackupDamaged.http_status(), 400);
    assert_eq!(ApiErrorCode::BackupUnsupportedVersion.http_status(), 400);
    assert_eq!(ApiErrorCode::AddressInUse.http_status(), 400);
    // Size is its own answer, for the reason `ImageTooLarge` is: a 400 would
    // send somebody looking at the contents of a file whose length is wrong.
    assert_eq!(ApiErrorCode::BackupTooLarge.http_status(), 413);
    // A conflict with the state of the device rather than a malformed request.
    assert_eq!(ApiErrorCode::RestoreInProgress.http_status(), 409);
    // Nothing the caller could have sent instead.
    assert_eq!(ApiErrorCode::BackupUnwritable.http_status(), 500);
}
