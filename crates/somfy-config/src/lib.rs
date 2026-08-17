//! # somfy-config
//!
//! The device's persisted configuration — Wi-Fi credentials and MQTT settings
//! — as pure data: validation rules and the bytes one flash slot holds. No
//! flash I/O, no network, no hardware.
//!
//! The two halves are independent: a record can carry a network and no broker,
//! a broker and no network, both, or neither. Each of those is a configuration
//! an operator can mean, and none of them is an error — a board with neither is
//! what a freshly flashed device looks like, and it still receives and decodes.
//!
//! ## This is a stopgap, and it is one on purpose
//!
//! Plan 6 replaces it with the real configuration store. It exists now because
//! the alternative is worse: without any persisted config, the firmware boots
//! with no network and no shades, so **every part of Plan 5 would be
//! unobservable** and "it compiles" would be the only standard available. This
//! project has not accepted that standard anywhere else.
//!
//! The precedent is `somfy-store`: its Plan 4 implementation is a minimal
//! flash region behind a trait, and the seam and its guarantees survive into
//! Plan 6 even though the implementation will not. Same shape here — the
//! validation rules and the refusal-over-repair posture are the parts meant to
//! last.
//!
//! ## What is stored is not a secret
//!
//! The Wi-Fi passphrase and the broker password are written to flash in the
//! clear. Flash encryption is not enabled on this device, so **anyone who can
//! hold the board can read both of them off it**, with `espflash read-flash`
//! and nothing else.
//!
//! That is stated rather than mitigated. An obfuscation scheme here would need
//! its key in the same flash, so it would protect nothing and would change
//! only how safe the reader felt — which is worse than the honest version,
//! because a stated limitation can be weighed and a false assurance cannot.
//! The only real fix is ESP32 flash encryption with the key in eFuse, and that
//! is a device-provisioning decision the owner makes, not a library one.
//!
//! [`WifiCredentials`] and [`MqttSettings`] both redact their secret from
//! [`core::fmt::Debug`], which is a much smaller and different claim: it stops
//! the secret reaching a serial console through the ordinary `{:?}` error
//! path. It is not protection at rest.
//!
//! ## The posture: refuse, never repair
//!
//! Every rule below rejects rather than adjusts. A truncated SSID names a
//! different network; a padded passphrase is the wrong passphrase. Both
//! present as a device that will not connect, with nothing anywhere saying
//! why — which is the same failure the MQTT requirements were written to stop.
//!
//! ## Example
//!
//! ```
//! use core::net::Ipv4Addr;
//! use somfy_config::{
//!     ConfigRecord, MqttSettings, WifiCredentials, DEFAULT_DISCOVERY_PREFIX, DEFAULT_STATE_ROOT,
//! };
//!
//! let wifi = WifiCredentials::new("example-network", "PLACEHOLDER_PASSPHRASE")
//!     .expect("a valid credential");
//! let mqtt = MqttSettings::new(
//!     Ipv4Addr::new(192, 0, 2, 10),
//!     1883,
//!     "somfy",
//!     "PLACEHOLDER_BROKER_PASSWORD",
//!     DEFAULT_DISCOVERY_PREFIX,
//!     DEFAULT_STATE_ROOT,
//! )?;
//! let record = ConfigRecord { seq: 0, wifi: Some(wifi), mqtt: Some(mqtt) };
//!
//! // The bytes one flash slot holds, and back again.
//! assert_eq!(ConfigRecord::decode(&record.encode()), Ok(record));
//! # Ok::<(), somfy_config::MqttSettingsError>(())
//! ```

#![cfg_attr(not(test), no_std)]

mod credentials;
mod mqtt;
mod record;
mod shade;

pub use credentials::{
    CredentialError, Field, WifiCredentials, MAX_PSK_LEN, MAX_SSID_LEN, MIN_PSK_LEN,
};
pub use mqtt::{
    MqttField, MqttSettings, MqttSettingsError, Namespaces, DEFAULT_DISCOVERY_PREFIX,
    DEFAULT_STATE_ROOT, MAX_BROKER_PASSWORD_LEN, MAX_BROKER_USERNAME_LEN, MAX_TOPIC_ROOT_LEN,
};
pub use record::{ConfigRecord, RecordError, CONFIG_RECORD_LEN};
pub use shade::{
    ShadeError, ShadeHeader, ShadeRecord, ShadeRecordError, StoredShade, TravelField,
    SHADE_RECORD_LEN, SHADE_TABLE_CAPACITY,
};
