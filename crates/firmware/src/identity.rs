//! What this device calls itself on the network, and why it is not a choice.
//!
//! # One identity, two spellings
//!
//! This board already has a name: the factory eFuse MAC, hex-encoded without
//! separators, is what every Home Assistant `unique_id` is built from. That
//! choice was made because an identifier has to survive a reboot, a
//! configuration change and a firmware update — an entity whose `unique_id`
//! moves is a *new* entity, and the old one stays behind with every automation
//! still pointing at it.
//!
//! A hostname has exactly the same requirement, for the same reason and with a
//! worse failure: a bookmark to `http://<something>.local` that stops resolving
//! is indistinguishable from a device that has died. So this module does not
//! choose a second identifier. It takes the one that exists and prefixes it.
//!
//! **`crate::mqtt::device_id` computes the same twelve characters and should
//! call [`mac_hex`] instead.** It is left alone here only because that file is
//! being changed elsewhere; the values are identical, so the two cannot disagree
//! about *what* this device is, only about which function said so.
//!
//! # Why the whole MAC and not the last three bytes
//!
//! `somfy-a1b2c3d4e5f6` is a mouthful next to `somfy-d4e5f6`, and the short form
//! was the first instinct. It is wrong twice over. It would be a *different*
//! identifier from the MQTT one, which is precisely the second scheme this
//! module exists to avoid — an operator could no longer read the hostname off a
//! browser tab and match it against the device page in Home Assistant. And the
//! low three bytes of a MAC are the NIC-specific half only: two boards from one
//! production run share the OUI and differ in the tail, so the short form is not
//! obviously safer, merely shorter.
//!
//! # Why it is not configurable yet, and what changes when it is
//!
//! There is nowhere to put it. The persisted record lives in `somfy-config`, and
//! a hostname field there is a record-format change belonging with the rest of
//! Plan 6 Task 2's config store. Until then a derived name is strictly better
//! than a fixed one — the reference implementation ships a constant, which means
//! two of them on one network are two devices claiming the same address.
//!
//! **When it becomes configurable, two things arrive with it.** The validation
//! below stops being a proof and becomes a check on user input, and *conflict
//! probing* (RFC 6762 §8.1) stops being unnecessary. Both are noted where they
//! apply.

use heapless::String;

/// The fixed half of the name.
///
/// Not "esp" and not the product's full title: it is what an operator scanning a
/// router's client list has to recognise, and it is what precedes the part that
/// makes it unique.
const PREFIX: &str = "somfy";

/// Characters of MAC, hex-encoded: six bytes at two digits each.
const MAC_HEX_LEN: usize = 12;

/// Longest name this module can produce: `PREFIX`, a hyphen, and the MAC.
pub const HOSTNAME_LEN: usize = PREFIX.len() + 1 + MAC_HEX_LEN;

/// Whether `label` is a hostname label a browser will accept in a URL.
///
/// The letter-digit-hyphen rule of RFC 1035 §2.3.1 as relaxed by RFC 1123 §2.1:
/// at least one and at most 63 octets, `a`-`z`, `0`-`9` and `-`, and neither the
/// first nor the last character a hyphen.
///
/// mDNS itself is more permissive — RFC 6762 §16 allows any UTF-8 in a label —
/// and that permission is deliberately not taken. The name's job is to be typed
/// into an address bar.
///
/// A `const fn` because today its only caller is a compile-time assertion. When
/// the hostname becomes user-supplied this is the function that has to be called
/// at run time as well, and it is written to be callable both ways rather than
/// rewritten then.
const fn is_hostname_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    if bytes[0] == b'-' || bytes[bytes.len() - 1] == b'-' {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let allowed = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        if !allowed {
            return false;
        }
        index += 1;
    }
    true
}

// **The two guards, and between them they are a proof rather than a check.**
//
// `hostname()` below cannot be validated at run time in any useful way: there is
// no second name to fall back to and no user to tell. So the validity is
// established here instead, over the only two things that can vary.
//
// The first says the fixed half is a legal label on its own — which, since it
// neither starts nor ends with a hyphen, means `PREFIX-<anything legal>` is one
// too. The second bounds the total length. What is left is the MAC half, and
// that is legal by construction: `{:02x}` emits `0`-`9` and `a`-`f`, twelve of
// them, so it can neither be empty nor end in a hyphen.
//
// Both were confirmed to fire by breaking them: `PREFIX = "somfy-"` fails the
// first, `HOSTNAME_LEN` written as 64 fails the second.
const _: () = assert!(
    is_hostname_label(PREFIX),
    "the fixed half of the hostname must itself be a legal DNS label, or \
     `somfy-<mac>` is not one either: see identity::is_hostname_label",
);
const _: () = assert!(
    HOSTNAME_LEN <= 63,
    "a DNS label is at most 63 octets; see identity::is_hostname_label",
);

/// The factory MAC, hex-encoded without separators.
///
/// Not a secret: it is in the clear in every frame the Wi-Fi radio transmits.
/// Not derived from anything a user can edit, which is the property that makes
/// it usable as an identity at all.
pub fn mac_hex() -> String<MAC_HEX_LEN> {
    use core::fmt::Write as _;

    let mut out = String::new();
    for byte in esp_hal::efuse::base_mac_address().as_bytes() {
        // Cannot fail: six bytes at two hex digits each is exactly the capacity.
        // `write!` rather than a lookup table because a truncated identifier is
        // two devices silently sharing one.
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// This device's `.local` hostname, without the `.local`.
///
/// `edge-mdns` appends the domain itself, so what it wants is the single label.
pub fn hostname() -> String<HOSTNAME_LEN> {
    let mut out = String::new();
    // Neither push can fail: `HOSTNAME_LEN` is the sum of exactly these three
    // pieces, asserted above.
    let _ = out.push_str(PREFIX);
    let _ = out.push('-');
    let _ = out.push_str(&mac_hex());
    out
}
