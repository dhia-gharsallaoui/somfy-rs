//! The settings contract: what goes out, what comes in, and what is refused.
//!
//! # Two things are pinned here that nothing else can pin
//!
//! **That no secret has a way out.** The check is not "we remembered not to
//! send it" but that the serialised bytes of a fully-populated response do not
//! contain the passphrase or the password — run against values chosen to be
//! unmistakable in a haystack. A field added later that carried one would fail
//! this without anybody having to think of it.
//!
//! **That every R3 rejection names its field.** `docs/specs/`
//! `2026-08-15-mqtt-ha-discovery-requirements.md` R3 says an invalid value is
//! refused at the point of entry with the field named, and its acceptance
//! criterion 4 asks for a typed error per invalid input. The table below is
//! that criterion, including the three configurations that made discovery
//! unusable on the C++ build.

use somfy_api::{
    ApiErrorCode, ApiErrorDto, MqttSettingsDto, MqttUpdateDto, SecretDto, SettingsDto,
    SettingsFieldDto, TrialPhaseDto, WifiSettingsDto, WifiTrialDto, WifiUpdateDto,
};
use somfy_config::{MqttSettings, WifiCredentials, WifiTrial, CONFIRM_DEADLINE_MS};

use core::net::Ipv4Addr;

/// A passphrase chosen to be findable in a byte haystack. Synthetic.
const SECRET_PSK: &str = "ZZZ-wifi-secret-never-leaves-ZZZ";
/// The same, for the broker.
const SECRET_PASSWORD: &str = "QQQ-broker-secret-never-leaves-QQQ";

fn stored_wifi() -> WifiCredentials {
    WifiCredentials::new("example-network", SECRET_PSK).expect("a valid credential")
}

fn stored_mqtt() -> MqttSettings {
    MqttSettings::new(
        Ipv4Addr::new(192, 0, 2, 10),
        1883,
        "somfy",
        SECRET_PASSWORD,
        "homeassistant",
        "somfyrs",
    )
    .expect("valid broker settings")
}

/// A well-formed update that changes nothing but the field under test.
fn mqtt_update() -> MqttUpdateDto {
    MqttUpdateDto {
        address: "192.0.2.10".try_into().unwrap(),
        port: 1883,
        username: "somfy".try_into().unwrap(),
        password: SecretDto::Keep,
        discovery_prefix: "homeassistant".try_into().unwrap(),
        state_root: "somfyrs".try_into().unwrap(),
    }
}

fn wifi_update(ssid: &str, psk: SecretDto) -> WifiUpdateDto {
    WifiUpdateDto {
        ssid: ssid.try_into().unwrap(),
        psk,
    }
}

fn set(value: &str) -> SecretDto {
    SecretDto::Set {
        value: value.try_into().unwrap(),
    }
}

// ---------------------------------------------------------------------------
// Secrets go in and never come out
// ---------------------------------------------------------------------------

#[test]
fn a_full_settings_response_contains_neither_secret() {
    let wifi = stored_wifi();
    let mqtt = stored_mqtt();
    let trial = WifiTrial::start(
        WifiCredentials::new("example-other", SECRET_PSK).expect("a valid candidate"),
        0,
    );
    let dto = SettingsDto {
        wifi: Some(WifiSettingsDto::of(&wifi)),
        mqtt: Some(MqttSettingsDto::of(&mqtt)),
        wifi_trial: Some(WifiTrialDto::of(&trial, 0)),
    };

    let json = serde_json::to_string(&dto).expect("serialises");
    assert!(
        !json.contains(SECRET_PSK),
        "the Wi-Fi passphrase reached the wire: {json}",
    );
    assert!(
        !json.contains(SECRET_PASSWORD),
        "the broker password reached the wire: {json}",
    );
    // And the flags that replace them are there, so the screen can still tell
    // "set" from "not set".
    assert!(json.contains("\"pskSet\":true"), "{json}");
    assert!(json.contains("\"passwordSet\":true"), "{json}");
}

#[test]
fn an_open_network_and_an_anonymous_broker_report_no_secret_rather_than_an_empty_one() {
    let open = WifiCredentials::new("example-open", "").expect("an open network is legal");
    let anonymous = MqttSettings::new(
        Ipv4Addr::new(192, 0, 2, 10),
        1883,
        "",
        "",
        "homeassistant",
        "somfyrs",
    )
    .expect("anonymous is legal");

    assert!(!WifiSettingsDto::of(&open).psk_set);
    assert!(!MqttSettingsDto::of(&anonymous).password_set);
}

#[test]
fn the_address_round_trips_as_the_operator_typed_it() {
    let dto = MqttSettingsDto::of(&stored_mqtt());
    assert_eq!(dto.address.as_str(), "192.0.2.10");
    assert_eq!(dto.port, 1883);
    assert_eq!(dto.username.as_str(), "somfy");
    assert_eq!(dto.discovery_prefix.as_str(), "homeassistant");
    assert_eq!(dto.state_root.as_str(), "somfyrs");
}

#[test]
fn a_device_with_nothing_provisioned_serialises_as_three_nulls() {
    let json = serde_json::to_string(&SettingsDto {
        wifi: None,
        mqtt: None,
        wifi_trial: None,
    })
    .expect("serialises");
    assert_eq!(json, r#"{"wifi":null,"mqtt":null,"wifiTrial":null}"#);
}

#[test]
fn a_trial_reports_its_candidate_network_and_the_time_left() {
    let trial = WifiTrial::start(
        WifiCredentials::new("example-other", SECRET_PSK).expect("a valid candidate"),
        1_000,
    );
    let dto = WifiTrialDto::of(&trial, 1_000);
    assert_eq!(dto.ssid.as_str(), "example-other");
    assert_eq!(dto.phase, TrialPhaseDto::Associating);
    assert!(dto.remaining_ms > 0);
}

#[test]
fn the_confirmation_phase_reports_the_confirmation_window() {
    let mut trial = WifiTrial::start(
        WifiCredentials::new("example-other", SECRET_PSK).expect("a valid candidate"),
        0,
    );
    trial.poll(500, true);
    let dto = WifiTrialDto::of(&trial, 500);
    assert_eq!(dto.phase, TrialPhaseDto::AwaitingConfirmation);
    assert_eq!(u64::from(dto.remaining_ms), CONFIRM_DEADLINE_MS);
}

// ---------------------------------------------------------------------------
// A secret that is not sent is not a secret that is cleared
// ---------------------------------------------------------------------------

#[test]
fn keep_resolves_to_what_is_stored() {
    let settings = mqtt_update()
        .to_settings(Some(&stored_mqtt()))
        .expect("accepted");
    assert_eq!(settings.password(), SECRET_PASSWORD);
}

#[test]
fn keep_with_nothing_stored_is_refused_rather_than_read_as_empty() {
    // The failure this variant exists to prevent: an operator configuring a
    // broker for the first time and asking to keep a password that does not
    // exist would otherwise get an anonymous connection they did not ask for.
    assert_eq!(
        mqtt_update().to_settings(None),
        Err(ApiErrorDto::field(
            ApiErrorCode::SecretNotSet,
            SettingsFieldDto::BrokerPassword,
        )),
    );
}

#[test]
fn set_replaces_and_clear_empties() {
    let mut update = mqtt_update();
    update.password = set("PLACEHOLDER_NEW_PASSWORD");
    assert_eq!(
        update
            .to_settings(Some(&stored_mqtt()))
            .expect("accepted")
            .password(),
        "PLACEHOLDER_NEW_PASSWORD",
    );

    let mut update = mqtt_update();
    update.password = SecretDto::Clear;
    update.username = "".try_into().unwrap();
    let settings = update.to_settings(Some(&stored_mqtt())).expect("accepted");
    assert!(settings.is_anonymous());
    assert_eq!(settings.password(), "");
}

#[test]
fn a_wifi_update_can_keep_the_stored_passphrase_while_changing_the_ssid() {
    let credentials = wifi_update("example-other", SecretDto::Keep)
        .to_credentials(Some(&stored_wifi()))
        .expect("accepted");
    assert_eq!(credentials.ssid(), "example-other");
    assert_eq!(credentials.psk(), SECRET_PSK);
}

#[test]
fn clearing_a_wifi_passphrase_means_an_open_network() {
    let credentials = wifi_update("example-open", SecretDto::Clear)
        .to_credentials(Some(&stored_wifi()))
        .expect("accepted");
    assert!(credentials.is_open());
}

// ---------------------------------------------------------------------------
// The wire form of a secret
// ---------------------------------------------------------------------------

#[test]
fn a_secret_is_read_from_its_tag_and_a_set_without_a_value_is_malformed() {
    assert_eq!(
        serde_json::from_str::<SecretDto>(r#"{"secret":"keep"}"#).unwrap(),
        SecretDto::Keep,
    );
    assert_eq!(
        serde_json::from_str::<SecretDto>(r#"{"secret":"clear"}"#).unwrap(),
        SecretDto::Clear,
    );
    assert_eq!(
        serde_json::from_str::<SecretDto>(r#"{"secret":"set","value":"abcdefgh"}"#).unwrap(),
        set("abcdefgh"),
    );
    assert!(serde_json::from_str::<SecretDto>(r#"{"secret":"set"}"#).is_err());
}

#[test]
fn an_absent_secret_is_a_malformed_body_rather_than_a_guess() {
    // Neither "keep" nor "clear" — the two things an omission might have meant
    // — so it is refused rather than picked between.
    assert!(serde_json::from_str::<WifiUpdateDto>(r#"{"ssid":"example-network"}"#).is_err());
}

// ---------------------------------------------------------------------------
// R3: refused at the point of entry, with the field named
// ---------------------------------------------------------------------------

/// Every Wi-Fi rejection, as `(ssid, psk, code, field)`.
#[test]
fn every_wifi_rejection_names_its_field() {
    let cases: &[(&str, SecretDto, ApiErrorCode, SettingsFieldDto)] = &[
        (
            "",
            set("PLACEHOLDER_PASSPHRASE"),
            ApiErrorCode::ValueEmpty,
            SettingsFieldDto::Ssid,
        ),
        (
            // 33 bytes against a 32-byte limit.
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            set("PLACEHOLDER_PASSPHRASE"),
            ApiErrorCode::ValueTooLong,
            SettingsFieldDto::Ssid,
        ),
        (
            "example-network",
            set("short"),
            ApiErrorCode::ValueTooShort,
            SettingsFieldDto::Psk,
        ),
        (
            "example-network",
            // 65 bytes against a 64-byte limit.
            set(&"b".repeat(65)),
            ApiErrorCode::ValueTooLong,
            SettingsFieldDto::Psk,
        ),
        (
            "exam\0ple",
            set("PLACEHOLDER_PASSPHRASE"),
            ApiErrorCode::ValueInteriorNul,
            SettingsFieldDto::Ssid,
        ),
        (
            "example-network",
            set("PLACEHOLDER\0PASSPHRASE"),
            ApiErrorCode::ValueInteriorNul,
            SettingsFieldDto::Psk,
        ),
    ];

    for (ssid, psk, code, field) in cases {
        assert_eq!(
            wifi_update(ssid, psk.clone()).to_credentials(Some(&stored_wifi())),
            Err(ApiErrorDto::field(*code, *field)),
            "ssid {ssid:?}",
        );
    }
}

/// Every MQTT rejection reachable from an operator's form.
#[test]
fn every_mqtt_rejection_names_its_field() {
    /// Apply one change to an otherwise-valid update.
    type Tweak = fn(&mut MqttUpdateDto);

    let cases: &[(&str, Tweak, ApiErrorCode, Option<SettingsFieldDto>)] = &[
        (
            "not an address at all",
            |u| u.address = "192.0.2".try_into().unwrap(),
            ApiErrorCode::BrokerAddressMalformed,
            Some(SettingsFieldDto::BrokerAddress),
        ),
        (
            "unspecified",
            |u| u.address = "0.0.0.0".try_into().unwrap(),
            ApiErrorCode::BrokerAddressUnroutable,
            Some(SettingsFieldDto::BrokerAddress),
        ),
        (
            "loopback",
            |u| u.address = "127.0.0.1".try_into().unwrap(),
            ApiErrorCode::BrokerAddressUnroutable,
            Some(SettingsFieldDto::BrokerAddress),
        ),
        (
            "multicast",
            |u| u.address = "224.0.0.1".try_into().unwrap(),
            ApiErrorCode::BrokerAddressUnroutable,
            Some(SettingsFieldDto::BrokerAddress),
        ),
        (
            "broadcast",
            |u| u.address = "255.255.255.255".try_into().unwrap(),
            ApiErrorCode::BrokerAddressUnroutable,
            Some(SettingsFieldDto::BrokerAddress),
        ),
        (
            "port zero",
            |u| u.port = 0,
            ApiErrorCode::BrokerPortZero,
            Some(SettingsFieldDto::BrokerPort),
        ),
        (
            "username too long",
            |u| u.username = "u".repeat(33).as_str().try_into().unwrap(),
            ApiErrorCode::ValueTooLong,
            Some(SettingsFieldDto::BrokerUsername),
        ),
        (
            "password too long",
            |u| u.password = set(&"p".repeat(65)),
            ApiErrorCode::ValueTooLong,
            Some(SettingsFieldDto::BrokerPassword),
        ),
        (
            "NUL in the username",
            |u| u.username = "som\0fy".try_into().unwrap(),
            ApiErrorCode::ValueInteriorNul,
            Some(SettingsFieldDto::BrokerUsername),
        ),
        (
            "a password with no username",
            |u| {
                u.username = "".try_into().unwrap();
                u.password = set("PLACEHOLDER_BROKER_PASSWORD");
            },
            ApiErrorCode::PasswordWithoutUsername,
            Some(SettingsFieldDto::BrokerUsername),
        ),
        // --- the namespace rules, which is where the C++ build failed ---
        (
            "empty discovery prefix — produced homeassistant//cover/1/config",
            |u| u.discovery_prefix = "".try_into().unwrap(),
            ApiErrorCode::ValueEmpty,
            Some(SettingsFieldDto::DiscoveryPrefix),
        ),
        (
            "empty state root — produced a payload base of /shades/1",
            |u| u.state_root = "".try_into().unwrap(),
            ApiErrorCode::ValueEmpty,
            Some(SettingsFieldDto::StateRoot),
        ),
        (
            "the two namespaces are equal — availability lands on HA's birth topic",
            |u| u.state_root = "homeassistant".try_into().unwrap(),
            ApiErrorCode::NamespacesOverlap,
            Some(SettingsFieldDto::StateRoot),
        ),
        (
            "the state root sits inside the discovery prefix",
            |u| u.state_root = "homeassistant/somfyrs".try_into().unwrap(),
            ApiErrorCode::NamespacesOverlap,
            Some(SettingsFieldDto::StateRoot),
        ),
        (
            "a multi-level wildcard",
            |u| u.state_root = "somfyrs/#".try_into().unwrap(),
            ApiErrorCode::TopicWildcard,
            Some(SettingsFieldDto::StateRoot),
        ),
        (
            "a single-level wildcard",
            |u| u.discovery_prefix = "home+assistant".try_into().unwrap(),
            ApiErrorCode::TopicWildcard,
            Some(SettingsFieldDto::DiscoveryPrefix),
        ),
        (
            "a leading slash",
            |u| u.state_root = "/somfyrs".try_into().unwrap(),
            ApiErrorCode::TopicLeadingSlash,
            Some(SettingsFieldDto::StateRoot),
        ),
        (
            "a trailing slash",
            |u| u.state_root = "somfyrs/".try_into().unwrap(),
            ApiErrorCode::TopicTrailingSlash,
            Some(SettingsFieldDto::StateRoot),
        ),
        (
            "an empty interior segment",
            |u| u.state_root = "somfyrs//shades".try_into().unwrap(),
            ApiErrorCode::TopicEmptySegment,
            Some(SettingsFieldDto::StateRoot),
        ),
        (
            "a character no topic segment may carry",
            |u| u.state_root = "somfy rs".try_into().unwrap(),
            ApiErrorCode::TopicIllegalCharacter,
            Some(SettingsFieldDto::StateRoot),
        ),
        (
            "a state root over the storage limit",
            |u| u.state_root = "s".repeat(33).as_str().try_into().unwrap(),
            ApiErrorCode::ValueTooLong,
            Some(SettingsFieldDto::StateRoot),
        ),
    ];

    for (what, tweak, code, field) in cases {
        let mut update = mqtt_update();
        tweak(&mut update);
        assert_eq!(
            update.to_settings(Some(&stored_mqtt())),
            Err(ApiErrorDto {
                code: *code,
                field: *field,
            }),
            "{what}",
        );
    }
}

#[test]
fn the_three_configurations_that_broke_the_cplusplus_build_are_all_refused() {
    // From the requirements spec's evidence table. Each was accepted there and
    // produced a device that looked like it had worked.
    let combinations = [
        // state root prepended to the discovery topic is not expressible here at
        // all — the two are separate fields — so what is left to check is that
        // the two degenerate namespaces cannot be stored.
        ("homeassistant", ""),
        ("", "homeassistant"),
        ("homeassistant", "homeassistant"),
    ];
    for (prefix, root) in combinations {
        let mut update = mqtt_update();
        update.discovery_prefix = prefix.try_into().unwrap();
        update.state_root = root.try_into().unwrap();
        assert!(
            update.to_settings(Some(&stored_mqtt())).is_err(),
            "discovery_prefix={prefix:?} state_root={root:?} was accepted",
        );
    }
}

#[test]
fn a_valid_pair_that_merely_shares_a_prefix_textually_is_accepted() {
    // The boundary is segment-wise, not textual: `home` is not inside
    // `homeassistant`. Refusing it would be repair by another name.
    let mut update = mqtt_update();
    update.discovery_prefix = "homeassistant".try_into().unwrap();
    update.state_root = "home".try_into().unwrap();
    assert!(update.to_settings(Some(&stored_mqtt())).is_ok());
}

// ---------------------------------------------------------------------------
// The error body
// ---------------------------------------------------------------------------

#[test]
fn a_rejection_that_is_not_about_a_field_serialises_exactly_as_it_always_did() {
    // Every response predating settings must be byte-identical, because the
    // `Refusal` writer measures its own content length.
    let json = serde_json::to_string(&ApiErrorDto::from(ApiErrorCode::NameTooLong)).unwrap();
    assert_eq!(json, r#"{"code":"nameTooLong"}"#);
}

#[test]
fn a_rejection_about_a_field_carries_it() {
    let json = serde_json::to_string(&ApiErrorDto::field(
        ApiErrorCode::ValueTooLong,
        SettingsFieldDto::StateRoot,
    ))
    .unwrap();
    assert_eq!(json, r#"{"code":"valueTooLong","field":"stateRoot"}"#);
}
