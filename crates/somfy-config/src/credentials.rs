//! Wi-Fi credentials, validated at the point they are constructed.
//!
//! The rules below are the access point's and the driver's, not this crate's
//! preferences. Each one describes a value that would be *accepted* somewhere
//! downstream and then silently fail to associate — and a device that never
//! joins a network looks exactly the same whether the passphrase is four
//! characters long or the router is switched off.

use core::fmt;

use heapless::String;

/// Longest SSID an 802.11 beacon can carry, in **bytes**.
///
/// The field in the frame is a length-prefixed byte string with a one-byte
/// length capped at 32 by the standard; it is not a character count, which is
/// why [`WifiCredentials::new`] measures `str::len` rather than `chars`.
pub const MAX_SSID_LEN: usize = 32;

/// Shortest WPA-PSK passphrase, from the standard's own key-derivation rule.
///
/// Access points refuse anything shorter, so a shorter value is a typo rather
/// than a configuration — and one worth naming at the moment it is entered.
pub const MIN_PSK_LEN: usize = 8;

/// Longest passphrase the Wi-Fi driver accepts.
///
/// 63 characters is the WPA-PSK passphrase limit and 64 is the raw hexadecimal
/// key, which the driver also takes; both fit, so both are allowed rather than
/// refusing a legitimate hex key on a rule about passphrases.
pub const MAX_PSK_LEN: usize = 64;

/// Which value a [`CredentialError`] is about.
///
/// Every error names one. "The Wi-Fi configuration is invalid" is the message
/// that made the C++ integration impossible to debug from the outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The network name.
    Ssid,
    /// The pre-shared key: a WPA passphrase, or empty on an open network.
    Psk,
}

impl Field {
    /// The field's name, for a message a person reads.
    pub const fn as_str(self) -> &'static str {
        match self {
            Field::Ssid => "ssid",
            Field::Psk => "psk",
        }
    }
}

/// Why a set of credentials was refused.
///
/// There is deliberately no variant meaning "accepted with adjustments". A
/// truncated SSID names a different network, and a padded passphrase is the
/// wrong passphrase; both would present as a device that cannot connect, with
/// nothing anywhere saying why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialError {
    /// The field is empty and may not be. Only the SSID reaches this: an empty
    /// passphrase is how an open network is expressed.
    Empty(Field),
    /// The field is longer than the protocol allows.
    TooLong {
        /// The field that was too long.
        field: Field,
        /// Its length in bytes.
        len: usize,
        /// The largest length that would have been accepted.
        limit: usize,
    },
    /// The field is shorter than the protocol allows. Only the passphrase
    /// reaches this, and only when it is not empty.
    TooShort {
        /// The field that was too short.
        field: Field,
        /// Its length in bytes.
        len: usize,
        /// The smallest non-empty length that would have been accepted.
        limit: usize,
    },
    /// The field contains a NUL byte. The driver hands both fields to a C API
    /// that terminates there, so the value that reaches the radio would be a
    /// silently shortened one.
    InteriorNul(Field),
}

impl CredentialError {
    /// The field this error is about.
    pub const fn field(self) -> Field {
        match self {
            CredentialError::Empty(field)
            | CredentialError::TooLong { field, .. }
            | CredentialError::TooShort { field, .. }
            | CredentialError::InteriorNul(field) => field,
        }
    }
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let field = self.field().as_str();
        match self {
            CredentialError::Empty(_) => write!(formatter, "{field} must not be empty"),
            CredentialError::TooLong { len, limit, .. } => write!(
                formatter,
                "{field} is {len} bytes; at most {limit} are allowed",
            ),
            CredentialError::TooShort { len, limit, .. } => write!(
                formatter,
                "{field} is {len} bytes; at least {limit} are needed",
            ),
            CredentialError::InteriorNul(_) => write!(
                formatter,
                "{field} contains a NUL byte, which the Wi-Fi driver would truncate at",
            ),
        }
    }
}

/// So the host-side provisioning tool can report one with `?`.
impl core::error::Error for CredentialError {}

/// One network's credentials, already known to be well-formed.
///
/// ## These are not secrets at rest
///
/// A `WifiCredentials` that has been persisted is readable by anyone holding
/// the board: flash is not encrypted here, and nothing in this crate pretends
/// otherwise. [`fmt::Debug`] redacts the passphrase so it does not reach a
/// serial console by accident, which is a different and much smaller claim.
#[derive(Clone, PartialEq, Eq)]
pub struct WifiCredentials {
    ssid: String<MAX_SSID_LEN>,
    psk: String<MAX_PSK_LEN>,
}

impl WifiCredentials {
    /// Check a network name and passphrase, or say which one is wrong.
    pub fn new(ssid: &str, psk: &str) -> Result<Self, CredentialError> {
        check(Field::Ssid, ssid, 1, MAX_SSID_LEN)?;
        // An empty passphrase is an open network, which is a configuration and
        // not an omission — so the minimum applies only once there is one.
        let psk_minimum = if psk.is_empty() { 0 } else { MIN_PSK_LEN };
        check(Field::Psk, psk, psk_minimum, MAX_PSK_LEN)?;

        Ok(Self {
            // Both `expect`s are unreachable: `check` has just bounded each
            // length by the capacity of the string it is copied into.
            ssid: String::try_from(ssid).expect("ssid length checked above"),
            psk: String::try_from(psk).expect("psk length checked above"),
        })
    }

    /// The network name.
    pub fn ssid(&self) -> &str {
        &self.ssid
    }

    /// The passphrase, empty on an open network.
    ///
    /// Named rather than exposed through `Debug` on purpose: a caller that
    /// wants the secret has to ask for it, so the ordinary debugging route
    /// cannot print it by accident.
    pub fn psk(&self) -> &str {
        &self.psk
    }

    /// Whether this network has no passphrase at all.
    pub fn is_open(&self) -> bool {
        self.psk.is_empty()
    }
}

/// Redacts the passphrase.
///
/// Not derived, and this is the only reason: every error path in the firmware
/// reports with `{:?}`, and a derived `Debug` would put the user's Wi-Fi
/// passphrase on the serial console the first time a network task logged a
/// failure. The SSID stays, because it is broadcast by the access point
/// several times a second anyway and it is the field worth seeing.
impl fmt::Debug for WifiCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WifiCredentials")
            .field("ssid", &self.ssid.as_str())
            .field(
                "psk",
                &if self.is_open() {
                    "<open>"
                } else {
                    "<redacted>"
                },
            )
            .finish()
    }
}

/// Bound one field's length and reject an embedded NUL.
fn check(field: Field, value: &str, minimum: usize, limit: usize) -> Result<(), CredentialError> {
    let len = value.len();
    if minimum > 0 && len == 0 {
        return Err(CredentialError::Empty(field));
    }
    if len > limit {
        return Err(CredentialError::TooLong { field, len, limit });
    }
    if len < minimum {
        return Err(CredentialError::TooShort {
            field,
            len,
            limit: minimum,
        });
    }
    if value.as_bytes().contains(&0) {
        return Err(CredentialError::InteriorNul(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The order matters: a value that is both too long *and* contains a NUL
    /// should report the length, because that is the one the operator can act
    /// on without re-reading the string byte by byte.
    #[test]
    fn a_length_failure_is_reported_before_a_nul() {
        let over = "s".repeat(MAX_SSID_LEN) + "\0";
        assert_eq!(
            WifiCredentials::new(&over, "passphrase"),
            Err(CredentialError::TooLong {
                field: Field::Ssid,
                len: MAX_SSID_LEN + 1,
                limit: MAX_SSID_LEN,
            })
        );
    }

    /// An open network's empty passphrase must not be read as "too short" —
    /// the minimum does not apply to it at all.
    #[test]
    fn the_passphrase_minimum_does_not_apply_to_an_open_network() {
        assert!(WifiCredentials::new("Guest", "").is_ok());
    }

    #[test]
    fn field_names_are_the_ones_an_operator_would_type() {
        assert_eq!(Field::Ssid.as_str(), "ssid");
        assert_eq!(Field::Psk.as_str(), "psk");
    }
}
