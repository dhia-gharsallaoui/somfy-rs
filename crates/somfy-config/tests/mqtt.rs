//! The persisted MQTT settings, and what they refuse.
//!
//! Same posture as the Wi-Fi credentials next door: every rule rejects rather
//! than adjusts, and every rejection names its field. The failure being
//! prevented is the one the MQTT requirements were written from — a setting
//! that was accepted, published to an address nobody reads, and looked like it
//! had worked.

use core::net::Ipv4Addr;

use somfy_config::{
    MqttField, MqttSettings, MqttSettingsError, DEFAULT_DISCOVERY_PREFIX, DEFAULT_STATE_ROOT,
    MAX_BROKER_PASSWORD_LEN, MAX_BROKER_USERNAME_LEN, MAX_TOPIC_ROOT_LEN,
};

const BROKER: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 10);

fn settings(username: &str, password: &str) -> Result<MqttSettings, MqttSettingsError> {
    MqttSettings::new(
        BROKER,
        1883,
        username,
        password,
        DEFAULT_DISCOVERY_PREFIX,
        DEFAULT_STATE_ROOT,
    )
}

#[test]
fn a_well_formed_setting_is_accepted_and_reads_back_unchanged() {
    let mqtt = settings("somfy", "PLACEHOLDER_BROKER_PASSWORD").expect("valid");
    assert_eq!(mqtt.address(), BROKER);
    assert_eq!(mqtt.port(), 1883);
    assert_eq!(mqtt.username(), "somfy");
    assert_eq!(mqtt.password(), "PLACEHOLDER_BROKER_PASSWORD");
    assert_eq!(mqtt.discovery_prefix(), DEFAULT_DISCOVERY_PREFIX);
    assert_eq!(mqtt.state_root(), DEFAULT_STATE_ROOT);
    assert!(!mqtt.is_anonymous());
}

/// An anonymous broker is a configuration, not an omission — some brokers allow
/// it — so an empty username is accepted and reported as such.
#[test]
fn an_empty_username_means_anonymous() {
    let mqtt = settings("", "").expect("valid");
    assert!(mqtt.is_anonymous());
}

/// A password with no username is not anonymous and not authenticated: MQTT
/// permits the combination, and no broker this device will meet does anything
/// useful with it. It is a half-finished configuration, and the operator who
/// typed it meant to type a username.
#[test]
fn a_password_without_a_username_is_refused() {
    assert_eq!(
        settings("", "PLACEHOLDER_BROKER_PASSWORD"),
        Err(MqttSettingsError::PasswordWithoutUsername),
    );
}

/// Not a preference: nothing on this device is listening on its own loopback,
/// 0.0.0.0 is not an address to connect to, and a multicast or broadcast
/// address cannot terminate a TCP connection. Each one is a value an operator
/// can plausibly paste in from a desktop configuration, and each would present
/// as a broker that never connects with nothing saying why.
#[test]
fn an_address_no_tcp_connection_could_reach_is_refused() {
    for address in [
        Ipv4Addr::UNSPECIFIED,
        Ipv4Addr::LOCALHOST,
        Ipv4Addr::BROADCAST,
        Ipv4Addr::new(224, 0, 0, 1),
    ] {
        assert_eq!(
            MqttSettings::new(
                address,
                1883,
                "",
                "",
                DEFAULT_DISCOVERY_PREFIX,
                DEFAULT_STATE_ROOT,
            ),
            Err(MqttSettingsError::Unroutable(address)),
            "{address} must be refused",
        );
    }
}

#[test]
fn port_zero_is_refused() {
    assert_eq!(
        MqttSettings::new(
            BROKER,
            0,
            "",
            "",
            DEFAULT_DISCOVERY_PREFIX,
            DEFAULT_STATE_ROOT,
        ),
        Err(MqttSettingsError::PortZero),
    );
}

#[test]
fn each_field_is_bounded_and_the_error_names_it() {
    let long_user = "u".repeat(MAX_BROKER_USERNAME_LEN + 1);
    assert_eq!(
        settings(&long_user, "password"),
        Err(MqttSettingsError::TooLong {
            field: MqttField::Username,
            len: MAX_BROKER_USERNAME_LEN + 1,
            limit: MAX_BROKER_USERNAME_LEN,
        }),
    );

    let long_password = "p".repeat(MAX_BROKER_PASSWORD_LEN + 1);
    assert_eq!(
        settings("somfy", &long_password),
        Err(MqttSettingsError::TooLong {
            field: MqttField::Password,
            len: MAX_BROKER_PASSWORD_LEN + 1,
            limit: MAX_BROKER_PASSWORD_LEN,
        }),
    );

    let long_root = "r".repeat(MAX_TOPIC_ROOT_LEN + 1);
    assert_eq!(
        MqttSettings::new(BROKER, 1883, "", "", DEFAULT_DISCOVERY_PREFIX, &long_root),
        Err(MqttSettingsError::TooLong {
            field: MqttField::StateRoot,
            len: MAX_TOPIC_ROOT_LEN + 1,
            limit: MAX_TOPIC_ROOT_LEN,
        }),
    );
}

/// MQTT's own rule: a UTF-8 encoded string in a control packet must not carry
/// U+0000. A broker is entitled to close the connection over it, which reads
/// from this end as a broker that rejects the credentials.
#[test]
fn an_interior_nul_is_refused_in_the_credential_fields() {
    assert_eq!(
        settings("som\0fy", "password"),
        Err(MqttSettingsError::InteriorNul(MqttField::Username)),
    );
    assert_eq!(
        settings("somfy", "pass\0word"),
        Err(MqttSettingsError::InteriorNul(MqttField::Password)),
    );
}

/// The two namespaces go through `somfy-mqtt`'s own validators, so a record
/// cannot deliver a root the topic builder would refuse. Storing a bad one and
/// discovering it at boot would put the rejection three flashes away from the
/// person who typed it.
#[test]
fn the_two_roots_are_validated_by_the_topic_rules_that_will_use_them() {
    use somfy_mqtt::{ConfigError, Field};

    for (prefix, root, expected) in [
        (
            "",
            DEFAULT_STATE_ROOT,
            ConfigError::Empty(Field::DiscoveryPrefix),
        ),
        (
            DEFAULT_DISCOVERY_PREFIX,
            "",
            ConfigError::Empty(Field::StateRoot),
        ),
        (
            DEFAULT_DISCOVERY_PREFIX,
            "somfyrs/",
            ConfigError::TrailingSlash(Field::StateRoot),
        ),
        (
            DEFAULT_DISCOVERY_PREFIX,
            "/somfyrs",
            ConfigError::LeadingSlash(Field::StateRoot),
        ),
        (
            DEFAULT_DISCOVERY_PREFIX,
            "som//fyrs",
            ConfigError::EmptySegment(Field::StateRoot),
        ),
        (
            "home#assistant",
            DEFAULT_STATE_ROOT,
            ConfigError::Wildcard(Field::DiscoveryPrefix, '#'),
        ),
        (
            DEFAULT_DISCOVERY_PREFIX,
            "somfy+rs",
            ConfigError::Wildcard(Field::StateRoot, '+'),
        ),
    ] {
        assert_eq!(
            MqttSettings::new(BROKER, 1883, "", "", prefix, root),
            Err(MqttSettingsError::Topic(expected)),
            "({prefix:?}, {root:?})",
        );
    }
}

/// The cross-field rule, which is the one neither R3 nor R4 originally named:
/// two individually valid roots that name the same namespace put availability
/// on Home Assistant's own birth topic, so HA marks the device available while
/// it is offline. Refused here rather than at boot, for the same reason as
/// above.
#[test]
fn two_roots_naming_the_same_namespace_are_refused() {
    use somfy_mqtt::{ConfigError, Field};

    for (prefix, root) in [
        ("homeassistant", "homeassistant"),
        ("homeassistant", "homeassistant/somfyrs"),
        ("homeassistant/somfyrs", "homeassistant"),
    ] {
        assert_eq!(
            MqttSettings::new(BROKER, 1883, "", "", prefix, root),
            Err(MqttSettingsError::Topic(ConfigError::Overlap(
                Field::StateRoot
            ))),
            "({prefix:?}, {root:?})",
        );
    }
    // And the boundary: these two share a text prefix but not a namespace.
    assert!(MqttSettings::new(BROKER, 1883, "", "", "home", "homeassistant").is_ok());
}

/// The same reason `WifiCredentials` hand-writes its `Debug`: every error path
/// in the firmware reports with `{:?}`, and a derived one would put the
/// broker's password on the serial console the first time a connection failed.
#[test]
fn debug_redacts_the_password_and_keeps_everything_an_operator_needs() {
    let mqtt = settings("somfy", "PLACEHOLDER_BROKER_PASSWORD").expect("valid");
    let rendered = format!("{mqtt:?}");
    assert!(
        !rendered.contains("PLACEHOLDER_BROKER_PASSWORD"),
        "{rendered}"
    );
    assert!(rendered.contains("redacted"), "{rendered}");
    assert!(rendered.contains("192.0.2.10"), "{rendered}");
    assert!(rendered.contains("1883"), "{rendered}");
    assert!(rendered.contains("somfy"), "{rendered}");

    let anonymous = settings("", "").expect("valid");
    assert!(format!("{anonymous:?}").contains("anonymous"));
}

/// The defaults are Home Assistant's own prefix and a state root that is not it
/// — the pair the requirements say a device must ship with. A device that
/// forces the discovery prefix to be changed taxes every other MQTT device on
/// that network for as long as it is installed.
#[test]
fn the_defaults_are_a_valid_pair_and_the_prefix_is_home_assistants_own() {
    assert_eq!(DEFAULT_DISCOVERY_PREFIX, "homeassistant");
    assert!(MqttSettings::new(
        BROKER,
        1883,
        "",
        "",
        DEFAULT_DISCOVERY_PREFIX,
        DEFAULT_STATE_ROOT,
    )
    .is_ok());
}
