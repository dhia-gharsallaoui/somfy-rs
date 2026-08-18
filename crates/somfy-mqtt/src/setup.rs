//! The add-a-shade form: nine entities, eight of which exist only while a setup
//! is running.
//!
//! # Why this is not the button that was refused
//!
//! This crate's own docs used to say adding a shade could not be reached from
//! Home Assistant, and the reasoning is preserved in `docs/provenance.md`
//! because three quarters of it still holds. What fell over is the fourth
//! quarter, in two places:
//!
//! - **The entities need not be always-present.** `announce_shade` and
//!   `retire_shade` already add and remove entities mid-session, so the form is
//!   announced when a setup starts and retired when it ends. An idle controller
//!   carries exactly one extra entity — [`SetupEntity::Begin`] — and the other
//!   eight do not exist.
//! - **The instructions fit.** A `sensor`'s *state* string holds 255 characters
//!   (`homeassistant/const.py:61`), which is room for the sentence that decides
//!   whether a pairing works at all: hold `PROG` on the shade's existing remote
//!   until it jogs, then press Send pairing within about two minutes.
//!   [`SetupMessage`] is that text, and every variant is asserted under the
//!   limit at compile time — because a sensor with no `device_class` runs its
//!   payload through Home Assistant's `check_state_too_long`
//!   (`components/mqtt/sensor.py:337-342`), which falls back to `unknown` and
//!   loses the message rather than truncating it.
//!
//! What has **not** changed is the reason a bare button was wrong: it can only
//! create a shade with a generated name and the factory travel times, and those
//! are the values behind a shade that moved about 1% when it was asked for 25%.
//! The whole point of a form is that those numbers are chosen. So
//! [`SetupEntity::TravelUp`] and [`SetupEntity::TravelDown`] start with **no
//! state at all** — Home Assistant shows an empty box — and the flow refuses to
//! create a shade until both have been filled in.
//!
//! # Why these are device-level
//!
//! Because a shade's entity identity is `(device, component, shade id)`:
//! [`ObjectId::for_shade`](crate::ObjectId::for_shade) yields `shade_5` and
//! [`UniqueId::for_shade`](crate::UniqueId::for_shade) yields
//! `{device}_button_5`, so a second per-shade `button` overwrites the first's
//! retained config *and* its Home Assistant identity. Four of the nine here are
//! buttons. Device-level slugs do not collide, and each of these is a literal
//! in `[a-z_]` beginning `setup_`, so it can meet neither a shade's `shade_N`
//! nor a diagnostic's bare slug.
//!
//! The form is also genuinely about the *controller* rather than about any
//! shade: while it is up, the shade it is creating either does not exist yet or
//! is one no motor has been reported to obey.

use crate::config;
use crate::entity::{
    write, write_device_block, write_json_string, write_object_id, Component, PayloadError,
    PAYLOAD_CAPACITY, WORST_COMMON_LEN,
};
use crate::ident::{ObjectId, UniqueId};
use crate::topic::Topic;
use core::fmt::Write as _;
use heapless::String;
use somfy_domain::{ShadeKind, MAX_TRAVEL_TIME_MS};

/// Characters a Home Assistant entity state may hold.
///
/// `MAX_LENGTH_STATE_STATE` in `homeassistant/const.py:61`. A `sensor` with no
/// `device_class` — which [`SetupEntity::NextStep`] is — passes its payload
/// through `check_state_too_long` (`components/mqtt/util.py:377-396`), which on
/// an over-long value logs a warning and sets the entity to `unknown`. **The
/// message is lost, not shortened**, so this is a budget rather than a
/// guideline, and every [`SetupMessage`] is asserted against it below.
pub const MAX_STATE_LEN: usize = 255;

/// Longest name the form will carry to the shade table, in bytes.
///
/// The same 32 `somfy_domain::ShadeConfig::name` holds. It is published as the
/// text entity's `max`, which Home Assistant enforces in *characters* — so a
/// 32-character name of accented letters passes there and is refused here. That
/// asymmetry is why the flow checks the byte length itself rather than trusting
/// the entity's own limit.
pub const MAX_DRAFT_NAME_LEN: usize = 32;

/// What every form control is filed under on Home Assistant's device page.
///
/// `entity_category` is validated once for every platform, in
/// `MQTT_ENTITY_COMMON_SCHEMA` (`components/mqtt/schemas.py:181`), with no
/// per-platform restriction — so `config` is as valid on the instructions
/// sensor as on the buttons, and using it for all nine is what puts the whole
/// form in **one** card instead of splitting the instructions into Diagnostic
/// and the controls into Configuration.
///
/// It is also what keeps four buttons that create shades and transmit `Prog`
/// off an auto-generated dashboard, which is the same argument
/// [`ButtonDiscovery`](crate::ButtonDiscovery) already makes for the per-shade
/// pairing button.
const SETUP_CATEGORY: &str = "config";

/// One entity of the add-a-shade form.
///
/// # The two halves, and why the split is the whole design
///
/// [`SetupEntity::Begin`] is always announced; [`SetupEntity::FORM`] is
/// announced when a setup starts and retired when it ends. A discarded or
/// finished setup must leave **no** retained config behind — spec R5, written
/// after 49 retained topics were deleted by hand — and the mechanism is that
/// [`SetupEntity::FORM`] is read by both the announcement and the retirement,
/// exactly as [`SHADE_COMPONENTS`](crate::SHADE_COMPONENTS) and
/// [`DeviceEntity::ALL`](crate::DeviceEntity::ALL) are. A variant added here
/// joins both sides at once and cannot be announced without being removable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupEntity {
    /// Start a setup. **The one entity that is always there.**
    Begin,
    /// The name the shade will be given.
    Name,
    /// Which kind of shade it is.
    Kind,
    /// How long a full travel upward takes, in milliseconds.
    TravelUp,
    /// How long a full travel downward takes, in milliseconds.
    TravelDown,
    /// What to do next, in words. See [`SetupMessage`].
    NextStep,
    /// Create the shade if it does not exist yet, then transmit `Prog` at it.
    Send,
    /// **The operator's report that the motor moved.** Not an observation: RTS
    /// is one-way and this controller cannot see a motor. See
    /// `somfy_domain::PairingState`, which is named after whose knowledge it
    /// is.
    Confirm,
    /// Abandon the setup, removing the shade if one was created.
    Discard,
}

impl SetupEntity {
    /// Every entity the form can own, announced or not.
    pub const ALL: [SetupEntity; 9] = [
        SetupEntity::Begin,
        SetupEntity::Name,
        SetupEntity::Kind,
        SetupEntity::TravelUp,
        SetupEntity::TravelDown,
        SetupEntity::NextStep,
        SetupEntity::Send,
        SetupEntity::Confirm,
        SetupEntity::Discard,
    ];

    /// The entity announced with the device's own diagnostics and never retired
    /// while the configuration stands.
    pub const ALWAYS: [SetupEntity; 1] = [SetupEntity::Begin];

    /// The entities a running setup owns.
    ///
    /// Read by both halves of the form's lifecycle — see the type's docs.
    pub const FORM: [SetupEntity; 8] = [
        SetupEntity::Name,
        SetupEntity::Kind,
        SetupEntity::TravelUp,
        SetupEntity::TravelDown,
        SetupEntity::NextStep,
        SetupEntity::Send,
        SetupEntity::Confirm,
        SetupEntity::Discard,
    ];

    /// Bytes the longest [`slug`](SetupEntity::slug) occupies.
    pub const MAX_SLUG_LEN: usize = longest_slug();

    /// Bytes the longest [`leaf`](SetupEntity::leaf) occupies.
    pub const MAX_LEAF_LEN: usize = longest_leaf();

    /// Bytes the longest [`label`](SetupEntity::label) occupies.
    pub const MAX_LABEL_LEN: usize = longest_label();

    /// The topic segment under `{state_root}/setup`.
    ///
    /// A firmware literal in `[a-z_]`, so R2's character class holds by
    /// construction. It carries **no** `setup_` prefix because the segment
    /// above it already says `setup`; the prefix belongs to the object id,
    /// where there is no such segment to lean on.
    pub const fn leaf(self) -> &'static str {
        match self {
            SetupEntity::Begin => "begin",
            SetupEntity::Name => "name",
            SetupEntity::Kind => "kind",
            SetupEntity::TravelUp => "travel_up",
            SetupEntity::TravelDown => "travel_down",
            SetupEntity::NextStep => "next_step",
            SetupEntity::Send => "send",
            SetupEntity::Confirm => "confirm",
            SetupEntity::Discard => "discard",
        }
    }

    /// The `object_id` topic segment, and the suffix of the `unique_id`.
    ///
    /// Prefixed, so it can collide with neither a shade's `shade_N` nor a
    /// [`DeviceEntity`](crate::DeviceEntity)'s bare slug however either set
    /// grows. `tests/setup_form.rs` checks that against both.
    pub const fn slug(self) -> &'static str {
        match self {
            SetupEntity::Begin => "setup_begin",
            SetupEntity::Name => "setup_name",
            SetupEntity::Kind => "setup_kind",
            SetupEntity::TravelUp => "setup_travel_up",
            SetupEntity::TravelDown => "setup_travel_down",
            SetupEntity::NextStep => "setup_next_step",
            SetupEntity::Send => "setup_send",
            SetupEntity::Confirm => "setup_confirm",
            SetupEntity::Discard => "setup_discard",
        }
    }

    /// What Home Assistant displays, after the device's own name.
    pub const fn label(self) -> &'static str {
        match self {
            SetupEntity::Begin => "Add shade",
            SetupEntity::Name => "New shade name",
            SetupEntity::Kind => "New shade kind",
            SetupEntity::TravelUp => "New shade travel up",
            SetupEntity::TravelDown => "New shade travel down",
            SetupEntity::NextStep => "Next step",
            SetupEntity::Send => "Send pairing",
            SetupEntity::Confirm => "It moved",
            SetupEntity::Discard => "Discard setup",
        }
    }

    /// Which Home Assistant component this entity is.
    pub const fn component(self) -> Component {
        match self {
            SetupEntity::Begin
            | SetupEntity::Send
            | SetupEntity::Confirm
            | SetupEntity::Discard => Component::Button,
            SetupEntity::Name => Component::Text,
            SetupEntity::Kind => Component::Select,
            SetupEntity::TravelUp | SetupEntity::TravelDown => Component::Number,
            SetupEntity::NextStep => Component::Sensor,
        }
    }

    /// Whether the firmware publishes a state for this entity.
    ///
    /// False for the four buttons, which have none: a press is an event, and a
    /// button that reported one would be reporting something Home Assistant
    /// already knows it did.
    pub const fn has_state(self) -> bool {
        !matches!(
            self,
            SetupEntity::Begin | SetupEntity::Send | SetupEntity::Confirm | SetupEntity::Discard
        )
    }

    /// Whether the firmware subscribes to a `.../set` topic for this entity.
    ///
    /// False for [`SetupEntity::NextStep`] alone: it is the one thing here the
    /// device tells the operator rather than the other way round. Home
    /// Assistant enforces the same shape from its side — `sensor` extends
    /// `MQTT_RO_SCHEMA` (`components/mqtt/config.py:31-37`), which has no
    /// `command_topic` at all, while the other eight extend `MQTT_RW_SCHEMA`
    /// (`:39-46`), which **requires** one.
    pub const fn accepts_command(self) -> bool {
        !matches!(self, SetupEntity::NextStep)
    }
}

const fn longest_slug() -> usize {
    let mut i = 0;
    let mut max = 0;
    while i < SetupEntity::ALL.len() {
        let len = SetupEntity::ALL[i].slug().len();
        if len > max {
            max = len;
        }
        i += 1;
    }
    max
}

const fn longest_leaf() -> usize {
    let mut i = 0;
    let mut max = 0;
    while i < SetupEntity::ALL.len() {
        let len = SetupEntity::ALL[i].leaf().len();
        if len > max {
            max = len;
        }
        i += 1;
    }
    max
}

const fn longest_label() -> usize {
    let mut i = 0;
    let mut max = 0;
    while i < SetupEntity::ALL.len() {
        let len = SetupEntity::ALL[i].label().len();
        if len > max {
            max = len;
        }
        i += 1;
    }
    max
}

/// The kinds [`SetupEntity::Kind`] offers, in the order they appear.
///
/// One per `somfy_domain::ShadeKind` variant the domain models, ordered by how
/// often a real estate holds one rather than by discriminant. The mapping runs
/// both ways through a `match` ([`kind_label`] and [`kind_from_label`]), so a
/// kind added to the domain fails to compile here rather than quietly dropping
/// out of the list. Home Assistant publishes the chosen option **verbatim** on
/// the command topic (`components/mqtt/select.py:162-166`) and logs an error
/// for anything not in `options` (`:124-136`), so these strings are a wire
/// format and not merely a display.
pub const KIND_OPTIONS: [ShadeKind; 7] = [
    ShadeKind::Roller,
    ShadeKind::Blind,
    ShadeKind::Shutter,
    ShadeKind::Awning,
    ShadeKind::DraperyLeft,
    ShadeKind::DraperyRight,
    ShadeKind::DraperyCenter,
];

/// What Home Assistant shows for one shade kind.
pub const fn kind_label(kind: ShadeKind) -> &'static str {
    match kind {
        ShadeKind::Roller => "Roller",
        ShadeKind::Blind => "Blind",
        ShadeKind::Shutter => "Shutter",
        ShadeKind::Awning => "Awning",
        ShadeKind::DraperyLeft => "Drapery (left)",
        ShadeKind::DraperyRight => "Drapery (right)",
        ShadeKind::DraperyCenter => "Drapery (centre)",
    }
}

/// The kind an option string names, or `None` if it names none.
///
/// Home Assistant will only ever send one of [`KIND_OPTIONS`] — it refuses
/// anything else before it reaches the broker — so `None` here means somebody
/// published to the topic by hand, and the caller ignores it rather than
/// guessing.
pub fn kind_from_label(label: &str) -> Option<ShadeKind> {
    KIND_OPTIONS
        .into_iter()
        .find(|kind| kind_label(*kind) == label)
}

const fn longest_kind_label() -> usize {
    let mut i = 0;
    let mut max = 0;
    while i < KIND_OPTIONS.len() {
        let len = kind_label(KIND_OPTIONS[i]).len();
        if len > max {
            max = len;
        }
        i += 1;
    }
    max
}

/// Bytes the longest option occupies.
pub const MAX_KIND_LABEL_LEN: usize = longest_kind_label();

/// The smallest travel time the form will accept, in milliseconds.
///
/// One rather than zero, because zero is what `somfy_api`'s
/// `checked_lift_times` refuses, and a `number` whose `min` admitted a value
/// the shade table would reject is a control that accepts input and fails
/// behind the operator's back. Home Assistant enforces it from its side too: an
/// inbound value outside `min..=max` is logged at error and dropped
/// (`components/mqtt/number.py:189-199`).
pub const MIN_TRAVEL_MS: u32 = 1;

/// The largest, which is `somfy_domain::MAX_TRAVEL_TIME_MS` — three minutes.
pub const MAX_TRAVEL_MS: u32 = MAX_TRAVEL_TIME_MS;

/// The granularity Home Assistant's number box steps by.
///
/// A tenth of a second. Travel times are read off a stopwatch or a phone and
/// nobody times a shade to the millisecond; a step of 1 would give the box
/// 180,000 positions. `step` must be at least 1e-3
/// (`components/mqtt/number.py:96-98`), and it does **not** constrain what the
/// device may publish back — so a measured 10,437 ms from the guided
/// calibration still displays exactly.
pub const TRAVEL_STEP_MS: u32 = 100;

/// What the form says to do next.
///
/// # Why this is a sensor's state and not an entity name
///
/// Because the procedure has a step in it that no name can hold: the motor must
/// be put into programming mode by a remote **this controller is not**, and the
/// window that opens is about two minutes long. That sentence is the difference
/// between a pairing that works and one that silently does nothing, and it is
/// ninety-odd characters. A state string holds 255.
///
/// Every variant is asserted against [`MAX_STATE_LEN`] at compile time below.
/// The check is not decorative: over the limit Home Assistant sets the entity
/// to `unknown` and logs a warning, so a message that grew past 255 would
/// **disappear** rather than arrive shortened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupMessage {
    /// A setup has started and no shade exists yet.
    Drafting,
    /// A shade exists at an address this controller allocated, `Prog` has gone
    /// out at least once, and nobody has reported a motor moving.
    AwaitingReport,
    /// Send pairing was pressed with no name.
    NeedsName,
    /// Send pairing was pressed with a travel time missing.
    NeedsTimes,
    /// The name is longer than a shade record holds.
    NameTooLong,
    /// The registry is full.
    RegistryFull,
    /// The shade table refused the change for a reason the form cannot name.
    Refused,
}

impl SetupMessage {
    /// Every message, for the length proof and for the tests.
    pub const ALL: [SetupMessage; 7] = [
        SetupMessage::Drafting,
        SetupMessage::AwaitingReport,
        SetupMessage::NeedsName,
        SetupMessage::NeedsTimes,
        SetupMessage::NameTooLong,
        SetupMessage::RegistryFull,
        SetupMessage::Refused,
    ];

    /// The words themselves.
    ///
    /// Plain ASCII throughout, including the `-` where a dash would read
    /// better: the budget below is counted in bytes against a limit Home
    /// Assistant counts in characters, and keeping the two equal is what makes
    /// the compile-time assertion mean what it says.
    pub const fn as_str(self) -> &'static str {
        match self {
            SetupMessage::Drafting => {
                "Step 1 of 2. Name the shade, pick its kind, and set how long a full travel \
                 takes each way in milliseconds - measure them, do not guess. Then hold PROG \
                 on the shade's existing remote about 2 s until it jogs, and press Send \
                 pairing within about 2 minutes."
            }
            SetupMessage::AwaitingReport => {
                "Step 2 of 2. A pairing frame has gone out. Command the shade and watch it: \
                 if it moved, press It moved and its cover appears. If nothing happened, hold \
                 PROG on its existing remote until it jogs and press Send pairing again."
            }
            SetupMessage::NeedsName => {
                "Give the shade a name before pressing Send pairing. Nothing is created until \
                 you do, and the name is how you will find it afterwards."
            }
            SetupMessage::NeedsTimes => {
                "Set both travel times before pressing Send pairing. Measure them with a \
                 stopwatch: a wrong travel time is why a shade asked for 25 percent moves \
                 about 1 percent, and it is the reason this form exists."
            }
            SetupMessage::NameTooLong => {
                "That name is longer than the 32 bytes a shade record holds, and accented \
                 letters cost more than one byte each. Shorten it and press Send pairing \
                 again."
            }
            SetupMessage::RegistryFull => {
                "This controller already holds as many shades as it can. Remove one before \
                 adding another. Nothing was created, and Discard clears this form."
            }
            SetupMessage::Refused => {
                "The shade table refused that. The serial console and this device's own web \
                 page both say why; this form has no room for the detail. Discard clears it."
            }
        }
    }
}

const fn longest_message() -> usize {
    let mut i = 0;
    let mut max = 0;
    while i < SetupMessage::ALL.len() {
        let len = SetupMessage::ALL[i].as_str().len();
        if len > max {
            max = len;
        }
        i += 1;
    }
    max
}

/// Bytes the longest message occupies.
pub const MAX_MESSAGE_LEN: usize = longest_message();

// **The budget Home Assistant enforces, enforced here first.** Over 255 the
// message does not arrive shortened: the entity goes to `unknown` and the whole
// sentence is lost, with a warning in a log nobody is reading at the moment they
// need the instruction. Counted in bytes against a character limit, which is the
// safe direction — these are ASCII, so the two are equal, and any future
// non-ASCII would make this stricter rather than looser.
const _: () = assert!(
    MAX_MESSAGE_LEN <= MAX_STATE_LEN,
    "a setup message outgrew Home Assistant's 255-character state limit; it would be \
     dropped entirely rather than truncated",
);

/// A discovery config for one entity of the add-a-shade form, as data.
///
/// One type for all five components rather than five types, because what
/// differs between them is four keys and what is the same is everything else —
/// and because a single [`SetupDiscovery::render`] is a single worst case to
/// prove, which is what keeps [`PAYLOAD_CAPACITY`] checkable.
#[derive(Debug, Clone)]
pub struct SetupDiscovery<'a> {
    /// The payload's `~`: `{state_root}/setup`, absolute and with no leading
    /// slash.
    pub base: Topic,
    /// Absolute, and under the state root. See
    /// [`CoverDiscovery::availability`](crate::CoverDiscovery::availability)
    /// for why it can never be under the discovery prefix.
    pub availability: Topic,
    /// The discovery topic's last segment before `config`.
    pub object_id: ObjectId,
    /// The identity Home Assistant remembers this entity by.
    pub unique_id: UniqueId,
    /// The stable device identifier, for the payload's `device` block.
    pub device_id: &'a str,
    /// Where a person goes to configure this controller, for the same block.
    pub configuration_url: Option<&'a str>,
    /// Which part of the form this is.
    pub entity: SetupEntity,
}

impl SetupDiscovery<'_> {
    /// Render the JSON Home Assistant reads.
    ///
    /// On failure `out` is left **empty**, for the same reason
    /// [`CoverDiscovery::render`](crate::CoverDiscovery::render) leaves it
    /// empty: a half-written config is truncated JSON, and a payload Home
    /// Assistant cannot parse produces no entity.
    ///
    /// Every key written here appears in the schema table in
    /// `docs/provenance.md`, with the file and line that validates it. That is
    /// not thoroughness for its own sake: each discovery schema is built with
    /// `extra=vol.REMOVE_EXTRA`, so a key Home Assistant does not recognise is
    /// dropped **without a word**, and the entity appears missing whatever it
    /// carried.
    pub fn render(&self, out: &mut String<PAYLOAD_CAPACITY>) -> Result<(), PayloadError> {
        out.clear();
        match self.write_into(out) {
            Ok(()) => Ok(()),
            Err(e) => {
                out.clear();
                Err(e)
            }
        }
    }

    fn write_into(&self, out: &mut String<PAYLOAD_CAPACITY>) -> Result<(), PayloadError> {
        write(out, "{")?;

        write(out, "\"~\":")?;
        write_json_string(out, self.base.as_str())?;

        write(out, ",\"availability_topic\":")?;
        write_json_string(out, self.availability.as_str())?;

        write_object_id(out, self.device_id, &self.object_id)?;

        write(out, ",\"unique_id\":")?;
        write_json_string(out, self.unique_id.as_str())?;

        write(out, ",\"name\":")?;
        write_json_string(out, self.entity.label())?;

        // See `SETUP_CATEGORY`: one category for all nine, so the form is one
        // card rather than two.
        write(out, ",\"entity_category\":")?;
        write_json_string(out, SETUP_CATEGORY)?;

        write_device_block(out, self.device_id, self.configuration_url)?;

        if self.entity.accepts_command() {
            // Required by `MQTT_RW_SCHEMA` for all four components that reach
            // here (`components/mqtt/config.py:39-46`); a payload without it is
            // rejected outright and the entity never appears.
            write(out, ",\"command_topic\":\"~/")?;
            write(out, self.entity.leaf())?;
            write(out, "/set\"")?;
        }
        if self.entity.has_state() {
            write(out, ",\"state_topic\":\"~/")?;
            write(out, self.entity.leaf())?;
            write(out, "\"")?;
        }

        self.write_component_keys(out)?;
        write(out, "}")
    }

    /// The keys that belong to one component and no other.
    ///
    /// Deliberately absent: `optimistic`, `retain` and `qos`. Home Assistant's
    /// own defaults are right for every one of them, and this project's
    /// standing rule is that a literal restated on both sides with nothing
    /// comparing them is a mismatch waiting to happen. `retain` matters most:
    /// it defaults to **false** (`components/mqtt/config.py:43`), which is R6 —
    /// Home Assistant must not retain a command it publishes here, or a press
    /// would replay on every reconnect and add a shade each time.
    fn write_component_keys(&self, out: &mut String<PAYLOAD_CAPACITY>) -> Result<(), PayloadError> {
        match self.entity.component() {
            // Nothing. `payload_press` defaults to `PRESS`
            // (`components/mqtt/const.py:307`), which the firmware matches
            // exactly rather than declaring — the same trade `ButtonDiscovery`
            // makes, and it fails in the safe direction: a mismatch produces a
            // button that presses and transmits nothing.
            Component::Button => Ok(()),
            // `min` 0 rather than 1, deliberately. Home Assistant's `text`
            // raises on a *state* shorter than `min`, and the retirement clears
            // this entity's state topic with a zero-length payload; ordering the
            // config tombstone first makes that unreachable, but a rule that
            // depends on ordering is a rule with a way to be wrong. An empty
            // name is refused here instead, by the flow, with a sentence saying
            // so — which is this crate's habit anyway.
            Component::Text => {
                write(out, ",\"min\":0,\"max\":")?;
                write_u32(out, MAX_DRAFT_NAME_LEN as u32)?;
                write(out, ",\"mode\":\"text\"")
            }
            // **The defaults are 0-100** (`components/mqtt/number.py:91-92`),
            // so stating the range is not decoration: without it every
            // millisecond value is out of range, logged at error and dropped.
            // `box` rather than the default `auto`, because a slider across
            // 180,000 milliseconds cannot be aimed.
            Component::Number => {
                write(out, ",\"min\":")?;
                write_u32(out, MIN_TRAVEL_MS)?;
                write(out, ",\"max\":")?;
                write_u32(out, MAX_TRAVEL_MS)?;
                write(out, ",\"step\":")?;
                write_u32(out, TRAVEL_STEP_MS)?;
                // No `device_class`. `duration` would earn an icon and would
                // cost the whole entity if Home Assistant ever disagreed about
                // which units it admits — and a rejected payload is an entity
                // that never appears. The unit alone cannot be rejected.
                write(out, ",\"mode\":\"box\",\"unit_of_measurement\":\"ms\"")
            }
            // The one required key beyond `command_topic`
            // (`components/mqtt/select.py:57`).
            Component::Select => {
                write(out, ",\"options\":[")?;
                for (index, kind) in KIND_OPTIONS.into_iter().enumerate() {
                    if index > 0 {
                        write(out, ",")?;
                    }
                    write_json_string(out, kind_label(kind))?;
                }
                write(out, "]")
            }
            // The instructions. No `device_class` and no `state_class`: it is
            // prose, and either would make Home Assistant try to parse it as a
            // number or a timestamp. Being classless is also what puts it
            // through `check_state_too_long` rather than a stricter parser, and
            // `MAX_MESSAGE_LEN` is asserted against that limit above.
            Component::Sensor => Ok(()),
            // Unreachable: `SetupEntity::component` returns one of the four
            // above. Written as a value rather than a panic because nothing in
            // this crate panics over a payload.
            _ => Ok(()),
        }
    }
}

/// Write a `u32` as decimal.
///
/// The digits go straight in rather than through the JSON escaper: they are a
/// bare JSON number, not a string, and no ASCII digit has an escape in any
/// case.
fn write_u32(out: &mut String<PAYLOAD_CAPACITY>, value: u32) -> Result<(), PayloadError> {
    // `Display for u32` writes its digits in one `write_str`, so this is
    // all-or-nothing like every other push in this crate — never a half-written
    // number, which in a JSON payload would be a different number.
    core::write!(out, "{value}").map_err(|_| PayloadError::TooLong)
}

/// Digits in the widest number this payload writes.
const MAX_NUMBER_DIGITS: usize = 10;

/// The `select` payload's option list at its widest.
const WORST_OPTIONS_LEN: usize =
    // ,"options":[
    12
    // "<label>", per option, each counted at the longest
    + KIND_OPTIONS.len() * (MAX_KIND_LABEL_LEN + 3)
    // ]
    + 1;

/// The widest component-specific block, whichever component reaches it.
///
/// The **maximum** rather than the sum: one entity is one component, so only one
/// of these blocks is ever written.
const WORST_COMPONENT_KEYS_LEN: usize = {
    // ,"min":0,"max":NN,"mode":"text"
    let text = 16 + MAX_NUMBER_DIGITS + 15;
    // ,"min":N,"max":N,"step":N,"mode":"box","unit_of_measurement":"ms"
    let number = 7 + MAX_NUMBER_DIGITS + 7 + MAX_NUMBER_DIGITS + 8 + MAX_NUMBER_DIGITS + 45;
    let widest = if text > number { text } else { number };
    if WORST_OPTIONS_LEN > widest {
        WORST_OPTIONS_LEN
    } else {
        widest
    }
};

/// The setup payload budget, proven the way the other three are.
///
/// Deliberately loose: it costs nothing to over-reserve, and the point is that
/// the arithmetic is checkable rather than tight. Both the command topic and
/// the state topic are counted as present even though no entity carries both at
/// the longest leaf.
const WORST_SETUP_PAYLOAD_LEN: usize = WORST_COMMON_LEN
    // "name":"<label>", — a firmware literal, so escaping cannot expand it.
    + 8 + SetupEntity::MAX_LABEL_LEN + 2
    // "entity_category":"config",
    + 28
    // ,"command_topic":"~/<leaf>/set"
    + 18 + 2 + SetupEntity::MAX_LEAF_LEN + 5
    // ,"state_topic":"~/<leaf>"
    + 16 + 2 + SetupEntity::MAX_LEAF_LEN + 1
    + WORST_COMPONENT_KEYS_LEN;

const _: () = assert!(
    PAYLOAD_CAPACITY >= WORST_SETUP_PAYLOAD_LEN,
    "PAYLOAD_CAPACITY is too small for the longest setup payload this crate can build",
);

/// `{state_root}/setup/{leaf}/set` at its widest.
pub(crate) const WORST_SETUP_TOPIC_LEN: usize = config::WORST_SETUP_BASE_LEN
    + 1
    + SetupEntity::MAX_LEAF_LEN
    // /set
    + 4;
