//! The gate. Everything that reaches a topic passes through here first.
//!
//! Two shapes are validated:
//!
//! - a **root** ([`check_root`]) — one or more segments joined by single
//!   slashes, e.g. `homeassistant` or `home/blinds`;
//! - a **token** ([`check_token`]) — exactly one segment, e.g. a `node_id`.
//!
//! Both refuse rather than repair. There is no code path here that returns a
//! modified value: an input is either usable as configured or it is an error
//! naming its field. A validator that quietly fixes its input hides the
//! difference between a working configuration and a typo, which is how a
//! misconfigured device ends up looking healthy.

use crate::error::{ConfigError, Field};

/// The characters a topic segment may contain, per the discovery contract.
const fn is_segment_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

/// Single-level and multi-level MQTT wildcards. Legal in a subscription filter,
/// never in a topic name something publishes to.
fn wildcard(ch: char) -> bool {
    ch == '#' || ch == '+'
}

/// Validate a multi-segment root: `discovery_prefix` or `state_root`.
///
/// Checks run in the order an operator would want them reported: the bound that
/// makes everything else infallible, then emptiness, then the structural faults
/// in the order they appear in the string, then character legality, then
/// interior empty segments.
pub(crate) fn check_root(value: &str, field: Field, capacity: usize) -> Result<(), ConfigError> {
    if value.len() > capacity {
        return Err(ConfigError::TooLong(field, capacity));
    }
    if value.is_empty() {
        return Err(ConfigError::Empty(field));
    }
    if value.starts_with('/') {
        return Err(ConfigError::LeadingSlash(field));
    }
    if value.ends_with('/') {
        return Err(ConfigError::TrailingSlash(field));
    }
    for ch in value.chars() {
        if wildcard(ch) {
            return Err(ConfigError::Wildcard(field, ch));
        }
        if ch != '/' && !is_segment_char(ch) {
            return Err(ConfigError::IllegalCharacter(field, ch));
        }
    }
    if value.contains("//") {
        return Err(ConfigError::EmptySegment(field));
    }
    Ok(())
}

/// Validate a single-segment token: `node_id` or `device_id`.
///
/// A slash is not a separator here — a token becomes exactly one topic segment,
/// so a slash in it would silently add a level to the discovery topic and move
/// the component out of the position Home Assistant requires.
pub(crate) fn check_token(value: &str, field: Field, capacity: usize) -> Result<(), ConfigError> {
    if value.len() > capacity {
        return Err(ConfigError::TooLong(field, capacity));
    }
    if value.is_empty() {
        return Err(ConfigError::Empty(field));
    }
    for ch in value.chars() {
        if wildcard(ch) {
            return Err(ConfigError::Wildcard(field, ch));
        }
        if !is_segment_char(ch) {
            return Err(ConfigError::IllegalCharacter(field, ch));
        }
    }
    Ok(())
}

/// True if `value` is exactly one legal topic segment.
///
/// Used by [`crate::Topic`]'s builder to assert, in debug builds, that nothing
/// multi-segment reaches a single-segment push. See that module for why a root
/// cannot get there in the first place.
pub(crate) fn is_segment(value: &str) -> bool {
    !value.is_empty() && value.chars().all(is_segment_char)
}
