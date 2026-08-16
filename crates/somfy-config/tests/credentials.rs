//! What a set of Wi-Fi credentials must refuse, and why each refusal exists.
//!
//! Every case here is a value the Wi-Fi driver would *accept* and then fail to
//! act on, which is the failure mode this crate exists to convert into a named
//! error at the point of entry. A device that silently never associates is
//! indistinguishable from one out of range.

use somfy_config::{
    CredentialError, Field, WifiCredentials, MAX_PSK_LEN, MAX_SSID_LEN, MIN_PSK_LEN,
};

#[test]
fn a_wpa2_network_is_accepted_and_readable_back() {
    let credentials =
        WifiCredentials::new("example-network", "PLACEHOLDER_PASSPHRASE").expect("valid");
    assert_eq!(credentials.ssid(), "example-network");
    assert_eq!(credentials.psk(), "PLACEHOLDER_PASSPHRASE");
    assert!(!credentials.is_open());
}

/// An open network is a legitimate configuration, not a missing passphrase, so
/// it has to be expressible. The driver's own rule is the same: an empty
/// password means no authentication.
#[test]
fn an_open_network_has_an_empty_passphrase() {
    let credentials = WifiCredentials::new("Guest", "").expect("valid");
    assert!(credentials.is_open());
    assert_eq!(credentials.psk(), "");
}

/// An empty SSID passes the driver's validation and then never associates —
/// there is no such network to join. Refusing it names the field instead.
#[test]
fn an_empty_ssid_is_refused() {
    assert_eq!(
        WifiCredentials::new("", "passphrase"),
        Err(CredentialError::Empty(Field::Ssid))
    );
}

#[test]
fn an_ssid_at_the_limit_is_accepted_and_one_over_is_not() {
    let at_limit = "s".repeat(MAX_SSID_LEN);
    assert!(WifiCredentials::new(&at_limit, "passphrase").is_ok());

    let over = "s".repeat(MAX_SSID_LEN + 1);
    assert_eq!(
        WifiCredentials::new(&over, "passphrase"),
        Err(CredentialError::TooLong {
            field: Field::Ssid,
            len: MAX_SSID_LEN + 1,
            limit: MAX_SSID_LEN,
        })
    );
}

/// The 802.11 limit is on **bytes**, not characters, so a 32-character SSID of
/// multi-byte characters is over it. Counting characters here would produce an
/// SSID the driver truncates, which associates with nothing.
#[test]
fn the_ssid_limit_counts_bytes_not_characters() {
    let twenty_four_characters = "é".repeat(24);
    assert_eq!(twenty_four_characters.chars().count(), 24);
    assert_eq!(twenty_four_characters.len(), 48);
    assert_eq!(
        WifiCredentials::new(&twenty_four_characters, "passphrase"),
        Err(CredentialError::TooLong {
            field: Field::Ssid,
            len: 48,
            limit: MAX_SSID_LEN,
        })
    );
}

/// WPA-PSK passphrases are 8 to 63 characters; 64 is the raw hex key. Anything
/// shorter than 8 is refused by every access point, so a 4-character
/// passphrase is a typo the operator should hear about at once rather than a
/// device that retries forever.
#[test]
fn a_passphrase_shorter_than_the_wpa_minimum_is_refused() {
    assert_eq!(
        WifiCredentials::new("Net", "short"),
        Err(CredentialError::TooShort {
            field: Field::Psk,
            len: 5,
            limit: MIN_PSK_LEN,
        })
    );
}

#[test]
fn a_passphrase_at_each_limit_is_accepted() {
    assert!(WifiCredentials::new("Net", &"p".repeat(MIN_PSK_LEN)).is_ok());
    assert!(WifiCredentials::new("Net", &"p".repeat(MAX_PSK_LEN)).is_ok());
}

#[test]
fn a_passphrase_over_the_limit_is_refused() {
    let over = "p".repeat(MAX_PSK_LEN + 1);
    assert_eq!(
        WifiCredentials::new("Net", &over),
        Err(CredentialError::TooLong {
            field: Field::Psk,
            len: MAX_PSK_LEN + 1,
            limit: MAX_PSK_LEN,
        })
    );
}

/// The driver hands both fields to a C API that terminates on NUL. A value
/// containing one is silently truncated there — an SSID of `Home\0Office`
/// becomes `Home`, which is a different network, or none. It cannot be
/// repaired without guessing which half was meant, so it is refused.
#[test]
fn an_interior_nul_is_refused_in_either_field() {
    assert_eq!(
        WifiCredentials::new("Home\0Office", "passphrase"),
        Err(CredentialError::InteriorNul(Field::Ssid))
    );
    assert_eq!(
        WifiCredentials::new("Home", "pass\0phrase"),
        Err(CredentialError::InteriorNul(Field::Psk))
    );
}

/// The passphrase must not be printable through the ordinary debugging route.
/// Every error path in the firmware prints `{:?}`, and a credential that
/// renders itself over the serial line is a credential published to whoever is
/// watching the console.
#[test]
fn debug_redacts_the_passphrase_and_keeps_the_ssid() {
    let credentials = WifiCredentials::new("example-network", "PLACEHOLDER_SECRET").expect("valid");
    let rendered = format!("{:?}", credentials);
    assert!(rendered.contains("example-network"), "{rendered}");
    assert!(!rendered.contains("PLACEHOLDER_SECRET"), "{rendered}");
}
