//! The rejection contract: the wire spelling of each code, and the HTTP status
//! it is reported with.
//!
//! Both halves are pinned because both are shared with a client that is not
//! compiled from this source. `ui/mock/plugin.ts` maps the same codes to the
//! same statuses so that one UI can run against the mock and the device with no
//! "mock mode" branch; a change here that is not made there produces a UI which
//! behaves differently against the two, which is the failure the mock exists to
//! prevent.

use somfy_api::{ApiErrorCode, ApiErrorDto};

/// Every code, so the array below cannot silently miss one.
///
/// A `match` rather than a bare list: adding a variant fails to compile here
/// until it is added to `ALL`, which is what makes the exhaustiveness real
/// rather than a comment asking for it.
fn is_listed(code: ApiErrorCode) -> bool {
    match code {
        ApiErrorCode::NameEmpty
        | ApiErrorCode::NameTooLong
        | ApiErrorCode::InvalidKind
        | ApiErrorCode::InvalidTiltMode
        | ApiErrorCode::TravelTimeZero
        | ApiErrorCode::InvalidAddress
        | ApiErrorCode::RegistryFull
        | ApiErrorCode::NotFound
        | ApiErrorCode::AddressNotAllocated
        | ApiErrorCode::InvalidDeadBand
        | ApiErrorCode::VentBandNotMeasured
        | ApiErrorCode::NotCalibrating
        | ApiErrorCode::CalibrationImplausible
        | ApiErrorCode::CommandNotAtThisWidth
        | ApiErrorCode::CommandRateLimited
        | ApiErrorCode::HostNotThisDevice
        | ApiErrorCode::OriginNotThisDevice
        | ApiErrorCode::ValueEmpty
        | ApiErrorCode::ValueTooLong
        | ApiErrorCode::ValueTooShort
        | ApiErrorCode::ValueInteriorNul
        | ApiErrorCode::BrokerAddressMalformed
        | ApiErrorCode::BrokerAddressUnroutable
        | ApiErrorCode::BrokerPortZero
        | ApiErrorCode::PasswordWithoutUsername
        | ApiErrorCode::TopicWildcard
        | ApiErrorCode::TopicLeadingSlash
        | ApiErrorCode::TopicTrailingSlash
        | ApiErrorCode::TopicEmptySegment
        | ApiErrorCode::TopicIllegalCharacter
        | ApiErrorCode::NamespacesOverlap
        | ApiErrorCode::SecretNotSet
        | ApiErrorCode::NoTrialInProgress
        | ApiErrorCode::TrialInProgress
        | ApiErrorCode::TrialNotAssociated
        | ApiErrorCode::SettingsUnwritable
        | ApiErrorCode::ImageNotFirmware
        | ApiErrorCode::ImageForAnotherChip
        | ApiErrorCode::ImageTooLarge
        | ApiErrorCode::ImageDamaged
        | ApiErrorCode::UpdateInProgress
        | ApiErrorCode::UpdateUnwritable => true,
    }
}

const ALL: &[(ApiErrorCode, &str, u16)] = &[
    (ApiErrorCode::NameEmpty, "nameEmpty", 400),
    (ApiErrorCode::NameTooLong, "nameTooLong", 400),
    (ApiErrorCode::InvalidKind, "invalidKind", 400),
    (ApiErrorCode::InvalidTiltMode, "invalidTiltMode", 400),
    (ApiErrorCode::TravelTimeZero, "travelTimeZero", 400),
    (ApiErrorCode::InvalidAddress, "invalidAddress", 500),
    (ApiErrorCode::RegistryFull, "registryFull", 409),
    (ApiErrorCode::NotFound, "notFound", 404),
    (
        ApiErrorCode::AddressNotAllocated,
        "addressNotAllocated",
        409,
    ),
    (ApiErrorCode::InvalidDeadBand, "invalidDeadBand", 400),
    (
        ApiErrorCode::VentBandNotMeasured,
        "ventBandNotMeasured",
        409,
    ),
    (ApiErrorCode::NotCalibrating, "notCalibrating", 409),
    (
        ApiErrorCode::CalibrationImplausible,
        "calibrationImplausible",
        400,
    ),
    (
        ApiErrorCode::CommandNotAtThisWidth,
        "commandNotAtThisWidth",
        409,
    ),
    (ApiErrorCode::CommandRateLimited, "commandRateLimited", 429),
    (ApiErrorCode::HostNotThisDevice, "hostNotThisDevice", 403),
    (
        ApiErrorCode::OriginNotThisDevice,
        "originNotThisDevice",
        403,
    ),
    (ApiErrorCode::ValueEmpty, "valueEmpty", 400),
    (ApiErrorCode::ValueTooLong, "valueTooLong", 400),
    (ApiErrorCode::ValueTooShort, "valueTooShort", 400),
    (ApiErrorCode::ValueInteriorNul, "valueInteriorNul", 400),
    (
        ApiErrorCode::BrokerAddressMalformed,
        "brokerAddressMalformed",
        400,
    ),
    (
        ApiErrorCode::BrokerAddressUnroutable,
        "brokerAddressUnroutable",
        400,
    ),
    (ApiErrorCode::BrokerPortZero, "brokerPortZero", 400),
    (
        ApiErrorCode::PasswordWithoutUsername,
        "passwordWithoutUsername",
        400,
    ),
    (ApiErrorCode::TopicWildcard, "topicWildcard", 400),
    (ApiErrorCode::TopicLeadingSlash, "topicLeadingSlash", 400),
    (ApiErrorCode::TopicTrailingSlash, "topicTrailingSlash", 400),
    (ApiErrorCode::TopicEmptySegment, "topicEmptySegment", 400),
    (
        ApiErrorCode::TopicIllegalCharacter,
        "topicIllegalCharacter",
        400,
    ),
    (ApiErrorCode::NamespacesOverlap, "namespacesOverlap", 400),
    (ApiErrorCode::SecretNotSet, "secretNotSet", 400),
    (ApiErrorCode::NoTrialInProgress, "noTrialInProgress", 409),
    (ApiErrorCode::TrialInProgress, "trialInProgress", 409),
    (ApiErrorCode::TrialNotAssociated, "trialNotAssociated", 409),
    (ApiErrorCode::SettingsUnwritable, "settingsUnwritable", 500),
    (ApiErrorCode::ImageNotFirmware, "imageNotFirmware", 400),
    (
        ApiErrorCode::ImageForAnotherChip,
        "imageForAnotherChip",
        400,
    ),
    (ApiErrorCode::ImageTooLarge, "imageTooLarge", 413),
    (ApiErrorCode::ImageDamaged, "imageDamaged", 400),
    (ApiErrorCode::UpdateInProgress, "updateInProgress", 409),
    (ApiErrorCode::UpdateUnwritable, "updateUnwritable", 500),
];

#[test]
fn every_code_is_listed() {
    for (code, _, _) in ALL {
        assert!(is_listed(*code));
    }
}

#[test]
fn each_code_serializes_to_its_wire_spelling() {
    for (code, spelling, _) in ALL {
        let json = serde_json::to_string(&ApiErrorDto::from(*code)).unwrap();
        assert_eq!(json, alloc_format(spelling), "{code:?}");
    }
}

#[test]
fn each_code_carries_the_status_the_mock_answers_with() {
    for (code, _, status) in ALL {
        assert_eq!(code.http_status(), *status, "{code:?}");
    }
}

/// `registryFull` is a conflict with the collection's state, not a storage
/// failure the client can wait out: the fix is to remove a shade. Pinned on its
/// own because 507 is the plausible wrong answer and nothing else in the table
/// would catch the swap.
#[test]
fn registry_full_is_a_conflict_not_insufficient_storage() {
    assert_eq!(ApiErrorCode::RegistryFull.http_status(), 409);
}

/// A pairing request for an imported shade is well-formed; what makes it
/// inapplicable is the shade's address. 400 would send the UI looking for a
/// form field to highlight, and there is none.
#[test]
fn address_not_allocated_is_a_conflict_not_a_bad_request() {
    assert_eq!(ApiErrorCode::AddressNotAllocated.http_status(), 409);
}

fn alloc_format(spelling: &str) -> String {
    format!(r#"{{"code":"{spelling}"}}"#)
}
