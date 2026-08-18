//! The add-a-shade flow: three phases, eleven inputs, and no behaviour of its
//! own.
//!
//! # What this is *not*
//!
//! It is not a second implementation of "what adding a shade does". Every
//! effect it can ask for is one of the edits the web UI already asks for, and
//! they are applied by the same function on the far side of the same seam —
//! `firmware::tasks::apply_edit`, reached through `firmware::rpc`. Adding a
//! shade allocates an address once, validates through `CreateShadeDto`, seeds a
//! rolling code and refuses to announce anything until an operator reports the
//! motor moved, and **none of that is here**. What is here is which control an
//! operator may press when, and what the form should say next.
//!
//! That is the honest division. The behaviour is shared because both surfaces
//! call the same code; the *form* is per-transport because a form is, and the
//! web UI's equivalent state lives in a browser rather than on the device.
//!
//! # The three phases
//!
//! | phase | what exists | what the form shows |
//! |---|---|---|
//! | [`SetupPhase::Idle`] | nothing | just `Add shade` |
//! | [`SetupPhase::Drafting`] | a draft, in RAM | the whole form; `Send pairing` creates the shade |
//! | [`SetupPhase::AwaitingReport`] | a shade at an allocated address, unconfirmed | the whole form; `It moved` finishes it |
//!
//! **Nothing between `Drafting` and a confirmed shade claims the motor obeys.**
//! A shade created in `AwaitingReport` has an address this controller invented
//! moments earlier, so it has no cover and no pairing button in Home Assistant
//! — `firmware::tasks::announce_shade` is the one gate and it is unchanged.
//! What it does have is a row in `Shades awaiting setup`, which is a count
//! about the controller and a claim about no motor at all.
//!
//! # Why the fields stay live after the shade exists
//!
//! Because the sequence that actually happens is: create, pair, *command the
//! shade and time it*, correct the travel times, confirm. If the numbers froze
//! at `Send pairing` the form would be a wizard with dead controls, and the one
//! chance the MQTT surface gets to set a measured travel time would be the
//! chance taken before anything had been measured. So an edit in
//! `AwaitingReport` asks for [`Ask::Amend`], which is the same `PatchShadeDto`
//! the web UI's edit screen sends.
//!
//! # Nothing here is persisted
//!
//! A draft is a form in progress, and a reboot loses it exactly as closing a
//! browser tab does. What survives is anything the shade table was actually
//! told: a shade created and not confirmed is still in the registry, still
//! counted by `Shades awaiting setup`, and still finishable from the web UI
//! that `configuration_url` links to. Resuming the *form* after a reboot was
//! considered and declined — the draft is gone, so the fields would have to be
//! rebuilt from the shade, and with two unconfirmed shades there is no
//! answering which one the form is about.

use crate::config::MqttConfig;
use crate::setup::{
    kind_from_label, SetupEntity, SetupMessage, MAX_DRAFT_NAME_LEN, MAX_TRAVEL_MS, MIN_TRAVEL_MS,
};
use heapless::String;
use somfy_domain::{ShadeId, ShadeKind};

/// Where a setup has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupPhase {
    /// No setup is running. Only `Add shade` exists.
    Idle,
    /// The form is up and **nothing has been created**. A discard here costs
    /// nothing and leaves no trace beyond the tombstones.
    Drafting,
    /// A shade exists at an address this controller allocated, and nobody has
    /// reported a motor obeying it.
    AwaitingReport {
        /// The shade the rest of the form now acts on.
        shade: ShadeId,
    },
}

impl SetupPhase {
    /// Whether the form's eight entities are on the broker.
    pub fn is_open(self) -> bool {
        !matches!(self, SetupPhase::Idle)
    }

    /// The shade this phase is about, if there is one.
    pub fn shade(self) -> Option<ShadeId> {
        match self {
            SetupPhase::AwaitingReport { shade } => Some(shade),
            _ => None,
        }
    }
}

/// A shade **this flow created**, and the only thing a removal can be addressed
/// at.
///
/// # Why a newtype and not a `ShadeId`
///
/// Because a confirmed shade was deleted from a real estate, and the form was
/// the only new thing that could issue a `Remove`. Whatever produced it, the
/// defect class is that `Ask::Abandon(ShadeId)` let a removal name *any* id —
/// so the invariant "only a shade the form created" lived in the reasoning of
/// one match arm rather than in anything a compiler or a reviewer checks.
///
/// The field is private and [`OwnShade::new`] is private to this module, and
/// the **single** call to it is in [`Setup::apply_drafting`]'s
/// [`SetupInput::Created`] arm — the point at which the shade table has just
/// told this flow the id it allocated *for this flow*. Nothing outside
/// `somfy-mqtt` can construct one, and nothing inside it does anywhere else, so
/// "the form can only delete what it made" is now a property of the type rather
/// than a claim about control flow.
///
/// It deliberately exposes [`OwnShade::id`] and no `From<ShadeId>`: reading one
/// out is what the firmware needs, and writing one in is the thing that must
/// stay impossible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnShade(ShadeId);

impl OwnShade {
    /// Private, and called from exactly one place. See the type's docs.
    fn new(id: ShadeId) -> OwnShade {
        OwnShade(id)
    }

    /// The shade, for a caller that has to address it.
    pub fn id(self) -> ShadeId {
        self.0
    }
}

/// What the operator has filled in.
///
/// Travel times are [`Option`] and start `None` — **that is the point of the
/// whole form.** A draft that started at the factory 10000/10000 would let an
/// operator press `Send pairing` immediately and get exactly the defaults that
/// made a shade move about 1% when it was asked for 25%. An empty box asks a
/// question; a pre-filled one answers it for you.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    name: String<MAX_DRAFT_NAME_LEN>,
    kind: ShadeKind,
    up_ms: Option<u32>,
    down_ms: Option<u32>,
}

impl Default for Draft {
    fn default() -> Draft {
        Draft {
            name: String::new(),
            // A shade is always *some* kind, so there is no honest "unset" here
            // the way there is for a travel time — and `Roller` is both the
            // domain's own discriminant zero and the commonest thing on a real
            // estate. It is published, so it is visibly chosen rather than
            // silently defaulted.
            kind: ShadeKind::Roller,
            up_ms: None,
            down_ms: None,
        }
    }
}

impl Draft {
    /// The name, which is empty until somebody types one.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The kind.
    pub fn kind(&self) -> ShadeKind {
        self.kind
    }

    /// The upward travel time, if it has been set.
    pub fn up_ms(&self) -> Option<u32> {
        self.up_ms
    }

    /// The downward travel time, if it has been set.
    pub fn down_ms(&self) -> Option<u32> {
        self.down_ms
    }

    /// Why this draft could not become a shade, or `None` if it can.
    ///
    /// The same three rules `somfy_api::CreateShadeDto::to_config` applies,
    /// checked here so the refusal arrives **in the form** rather than as a
    /// discarded error on a fire-and-forget channel. It is a restatement and
    /// not a second authority: the shade table checks them again, and a draft
    /// this passes can still be refused for a reason only the table knows.
    pub fn blocker(&self) -> Option<SetupMessage> {
        if self.name.is_empty() {
            return Some(SetupMessage::NeedsName);
        }
        if self.up_ms.is_none() || self.down_ms.is_none() {
            return Some(SetupMessage::NeedsTimes);
        }
        None
    }
}

/// One thing that reaches the flow.
///
/// The first eight arrive from the broker and are produced by
/// [`Setup::decode`]. The last three are the shade table answering, and are fed
/// in by the firmware — so a refusal and a created id travel the same path as a
/// button press and are handled by the same `match`, which is what stops the
/// two halves of the flow drifting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupInput<'a> {
    /// `Add shade` was pressed.
    Begin,
    /// The name field was set.
    Name(&'a str),
    /// The kind field was set, as one of the option strings.
    Kind(&'a str),
    /// The upward travel time was set, as Home Assistant renders a number.
    TravelUp(&'a str),
    /// The downward travel time was set.
    TravelDown(&'a str),
    /// `Send pairing` was pressed.
    Send,
    /// `It moved` was pressed. **The operator's report**, not an observation.
    Confirm,
    /// `Discard setup` was pressed.
    Discard,
    /// The shade table created a shade and gave it this id.
    Created(ShadeId),
    /// The shade table refused what it was asked, with the nearest message the
    /// form has.
    Refused(SetupMessage),
    /// The shade table did what it was asked, and the setup is over.
    Done,
}

/// Whether the form's entities must appear, go, or stay as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormChange {
    /// Neither announce nor retire.
    Unchanged,
    /// Publish [`MqttConfig::open_form`], then the values.
    Open,
    /// Publish [`MqttConfig::close_form`]. **No values follow**: the entities
    /// are gone and their topics have just been cleared.
    Close,
}

/// What the flow needs the shade table to do.
///
/// Every variant is one of the edits the web UI already makes, named from this
/// side. None of them is applied here — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// Create a shade from the draft. The answer comes back as
    /// [`SetupInput::Created`] or [`SetupInput::Refused`].
    Create,
    /// Transmit a pairing burst at this shade.
    Pair(ShadeId),
    /// Record the operator's report. The answer comes back as
    /// [`SetupInput::Done`] or [`SetupInput::Refused`].
    Confirm(ShadeId),
    /// Remove **a shade this flow created in this session**, and nothing else.
    ///
    /// Carries [`OwnShade`] rather than a bare [`ShadeId`], so a `Remove` is
    /// not a thing a caller can address at a shade of its choosing — see that
    /// type. The form closes either way.
    Abandon(OwnShade),
    /// Store the draft's current values on the shade that already exists.
    Amend(ShadeId),
}

/// What one input costs.
///
/// # The republish rule, stated once
///
/// **After any input that leaves the form open, the caller republishes every
/// form value.** That is five small retained publishes for an action a human
/// took, and it is deliberately not optimised into "republish only what
/// changed": the alternative is a per-input list of which entities moved, which
/// is a second place for the truth to live and the exact shape of the
/// payload/publisher drift `ShadeTopic` exists to prevent. `Next step` changes
/// on almost every input anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    /// Whether the form's entities appear or go.
    pub form: FormChange,
    /// What to ask the shade table for, if anything.
    pub ask: Option<Ask>,
}

impl Effect {
    const NOTHING: Effect = Effect {
        form: FormChange::Unchanged,
        ask: None,
    };
}

/// One form entity's current value, as the caller must publish it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupValue<'a> {
    /// Publish these bytes.
    Text(&'a str),
    /// Publish this number in decimal.
    Number(u32),
    /// **Publish nothing.** Home Assistant shows the entity as unknown, which
    /// is what it is — and an empty retained payload would be a *tombstone*,
    /// removing the value rather than leaving it blank.
    Unset,
}

/// The add-a-shade flow.
///
/// Pure: no clock, no socket, no registry. Everything it can ask for goes back
/// to the caller as an [`Effect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    phase: SetupPhase,
    draft: Draft,
    message: SetupMessage,
    /// The shade this flow created in this session, if it created one.
    ///
    /// **The only thing a removal may be addressed at**, and the reason
    /// [`OwnShade`] exists. Set in exactly one place — the [`SetupInput::Created`]
    /// arm — and cleared by [`Setup::close`], so it cannot outlive the setup
    /// that produced it. It duplicates the id in
    /// [`SetupPhase::AwaitingReport`] on purpose: the phase is *where the form
    /// is*, which several inputs move, and this is *what the form made*, which
    /// only one thing sets. Keeping them separate is what makes
    /// [`Setup::abandon`] able to check that they agree.
    created: Option<OwnShade>,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            phase: SetupPhase::Idle,
            draft: Draft::default(),
            message: SetupMessage::Drafting,
            created: None,
        }
    }
}

impl Setup {
    /// A flow with nothing running.
    pub fn new() -> Setup {
        Setup::default()
    }

    /// Where the setup has got to.
    pub fn phase(&self) -> SetupPhase {
        self.phase
    }

    /// What the operator has filled in.
    pub fn draft(&self) -> &Draft {
        &self.draft
    }

    /// What `Next step` currently says.
    pub fn message(&self) -> SetupMessage {
        self.message
    }

    /// One form entity's value, for the caller to publish.
    ///
    /// Returns [`SetupValue::Unset`] for the four buttons as well as for a
    /// travel time nobody has set, and the caller publishes nothing in either
    /// case — a button has no state topic to publish to, and an unset number
    /// has no honest value.
    pub fn value(&self, entity: SetupEntity) -> SetupValue<'_> {
        match entity {
            SetupEntity::Name if self.draft.name.is_empty() => SetupValue::Unset,
            SetupEntity::Name => SetupValue::Text(&self.draft.name),
            SetupEntity::Kind => SetupValue::Text(crate::setup::kind_label(self.draft.kind)),
            SetupEntity::TravelUp => self
                .draft
                .up_ms
                .map_or(SetupValue::Unset, SetupValue::Number),
            SetupEntity::TravelDown => self
                .draft
                .down_ms
                .map_or(SetupValue::Unset, SetupValue::Number),
            SetupEntity::NextStep => SetupValue::Text(self.message.as_str()),
            SetupEntity::Begin
            | SetupEntity::Send
            | SetupEntity::Confirm
            | SetupEntity::Discard => SetupValue::Unset,
        }
    }

    /// Turn one inbound message into an input, or `None` if it is not one.
    ///
    /// The topic is matched against what the device actually subscribed to
    /// rather than parsed, so a message on an unexpected topic is ignored
    /// rather than guessed at — the same discipline
    /// `firmware::mqtt::decode_command` uses for a shade.
    ///
    /// **The press payload is matched exactly.** Home Assistant's default
    /// `payload_press` is the literal `PRESS`
    /// (`components/mqtt/const.py:307`), and a lenient parse here would let a
    /// stray retained message or a mistyped `mosquitto_pub` create a shade or
    /// put `Prog` on the air.
    pub fn decode<'a>(
        config: &MqttConfig,
        topic: &str,
        payload: &'a [u8],
    ) -> Option<SetupInput<'a>> {
        let text = core::str::from_utf8(payload).ok()?;
        for entity in SetupEntity::ALL {
            if !entity.accepts_command() {
                continue;
            }
            if topic != config.setup_command_topic(entity).as_str() {
                continue;
            }
            return match entity {
                SetupEntity::Begin => press(text, SetupInput::Begin),
                SetupEntity::Send => press(text, SetupInput::Send),
                SetupEntity::Confirm => press(text, SetupInput::Confirm),
                SetupEntity::Discard => press(text, SetupInput::Discard),
                SetupEntity::Name => Some(SetupInput::Name(text)),
                SetupEntity::Kind => Some(SetupInput::Kind(text)),
                SetupEntity::TravelUp => Some(SetupInput::TravelUp(text)),
                SetupEntity::TravelDown => Some(SetupInput::TravelDown(text)),
                // Filtered out above; a sensor has no command topic.
                SetupEntity::NextStep => None,
            };
        }
        None
    }

    /// Hand the flow one input and learn what it costs.
    ///
    /// Total: every input is legal in every phase, and one that means nothing
    /// where it arrived changes nothing and — where the form is open — says why
    /// in `Next step`. There is no error return, because there is no caller who
    /// could do anything with one that the message does not already do better.
    pub fn apply(&mut self, input: SetupInput<'_>) -> Effect {
        match self.phase {
            SetupPhase::Idle => self.apply_idle(input),
            SetupPhase::Drafting => self.apply_drafting(input),
            SetupPhase::AwaitingReport { shade } => self.apply_awaiting(input, shade),
        }
    }

    /// Nothing is running: only `Add shade` can do anything.
    ///
    /// Everything else is silently ignored rather than answered, and the
    /// silence is the right behaviour: the entity that would carry an answer
    /// does not exist, so a message published here would be a retained value
    /// under a config nothing has announced — an orphan of exactly the kind R5
    /// is about.
    fn apply_idle(&mut self, input: SetupInput<'_>) -> Effect {
        match input {
            SetupInput::Begin => {
                self.phase = SetupPhase::Drafting;
                self.draft = Draft::default();
                self.message = SetupMessage::Drafting;
                Effect {
                    form: FormChange::Open,
                    ask: None,
                }
            }
            _ => Effect::NOTHING,
        }
    }

    /// A draft exists and no shade does.
    fn apply_drafting(&mut self, input: SetupInput<'_>) -> Effect {
        match input {
            // Idempotent, and useful: it republishes the configs, which is how
            // a form half-lost to a broker restart comes back without the
            // operator discarding a draft they have already typed.
            SetupInput::Begin => Effect {
                form: FormChange::Open,
                ask: None,
            },
            SetupInput::Name(_)
            | SetupInput::Kind(_)
            | SetupInput::TravelUp(_)
            | SetupInput::TravelDown(_) => {
                self.edit(input);
                Effect::NOTHING
            }
            SetupInput::Send => match self.draft.blocker() {
                Some(message) => {
                    self.message = message;
                    Effect::NOTHING
                }
                None => Effect {
                    form: FormChange::Unchanged,
                    ask: Some(Ask::Create),
                },
            },
            // Nothing to confirm. The message restates the step rather than
            // scolding: what to do is press Send pairing, and that is what
            // `Drafting` says.
            SetupInput::Confirm => {
                self.message = SetupMessage::Drafting;
                Effect::NOTHING
            }
            SetupInput::Discard => {
                self.close();
                Effect {
                    form: FormChange::Close,
                    ask: None,
                }
            }
            // The shade exists. Pair it at once — that is what makes `Send
            // pairing` one press rather than two, and the operator is holding
            // PROG on the remote right now.
            SetupInput::Created(shade) => {
                self.phase = SetupPhase::AwaitingReport { shade };
                // **The one call to `OwnShade::new` in the crate.** The shade
                // table has just said it allocated this id for this flow, which
                // is the only moment that claim is true.
                self.created = Some(OwnShade::new(shade));
                self.message = SetupMessage::AwaitingReport;
                Effect {
                    form: FormChange::Unchanged,
                    ask: Some(Ask::Pair(shade)),
                }
            }
            SetupInput::Refused(message) => {
                self.message = message;
                Effect::NOTHING
            }
            // Nothing was asked that could answer this.
            SetupInput::Done => Effect::NOTHING,
        }
    }

    /// A shade exists, unconfirmed, at an address no motor is known to obey.
    fn apply_awaiting(&mut self, input: SetupInput<'_>, shade: ShadeId) -> Effect {
        match input {
            SetupInput::Begin => Effect {
                form: FormChange::Open,
                ask: None,
            },
            SetupInput::Name(_)
            | SetupInput::Kind(_)
            | SetupInput::TravelUp(_)
            | SetupInput::TravelDown(_) => {
                let moved = self.edit(input);
                Effect {
                    form: FormChange::Unchanged,
                    // Only when the value actually changed, so a rejected edit
                    // — an unknown kind, an unparseable number — does not send
                    // the shade table a patch that would restate what it
                    // already holds.
                    ask: moved.then_some(Ask::Amend(shade)),
                }
            }
            // A second, third or tenth `Prog`. Pairing is a burst at a motor
            // somebody has just put into programming mode, and getting the
            // two-minute window right on the first try is not the common case.
            SetupInput::Send => {
                self.message = SetupMessage::AwaitingReport;
                Effect {
                    form: FormChange::Unchanged,
                    ask: Some(Ask::Pair(shade)),
                }
            }
            SetupInput::Confirm => Effect {
                form: FormChange::Unchanged,
                ask: Some(Ask::Confirm(shade)),
            },
            // **The shade goes with the form.** It was created by this flow, it
            // has no entities, and leaving it behind would leave a row in
            // `Shades awaiting setup` that nothing on this surface can reach
            // again.
            SetupInput::Discard => {
                let ask = self.abandon(shade);
                self.close();
                Effect {
                    form: FormChange::Close,
                    ask,
                }
            }
            // The report landed and the shade is announced by the ordinary
            // path. The form's work is over.
            SetupInput::Done => {
                self.close();
                Effect {
                    form: FormChange::Close,
                    ask: None,
                }
            }
            SetupInput::Refused(message) => {
                self.message = message;
                Effect::NOTHING
            }
            // Unreachable: nothing asks to create while a shade exists.
            SetupInput::Created(_) => Effect::NOTHING,
        }
    }

    /// Apply one field edit. Returns whether it changed anything.
    ///
    /// A value the form cannot use — an unknown kind, a number that will not
    /// parse, a name too long — leaves the draft alone and says so in `Next
    /// step`. It is never silently coerced: a truncated name is a different
    /// shade from the one that was typed.
    fn edit(&mut self, input: SetupInput<'_>) -> bool {
        match input {
            SetupInput::Name(name) => {
                if name.len() > MAX_DRAFT_NAME_LEN {
                    self.message = SetupMessage::NameTooLong;
                    return false;
                }
                let mut held: String<MAX_DRAFT_NAME_LEN> = String::new();
                // Cannot fail — the length was just checked — and `push_str` is
                // all-or-nothing in `heapless`, so there is no half-written
                // name either way.
                if held.push_str(name).is_err() {
                    self.message = SetupMessage::NameTooLong;
                    return false;
                }
                let moved = held != self.draft.name;
                self.draft.name = held;
                self.step_message();
                moved
            }
            SetupInput::Kind(label) => match kind_from_label(label) {
                Some(kind) => {
                    let moved = kind != self.draft.kind;
                    self.draft.kind = kind;
                    self.step_message();
                    moved
                }
                None => false,
            },
            SetupInput::TravelUp(text) => match parse_travel_ms(text) {
                Some(ms) => {
                    let moved = Some(ms) != self.draft.up_ms;
                    self.draft.up_ms = Some(ms);
                    self.step_message();
                    moved
                }
                None => false,
            },
            SetupInput::TravelDown(text) => match parse_travel_ms(text) {
                Some(ms) => {
                    let moved = Some(ms) != self.draft.down_ms;
                    self.draft.down_ms = Some(ms);
                    self.step_message();
                    moved
                }
                None => false,
            },
            _ => false,
        }
    }

    /// Put `Next step` back to whatever the phase says.
    ///
    /// Called after any successful edit, so a refusal an operator has just
    /// acted on stops being shown. Without it, "give the shade a name" would
    /// stay on screen after the name was typed.
    fn step_message(&mut self) {
        self.message = match self.phase {
            SetupPhase::AwaitingReport { .. } => SetupMessage::AwaitingReport,
            _ => SetupMessage::Drafting,
        };
    }

    /// End the setup, whatever it was.
    ///
    /// The draft goes too. A discarded name reappearing in the next setup would
    /// be a half-finished thing somebody has to notice, which is the whole
    /// class of fault this flow was shaped to make unreachable.
    fn close(&mut self) {
        self.phase = SetupPhase::Idle;
        self.draft = Draft::default();
        self.message = SetupMessage::Drafting;
        // **Before anything else can run.** A shade this flow made stops being
        // this flow's the moment the setup ends, whether it ended by being
        // confirmed, discarded or abandoned — so a later `Discard`, a queued
        // effect carried out after the fact, or any future arm cannot reach a
        // removal through a stale claim.
        self.created = None;
    }

    /// The removal for a shade **this flow created**, or `None`.
    ///
    /// The guard the deleted shade earned. `shade` is where the phase says the
    /// form is; `self.created` is what the form actually made. They are set
    /// together and cleared together, so they agree — and if they ever do not,
    /// this returns `None` and the setup closes without touching the table,
    /// which is the direction that costs an abandoned record rather than
    /// somebody's shade.
    fn abandon(&self, shade: ShadeId) -> Option<Ask> {
        match self.created {
            Some(own) if own.id() == shade => Some(Ask::Abandon(own)),
            _ => None,
        }
    }
}

/// `PRESS`, exactly, or nothing.
fn press(text: &str, input: SetupInput<'static>) -> Option<SetupInput<'static>> {
    (text == PAYLOAD_PRESS).then_some(input)
}

/// What Home Assistant publishes when a `button` entity is pressed.
///
/// `DEFAULT_PAYLOAD_PRESS` in `components/mqtt/const.py:307`. Matched rather
/// than declared in the payload, for the reason
/// [`ButtonDiscovery`](crate::ButtonDiscovery) already gives: a literal stated
/// on both sides with nothing comparing them is a mismatch waiting to happen,
/// and this direction fails safe — a button that presses and does nothing.
pub const PAYLOAD_PRESS: &str = "PRESS";

/// Read a travel time out of what Home Assistant publishes for a `number`.
///
/// Home Assistant sends `int(value)` when the float is integral and the float
/// otherwise (`components/mqtt/number.py:225-236`), so with a `step` of 100 and
/// integral bounds `"10000"` is what arrives. `"10000.5"` is still reachable —
/// nothing in the number component enforces `step` on a service call — so a
/// fractional part is **truncated** rather than refused: a shade has no
/// fractional millisecond, and refusing would turn a legal action into silence.
///
/// Everything else is refused rather than guessed at: a sign, a second point,
/// anything non-numeric, and anything outside
/// [`MIN_TRAVEL_MS`]`..=`[`MAX_TRAVEL_MS`]. Home Assistant already drops
/// out-of-range values with an error (`:189-199`), so a value that reaches here
/// out of range came from somebody with an MQTT client.
fn parse_travel_ms(text: &str) -> Option<u32> {
    let whole = match text.split_once('.') {
        Some((whole, fraction)) => {
            if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            whole
        }
        None => text,
    };
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ms: u32 = whole.parse().ok()?;
    (MIN_TRAVEL_MS..=MAX_TRAVEL_MS).contains(&ms).then_some(ms)
}
