//! The shade lifecycle contract: the create and patch bodies with their
//! validation, the address-origin classification that decides whether pairing
//! is offered, and the calibration provenance that decides whether a position
//! estimate is worth anything.

use somfy_api::{
    AddressOrigin, ApiErrorCode, ApiErrorDto, CalibrationSource, CreateShadeDto, PatchShadeDto,
    ShadeDto, FACTORY_DOWN_TIME_MS, FACTORY_TILT_TIME_MS, FACTORY_UP_TIME_MS,
};
use somfy_domain::{RemoteIdentity, Shade, ShadeConfig, ShadeId, ShadeKind, TiltMode};

/// A well-formed request body, as JSON, so the tests exercise the wire form
/// rather than a struct literal that could not have arrived over HTTP.
fn body(overrides: serde_json::Value) -> CreateShadeDto {
    let mut value = serde_json::json!({
        "name": "Landing",
        "kind": 0x01,
        "tiltMode": 0x02,
        "upTimeMs": 8_000,
        "downTimeMs": 7_000,
        "tiltTimeMs": 1_500,
    });
    let object = value.as_object_mut().expect("object");
    for (key, patch) in overrides.as_object().expect("overrides object") {
        object.insert(key.clone(), patch.clone());
    }
    serde_json::from_value(value).expect("deserialize CreateShadeDto")
}

const ALLOCATED: u32 = RemoteIdentity::SPACE_START | 0x0A_CE01;
const IMPORTED: u32 = 0x7A_CE01;

/// The fixtures only mean anything if they really do straddle the reserved bit,
/// and that is decidable at compile time — so it is decided there rather than
/// asserted in one test that a later edit could quietly stop running.
const _: () = assert!(IMPORTED < RemoteIdentity::SPACE_START);
const _: () = assert!(ALLOCATED > RemoteIdentity::SPACE_START);

// ---------------------------------------------------------------- create body

#[test]
fn a_valid_body_lowers_onto_the_domain_config() {
    let config = body(serde_json::json!({})).to_config(ALLOCATED).unwrap();
    assert_eq!(config.name.as_str(), "Landing");
    assert_eq!(config.address, ALLOCATED);
    assert_eq!(config.kind, ShadeKind::Blind);
    assert_eq!(config.tilt_mode, TiltMode::Integrated);
    assert_eq!(config.up_time_ms, 8_000);
    assert_eq!(config.down_time_ms, 7_000);
    assert_eq!(config.tilt_time_ms, 1_500);
}

#[test]
fn an_empty_name_is_refused_even_though_the_domain_would_take_it() {
    // The domain accepts it (a migrated backup may carry one); a newly created
    // shade with no name is a row nobody can identify, so the boundary is
    // deliberately stricter. Guard against the domain quietly gaining the rule
    // and this test then proving nothing.
    assert!(ShadeConfig::new("", ALLOCATED).is_ok());
    assert_eq!(
        body(serde_json::json!({ "name": "" })).to_config(ALLOCATED),
        Err(ApiErrorCode::NameEmpty)
    );
}

#[test]
fn a_name_over_the_limit_is_a_typed_rejection_not_a_parse_failure() {
    let long = "x".repeat(somfy_api::NAME_MAX_BYTES + 1);
    assert_eq!(
        body(serde_json::json!({ "name": long })).to_config(ALLOCATED),
        Err(ApiErrorCode::NameTooLong)
    );
}

#[test]
fn the_name_limit_is_bytes_not_characters() {
    // Sixteen two-byte characters are 16 characters and 32 bytes: the longest
    // accented French name that fits. Seventeen do not, and a UI counting
    // `String.length` would have offered the user a name the device refuses.
    let exactly_full = "é".repeat(somfy_api::NAME_MAX_BYTES / 2);
    assert_eq!(exactly_full.chars().count(), 16);
    assert_eq!(exactly_full.len(), somfy_api::NAME_MAX_BYTES);
    assert!(body(serde_json::json!({ "name": exactly_full }))
        .to_config(ALLOCATED)
        .is_ok());

    let one_too_many = "é".repeat(somfy_api::NAME_MAX_BYTES / 2 + 1);
    assert_eq!(
        body(serde_json::json!({ "name": one_too_many })).to_config(ALLOCATED),
        Err(ApiErrorCode::NameTooLong)
    );
}

#[test]
fn an_unmodelled_kind_is_refused_rather_than_defaulted_to_roller() {
    // 0x05 is a garage door: a kind deployed devices have and this firmware
    // does not model. Import policy defaults it to Roller and warns; create
    // must not, because handing back a roller is a wrong answer to a question
    // nobody had to ask yet.
    assert!(ShadeKind::from_raw(0x05).is_none());
    assert_eq!(
        body(serde_json::json!({ "kind": 0x05 })).to_config(ALLOCATED),
        Err(ApiErrorCode::InvalidKind)
    );
}

#[test]
fn an_unmodelled_tilt_mode_is_refused() {
    assert!(TiltMode::from_raw(0x09).is_none());
    assert_eq!(
        body(serde_json::json!({ "tiltMode": 0x09 })).to_config(ALLOCATED),
        Err(ApiErrorCode::InvalidTiltMode)
    );
}

#[test]
fn a_zero_lift_travel_time_is_refused_in_either_direction() {
    for field in ["upTimeMs", "downTimeMs"] {
        assert_eq!(
            body(serde_json::json!({ field: 0 })).to_config(ALLOCATED),
            Err(ApiErrorCode::TravelTimeZero),
            "{field} of zero must be refused"
        );
    }
}

#[test]
fn a_zero_tilt_time_is_accepted_because_a_tiltless_shade_has_none() {
    let config = body(serde_json::json!({ "tiltMode": 0x00, "tiltTimeMs": 0 }))
        .to_config(ALLOCATED)
        .unwrap();
    assert_eq!(config.tilt_mode, TiltMode::None);
    assert_eq!(config.tilt_time_ms, 0);
}

#[test]
fn a_sentinel_address_is_reported_rather_than_silently_accepted() {
    // Not reachable through `RemoteIdentity::address_for`; carried so an
    // allocator that has gone wrong says so.
    assert_eq!(
        body(serde_json::json!({})).to_config(0xFF_FFFF),
        Err(ApiErrorCode::InvalidAddress)
    );
}

// -------------------------------------------------------------- address origin

#[test]
fn every_address_the_allocator_can_produce_classifies_as_allocated() {
    let identity = RemoteIdentity::from_mac([0x02, 0x00, 0x00, 0x12, 0x34, 0x56]);
    for slot in [0u8, 1, 31] {
        let address = identity.address_for(ShadeId(slot), |_| false).unwrap();
        assert_eq!(
            AddressOrigin::of(address),
            AddressOrigin::Allocated,
            "{address:#08X} came from our own allocator"
        );
    }
}

#[test]
fn an_address_below_the_reserved_bit_is_imported() {
    assert_eq!(AddressOrigin::of(IMPORTED), AddressOrigin::Imported);
    // The sentinel the domain refuses is still classified rather than panicking:
    // classification is a read, and a read must be total.
    assert_eq!(AddressOrigin::of(0), AddressOrigin::Imported);
}

#[test]
fn shade_dto_derives_the_origin_from_the_address() {
    for (address, expected) in [
        (ALLOCATED, AddressOrigin::Allocated),
        (IMPORTED, AddressOrigin::Imported),
    ] {
        let shade = Shade::new(ShadeConfig::new("Landing", address).unwrap());
        let dto = ShadeDto::from_shade(ShadeId(0), &shade);
        assert_eq!(dto.address_origin, expected);
    }
}

#[test]
fn address_origin_serializes_as_a_camel_case_string() {
    let shade = Shade::new(ShadeConfig::new("Landing", IMPORTED).unwrap());
    let json = serde_json::to_value(ShadeDto::from_shade(ShadeId(0), &shade)).unwrap();
    assert_eq!(json["addressOrigin"], "imported");
}

// --------------------------------------------------------------- calibration

/// The whole R7 mechanism rests on these three numbers being the ones
/// `ShadeConfig::new` actually hands out. `somfy-api` restates them because the
/// domain returns them inside a value rather than exposing constants, so this
/// is the pin that stops the restatement drifting: a domain default changed
/// without changing these would otherwise make every shade in the estate
/// silently report itself as *calibrated*, which is precisely the failure R7
/// was raised to a MUST to prevent.
#[test]
fn the_restated_factory_defaults_are_the_domain_s_own() {
    let fresh = ShadeConfig::new("pin", ALLOCATED).unwrap();
    assert_eq!(fresh.up_time_ms, FACTORY_UP_TIME_MS);
    assert_eq!(fresh.down_time_ms, FACTORY_DOWN_TIME_MS);
    assert_eq!(fresh.tilt_time_ms, FACTORY_TILT_TIME_MS);
}

#[test]
fn an_untouched_shade_reports_every_travel_time_as_uncalibrated() {
    // The 2026-08-17 failure in miniature: three factory defaults, imported
    // faithfully, presented as configured. They must now read as uncalibrated.
    let shade = Shade::new(ShadeConfig::new("Imported", ALLOCATED).unwrap());
    let dto = ShadeDto::from_shade(ShadeId(0), &shade);
    assert_eq!(dto.up_time_source, CalibrationSource::FactoryDefault);
    assert_eq!(dto.down_time_source, CalibrationSource::FactoryDefault);
    assert_eq!(dto.tilt_time_source, CalibrationSource::FactoryDefault);
}

#[test]
fn hand_measured_times_report_as_operator_supplied_per_field() {
    // The real measurement from that day: 30 s up, 27 s down — a ~10% asymmetry,
    // because closing is gravity-assisted. Tilt was left alone, and must still
    // read as uncalibrated: the states are per field, not per shade.
    let mut config = ShadeConfig::new("Measured", ALLOCATED).unwrap();
    config.up_time_ms = 30_000;
    config.down_time_ms = 27_000;
    let dto = ShadeDto::from_shade(ShadeId(0), &Shade::new(config));

    assert_eq!(dto.up_time_source, CalibrationSource::OperatorSupplied);
    assert_eq!(dto.down_time_source, CalibrationSource::OperatorSupplied);
    assert_eq!(dto.tilt_time_source, CalibrationSource::FactoryDefault);
}

#[test]
fn calibration_source_serializes_as_a_camel_case_string() {
    let shade = Shade::new(ShadeConfig::new("Imported", ALLOCATED).unwrap());
    let json = serde_json::to_value(ShadeDto::from_shade(ShadeId(0), &shade)).unwrap();
    assert_eq!(json["upTimeSource"], "factoryDefault");
    assert_eq!(json["upTimeMs"], 10_000);
}

// -------------------------------------------------------------------- patch

fn patch(body: serde_json::Value) -> PatchShadeDto {
    serde_json::from_value(body).expect("deserialize PatchShadeDto")
}

fn configured() -> ShadeConfig {
    let mut config = ShadeConfig::new("Landing", ALLOCATED).unwrap();
    config.kind = ShadeKind::Blind;
    config.tilt_mode = TiltMode::Integrated;
    config.up_time_ms = 8_000;
    config.down_time_ms = 7_000;
    config.tilt_time_ms = 1_500;
    config
}

#[test]
fn an_empty_patch_changes_nothing() {
    assert_eq!(
        patch(serde_json::json!({})).apply(&configured()).unwrap(),
        configured()
    );
}

#[test]
fn a_patch_touches_only_the_fields_it_names() {
    // The R9 case: an operator with a stopwatch supplies both lift times in one
    // request and says nothing about anything else.
    let next = patch(serde_json::json!({ "upTimeMs": 30_000, "downTimeMs": 27_000 }))
        .apply(&configured())
        .unwrap();

    assert_eq!(next.up_time_ms, 30_000);
    assert_eq!(next.down_time_ms, 27_000);
    assert_eq!(next.tilt_time_ms, 1_500);
    assert_eq!(next.name.as_str(), "Landing");
    assert_eq!(next.kind, ShadeKind::Blind);
}

#[test]
fn a_patch_never_moves_the_address() {
    // Editing it would break a pairing a motor has already learned, so it is
    // not a field at all — a body naming it is ignored rather than obeyed.
    let next = patch(serde_json::json!({ "address": 0x123456, "upTimeMs": 30_000 }))
        .apply(&configured())
        .unwrap();
    assert_eq!(next.address, ALLOCATED);
}

#[test]
fn patched_times_then_report_as_operator_supplied() {
    // End to end: patch the value, snapshot the shade, and the provenance the
    // UI renders has followed the edit.
    let mut config = ShadeConfig::new("Fresh", ALLOCATED).unwrap();
    config = patch(serde_json::json!({ "upTimeMs": 30_000 }))
        .apply(&config)
        .unwrap();
    let dto = ShadeDto::from_shade(ShadeId(0), &Shade::new(config));

    assert_eq!(dto.up_time_source, CalibrationSource::OperatorSupplied);
    assert_eq!(dto.down_time_source, CalibrationSource::FactoryDefault);
}

#[test]
fn a_patch_back_to_the_default_reports_as_uncalibrated_again() {
    // Not a quirk to hide: under R7 the value *is* the evidence, so typing the
    // default back in genuinely removes the evidence that anybody measured it.
    let next = patch(serde_json::json!({ "upTimeMs": FACTORY_UP_TIME_MS }))
        .apply(&configured())
        .unwrap();
    let dto = ShadeDto::from_shade(ShadeId(0), &Shade::new(next));
    assert_eq!(dto.up_time_source, CalibrationSource::FactoryDefault);
}

#[test]
fn a_patch_is_refused_by_the_same_rules_as_a_create() {
    // The invariant: nothing reachable by creating and then patching may be
    // unreachable by creating directly.
    for (body, expected) in [
        (serde_json::json!({ "name": "" }), ApiErrorCode::NameEmpty),
        (
            serde_json::json!({ "name": "x".repeat(somfy_api::NAME_MAX_BYTES + 1) }),
            ApiErrorCode::NameTooLong,
        ),
        (
            serde_json::json!({ "kind": 0x05 }),
            ApiErrorCode::InvalidKind,
        ),
        (
            serde_json::json!({ "tiltMode": 0x09 }),
            ApiErrorCode::InvalidTiltMode,
        ),
        (
            serde_json::json!({ "upTimeMs": 0 }),
            ApiErrorCode::TravelTimeZero,
        ),
        (
            serde_json::json!({ "downTimeMs": 0 }),
            ApiErrorCode::TravelTimeZero,
        ),
    ] {
        assert_eq!(
            patch(body.clone()).apply(&configured()),
            Err(expected),
            "patch {body} must be refused"
        );
    }
}

#[test]
fn a_rejected_patch_applies_none_of_itself() {
    // The name is valid and the kind is not. The config handed back must be an
    // error, never a shade that has been renamed and left with its old kind.
    let before = configured();
    let result = patch(serde_json::json!({ "name": "Renamed", "kind": 0x05 })).apply(&before);
    assert_eq!(result, Err(ApiErrorCode::InvalidKind));
    assert_eq!(before.name.as_str(), "Landing");
}

#[test]
fn a_zero_lift_time_is_refused_against_the_result_not_the_body() {
    // The body says nothing about `downTimeMs`, so the check has to look at
    // what the shade would end up with rather than at what was sent.
    let mut current = configured();
    current.down_time_ms = 7_000;
    assert_eq!(
        patch(serde_json::json!({ "upTimeMs": 0 })).apply(&current),
        Err(ApiErrorCode::TravelTimeZero)
    );
}

#[test]
fn a_patch_may_set_a_tilt_time_of_zero() {
    let next = patch(serde_json::json!({ "tiltTimeMs": 0 }))
        .apply(&configured())
        .unwrap();
    assert_eq!(next.tilt_time_ms, 0);
}

// --------------------------------------------------------------------- errors

#[test]
fn an_error_body_is_a_bare_code() {
    let json = serde_json::to_value(ApiErrorDto::from(ApiErrorCode::AddressNotAllocated)).unwrap();
    assert_eq!(json, serde_json::json!({ "code": "addressNotAllocated" }));
}

#[test]
fn error_codes_round_trip() {
    for code in [
        ApiErrorCode::NameEmpty,
        ApiErrorCode::NameTooLong,
        ApiErrorCode::InvalidKind,
        ApiErrorCode::InvalidTiltMode,
        ApiErrorCode::TravelTimeZero,
        ApiErrorCode::InvalidAddress,
        ApiErrorCode::RegistryFull,
        ApiErrorCode::NotFound,
        ApiErrorCode::AddressNotAllocated,
    ] {
        let text = serde_json::to_string(&ApiErrorDto::from(code)).unwrap();
        let back: ApiErrorDto = serde_json::from_str(&text).unwrap();
        assert_eq!(back.code, code);
    }
}
