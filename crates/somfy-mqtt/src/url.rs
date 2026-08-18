//! Where a person goes to configure this controller, as Home Assistant is told
//! it.
//!
//! # What this is for
//!
//! Home Assistant's device registry carries a `configuration_url`, and its
//! device page renders it as a link. Every well-behaved MQTT integration sets
//! it — it is how an operator reaches the device's own settings from the
//! integration that represents it, instead of remembering an address.
//!
//! That matters here more than it does for most devices, because **adding a
//! shade is a guided procedure that Home Assistant cannot express.** Pairing
//! needs a person at the motor, a remote this controller is not, a two-minute
//! window nothing here opens, and an answer to "did it move?" that only a human
//! can give. A link into the assistant that walks all four is worth more than
//! any number of entities that reproduce its state machine without its
//! instructions. `docs/provenance.md` records that ruling and what would
//! reverse it.
//!
//! # Why it is validated rather than passed through
//!
//! Because Home Assistant validates it too, and **it fails the whole payload
//! rather than the field**. `homeassistant/components/mqtt/schemas.py` runs the
//! device block through `cv.configuration_url`, which parses the URL, refuses
//! any scheme outside `http`, `https` and `homeassistant`, and refuses a
//! malformed authority. A `vol.Invalid` there discards the entire discovery
//! config — so a bad URL does not produce an entity with a dead link, it
//! produces **no entity at all**, silently, which is precisely the failure mode
//! this crate exists to remove.
//!
//! So the value is refused here, at the point of entry, with a typed error
//! naming the rule it broke — exactly as the two namespace roots are.
//!
//! # Why it has its own error type rather than joining [`ConfigError`]
//!
//! [`ConfigError`](crate::ConfigError) and [`Field`](crate::Field) are the
//! *settings* vocabulary: `somfy-api` maps every variant of them onto a wire
//! error code, so that a screen can highlight the field an operator typed
//! wrongly. A configuration URL is not one of those. The firmware derives it
//! from its own hostname, it is not an input to `MqttSettings`, and no request
//! can carry a bad one — so widening `ConfigError` would oblige `somfy-api` to
//! publish two wire codes no route can ever return, or to map them onto a code
//! that describes something else.
//!
//! A separate type costs one `match` in this crate and buys an API surface with
//! nothing dead in it. **What would change this:** the hostname becoming
//! operator-configurable, which `crates/firmware/src/identity.rs` records as
//! Plan 6 work — at that point the URL *is* a settings field, and the two error
//! types should be reconsidered together with it.
//!
//! # Why `homeassistant://` is refused even though Home Assistant accepts it
//!
//! It addresses a page *inside* Home Assistant. This controller has no page
//! there: it is not an integration, it is an MQTT device, and the thing it
//! wants to point at is its own web server. Admitting a scheme nothing here can
//! ever produce would widen the rule for no caller and leave a reader of it
//! wondering which case it was for.

use core::fmt;

use heapless::String;

/// Bytes a configuration URL may occupy.
///
/// Generous rather than derived, in the same spirit as
/// [`MAX_DISCOVERY_PREFIX_LEN`](crate::MAX_DISCOVERY_PREFIX_LEN): what matters
/// is that a bound exists, so that the payload budget in `entity.rs` can be
/// proven at compile time.
///
/// It is not unlimited, and the trade is worth naming. `https://` and a maximal
/// 63-octet DNS label and `.local` would be 77 bytes, which this refuses. The
/// firmware's own hostname is `somfy-` and twelve hex digits of MAC — 18 bytes,
/// pinned by an assertion in `crates/firmware/src/identity.rs` — so the URL it
/// builds is 31, and a refusal is a typed error rather than a truncation if
/// that ever stops being true.
///
/// **Raising it costs payload capacity, and the room left is 17 bytes.**
/// Measured rather than estimated: at 81 the cover-payload assertion at the
/// foot of `entity.rs` still holds and at 82 it fires. Going past that is a
/// decision about [`PAYLOAD_CAPACITY`](crate::PAYLOAD_CAPACITY), which is a
/// buffer the firmware holds one of, in the DRAM its heap is carved from.
pub const MAX_CONFIGURATION_URL_LEN: usize = 64;

/// The prefixes a configuration URL may start with.
///
/// Home Assistant's `CONFIGURATION_URL_PROTOCOL_SCHEMA_LIST` also admits
/// `homeassistant`; this does not, and the module docs say why.
const SCHEMES: [&str; 2] = ["http://", "https://"];

/// Why a configuration URL was refused.
///
/// There is no variant meaning "accepted with adjustments", for the same reason
/// [`ConfigError`](crate::ConfigError) has none: a silently repaired address is
/// indistinguishable, from the operator's side, from a device that is broken.
/// Here it would be worse than that — Home Assistant discards the whole payload
/// rather than the field, so the symptom of a repaired-and-still-wrong URL is
/// an entity that never appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    /// There is no URL. Not the same thing as having no link: a device with
    /// nothing to point at omits the key entirely, which is
    /// [`MqttConfig::with_configuration_url`](crate::MqttConfig::with_configuration_url)
    /// never being called rather than being called with an empty string.
    Empty,
    /// Longer than [`MAX_CONFIGURATION_URL_LEN`], whose value is the payload so
    /// a caller can say what the limit is.
    TooLong(usize),
    /// The scheme is not one this crate emits. See the module docs for why
    /// `homeassistant://` is among the refusals.
    UnsupportedScheme,
    /// Nothing between the scheme and the path. `http:///shades` parses to a
    /// URL with no host, and Home Assistant refuses the whole payload for it.
    MissingAuthority,
    /// A character RFC 3986 does not permit in a URI.
    IllegalCharacter(char),
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UrlError::Empty => f.write_str("configuration_url must not be empty"),
            UrlError::TooLong(limit) => {
                write!(f, "configuration_url must be at most {limit} bytes")
            }
            UrlError::UnsupportedScheme => {
                f.write_str("configuration_url must start with 'http://' or 'https://'")
            }
            UrlError::MissingAuthority => {
                f.write_str("configuration_url must name a host after its scheme")
            }
            UrlError::IllegalCharacter(c) => write!(
                f,
                "configuration_url must not contain {c:?}; allowed: the characters RFC 3986 \
                 permits in a URI",
            ),
        }
    }
}

/// A URL a person can open to configure this controller.
///
/// Carried in every discovery payload's `device` block, so Home Assistant's
/// device page links to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationUrl(String<MAX_CONFIGURATION_URL_LEN>);

impl ConfigurationUrl {
    /// Validate and store a configuration URL, or say which rule it broke.
    ///
    /// ```
    /// use somfy_mqtt::{ConfigurationUrl, UrlError};
    ///
    /// assert!(ConfigurationUrl::new("http://somfy-a1b2c3d4e5f6.local").is_ok());
    /// assert_eq!(
    ///     ConfigurationUrl::new("ftp://somfy.local"),
    ///     Err(UrlError::UnsupportedScheme),
    /// );
    /// assert_eq!(
    ///     ConfigurationUrl::new("http://"),
    ///     Err(UrlError::MissingAuthority),
    /// );
    /// ```
    pub fn new(value: &str) -> Result<ConfigurationUrl, UrlError> {
        if value.len() > MAX_CONFIGURATION_URL_LEN {
            return Err(UrlError::TooLong(MAX_CONFIGURATION_URL_LEN));
        }
        if value.is_empty() {
            return Err(UrlError::Empty);
        }
        let Some(rest) = SCHEMES
            .into_iter()
            .find_map(|scheme| value.strip_prefix(scheme))
        else {
            return Err(UrlError::UnsupportedScheme);
        };
        // Everything up to the first `/`, `?` or `#` is the authority, and
        // Python's `urlparse` — which is what Home Assistant validates with —
        // treats an empty one as a URL with no host. `vol.Url()` then rejects
        // it, and the rejection takes the entity with it.
        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .expect("split always yields at least one piece");
        if authority.is_empty() {
            return Err(UrlError::MissingAuthority);
        }
        // The characters RFC 3986 permits in a URI, and nothing else. Chosen
        // for two reasons at once: a byte outside this set is not a URL Home
        // Assistant will parse, **and** the set excludes `"`, `\`, every
        // control character and the space, so a validated URL cannot expand
        // under the payload's JSON escaper. The capacity proof counts it at one
        // byte per byte, and that is why it may.
        if let Some(ch) = value.chars().find(|ch| !is_uri_char(*ch)) {
            return Err(UrlError::IllegalCharacter(ch));
        }
        let mut inner = String::new();
        // Cannot fail: the length was checked above.
        let _ = inner.push_str(value);
        Ok(ConfigurationUrl(inner))
    }

    /// The URL, for the discovery payload's device block.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `ch` is a character RFC 3986 permits unescaped in a URI: the
/// unreserved set, the reserved set, and `%` for percent-encoding.
const fn is_uri_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            // unreserved
            '-' | '.' | '_' | '~'
            // gen-delims
            | ':' | '/' | '?' | '#' | '[' | ']' | '@'
            // sub-delims
            | '!' | '$' | '&' | '\'' | '(' | ')' | '*' | '+' | ',' | ';' | '='
            // the escape marker itself
            | '%'
        )
}

// The set above is what lets `WORST_DEVICE_BLOCK_LEN` count a URL at one byte
// per byte. Stated as an assertion rather than as a sentence, because the claim
// is about a function and a sentence cannot be re-checked when the function
// changes: no character the validator admits has a JSON escape.
const _: () = {
    let mut code = 0u32;
    while code < 128 {
        // Every admitted character is printable ASCII outside `"` and `\`, and
        // those are the only two single-byte inputs the escaper expands.
        let ch = match char::from_u32(code) {
            Some(ch) => ch,
            None => break,
        };
        assert!(
            !is_uri_char(ch) || (code >= 0x21 && code != 0x22 && code != 0x5C && code < 0x7F),
            "a configuration URL may now contain a character the JSON escaper expands, so \
             `WORST_DEVICE_BLOCK_LEN` no longer bounds the rendered payload",
        );
        code += 1;
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Anything Home Assistant's own validator would refuse is refused here,
    /// and each refusal names its own rule rather than a generic one.
    ///
    /// The stake is higher than a dead link. `cv.configuration_url` runs over
    /// the whole device block, and a `vol.Invalid` there discards the entire
    /// discovery payload — so a URL this crate let through and Home Assistant
    /// did not would produce no entity at all, silently.
    #[test]
    fn every_rule_has_its_own_refusal() {
        assert_eq!(ConfigurationUrl::new(""), Err(UrlError::Empty));
        for value in [
            "somfy.local",
            "ftp://somfy.local",
            "//somfy.local",
            "http:/somfy.local",
            // Home Assistant admits this scheme; this crate does not, because
            // this controller has no page inside Home Assistant to address.
            "homeassistant://config/integrations",
        ] {
            assert_eq!(
                ConfigurationUrl::new(value),
                Err(UrlError::UnsupportedScheme),
                "{value:?}",
            );
        }
        for value in ["http://", "https://", "https:///shades"] {
            assert_eq!(
                ConfigurationUrl::new(value),
                Err(UrlError::MissingAuthority),
                "{value:?}",
            );
        }
        // Each of these is a character the JSON escaper would expand, a control
        // character, or non-ASCII — the three shapes the capacity proof assumes
        // a stored URL cannot contain.
        for (value, ch) in [
            ("http://somfy .local", ' '),
            ("http://somfy\".local", '"'),
            ("http://somfy\\x.local", '\\'),
            ("http://somfy\u{7f}.local", '\u{7f}'),
            ("http://caf\u{e9}.local", '\u{e9}'),
        ] {
            assert_eq!(
                ConfigurationUrl::new(value),
                Err(UrlError::IllegalCharacter(ch)),
                "{value:?}",
            );
        }
        let long = "http://".to_string() + &"a".repeat(MAX_CONFIGURATION_URL_LEN);
        assert_eq!(
            ConfigurationUrl::new(&long),
            Err(UrlError::TooLong(MAX_CONFIGURATION_URL_LEN)),
        );
    }

    /// Every refusal explains itself in words an operator reading a serial
    /// console can act on. The firmware prints one and carries on without a
    /// link rather than panicking, so the message is the whole report.
    #[test]
    fn every_refusal_explains_itself() {
        for (error, expected) in [
            (UrlError::Empty, "must not be empty"),
            (UrlError::TooLong(64), "at most 64 bytes"),
            (UrlError::UnsupportedScheme, "http://"),
            (UrlError::MissingAuthority, "host"),
            (UrlError::IllegalCharacter(' '), "RFC 3986"),
        ] {
            let text = format!("{error}");
            assert!(text.contains(expected), "{error:?} rendered as {text:?}");
        }
    }

    /// The shapes the firmware actually builds, and the ones an operator might
    /// reasonably configure by hand later.
    #[test]
    fn real_urls_are_accepted_unchanged() {
        for url in [
            "http://somfy-a1b2c3d4e5f6.local",
            "http://somfy-a1b2c3d4e5f6.local/",
            "https://192.0.2.10",
            "http://192.0.2.10:8080/shades/new",
        ] {
            let held = ConfigurationUrl::new(url).expect("a usable URL");
            assert_eq!(held.as_str(), url, "a URL must be stored verbatim");
        }
    }
}
