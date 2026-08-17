//! What the firmware says to a broker, and when — as data, so that the rules
//! can be checked without a broker.
//!
//! # Why the lifecycle is a plan rather than a sequence of calls
//!
//! Every rule in R5 is a statement about which message goes to which topic with
//! which retention, and every one of them is pure. None of them needs a socket
//! to be true or a socket to be tested. So the transport does not decide any of
//! them: it executes a [`Step`] at a time, and the decisions live here where
//! `tests/lifecycle.rs` can assert them.
//!
//! The three rules, and the failure each one is about:
//!
//! - **Retained where it must be.** A discovery config that is not retained
//!   disappears when the broker restarts, taking the entities with it. The
//!   device would have to be power-cycled to get them back, which is exactly
//!   the sort of dependency a home-automation integration must not have.
//! - **Cleared when it must be.** Removing an entity means publishing a
//!   *zero-length retained* payload to its config topic. Nothing else removes a
//!   retained message from a broker: not an empty non-retained publish, not an
//!   unsubscribe, not deleting the device. Without it an estate accumulates
//!   orphans that can only be cleared by hand — clearing up after the
//!   experiments behind the requirements spec took 49 of them.
//! - **Never retained where it must not be.** A retained command replays to
//!   every new subscriber, so a broker restart re-delivers whatever was last
//!   commanded. That is a shade that closes itself every time the broker
//!   restarts.
//!
//! # How each rule is kept
//!
//! Not by care at the call site. [`Publish`] holds its [`Retention`] privately
//! and has no constructor that takes one — the four builders below each fix it,
//! and each is named for the thing it builds. And the two directions a shade
//! topic can flow are separate types: [`PublishedTopic`] and
//! [`SubscribedTopic`] are both built by filtering on
//! [`ShadeTopic::role`](crate::ShadeTopic::role), and
//! [`MqttConfig::state`](crate::MqttConfig::state) — the only retained
//! per-shade publish — takes the first and cannot be given the second.
//!
//! # The symmetry that prevents orphans
//!
//! **Two arrays, and both are read by both halves.** [`SHADE_COMPONENTS`] is
//! the per-shade set and [`DeviceEntity::ALL`] is the per-device one; an
//! announcement publishes a discovery config for each member of each, and a
//! retirement clears one. An entity added to either therefore joins both sides
//! at once, and cannot be announced without also being removable.
//! `tests/lifecycle.rs::retirement_clears_every_topic_an_announcement_retains`
//! is that property, checked against the plans rather than against the arrays.
//!
//! The device-level half is the one that could have been a second list somebody
//! has to remember. It is not: [`MqttConfig::retire`] walks the same
//! `DeviceEntity::ALL` the announcement does, and does so *outside* the loop
//! over shades — so a controller with no shades at all still clears its own
//! diagnostics, which is the case a retirement written as "for each shade,
//! clear its topics" gets silently wrong.
//!
//! Retirement additionally does **not** ask whether a shade had tilt. Clearing
//! a topic that was never published costs one packet; getting the answer wrong
//! leaves `.../tilt` and `.../tilt/set` behind forever.

use crate::config::MqttConfig;
use crate::entity::{Component, DeviceEntity, ShadeTopic, TopicRole};
use crate::ident::ObjectId;
use crate::topic::Topic;
use somfy_domain::ShadeId;

/// The availability payload for a device that is running.
pub const ONLINE: &[u8] = b"online";

/// The availability payload for a device that is not.
///
/// Published by the broker on this device's behalf, from the will registered in
/// CONNECT, and by the device itself when it retires a configuration.
pub const OFFLINE: &[u8] = b"offline";

/// The components one shade owns an entity of.
///
/// Read by both halves of the lifecycle — see this module's docs. Only `Cover`,
/// and that is the honest answer rather than a placeholder: a shade reports a
/// position and a direction, and `somfy-domain` reports nothing else about one.
/// R7's fuller entity set is device-level, and [`DeviceEntity`] carries it.
/// Adding a component here adds it to the announcement and to the retirement
/// together.
pub const SHADE_COMPONENTS: [Component; 1] = [Component::Cover];

/// Whether the broker keeps a message after it has delivered it.
///
/// An enum rather than a `bool` because the two are not interchangeable at a
/// call site and a `true` there says nothing about which way round it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    /// The broker stores this message against its topic and replays it to every
    /// later subscriber. The last one wins; a zero-length payload deletes it.
    Retained,
    /// The broker forwards it to whoever is subscribed now, and forgets it.
    Transient,
}

/// The bytes of one message, or an instruction for producing them.
///
/// [`Payload::Discovery`] exists because rendering a discovery config needs a
/// kilobyte of buffer, and this crate holds none: the firmware owns exactly one
/// and renders into it as it walks the plan. Keeping the *decision* here and
/// the *buffer* there is what lets the plan be a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload<'a> {
    /// Bytes the executor writes as they are.
    Bytes(&'a [u8]),
    /// The discovery config for one shade's entity of this component, to be
    /// rendered by the executor.
    Discovery {
        /// The shade the config describes.
        shade: ShadeId,
        /// Which of the shade's entities it describes.
        component: Component,
    },
    /// The discovery config for one device-level entity, to be rendered by the
    /// executor.
    ///
    /// A separate variant rather than a `Discovery` with no shade: the two are
    /// rendered by different functions from different data, and an `Option`
    /// there would leave the executor with a combination — a shade and a
    /// diagnostic — that means nothing.
    DeviceDiscovery(DeviceEntity),
    /// No bytes at all.
    ///
    /// **This is the removal.** Paired with [`Retention::Retained`] — which is
    /// the only pairing this module builds — it is what deletes a retained
    /// message from a broker.
    Nothing,
}

/// One message the lifecycle requires, with its retention already decided.
///
/// There is no constructor that takes a [`Retention`]. The four builders below
/// each fix it, so "was this meant to be retained?" is not a question anyone
/// answers at a call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publish<'a> {
    topic: Topic,
    payload: Payload<'a>,
    retention: Retention,
}

impl<'a> Publish<'a> {
    /// Where it goes.
    pub fn topic(&self) -> &Topic {
        &self.topic
    }

    /// What it carries.
    pub fn payload(&self) -> Payload<'a> {
        self.payload
    }

    /// Whether the broker keeps it.
    pub fn retention(&self) -> Retention {
        self.retention
    }

    /// Convenience for a transport that wants a flag.
    pub fn is_retained(&self) -> bool {
        matches!(self.retention, Retention::Retained)
    }
}

/// A subscription, and what it must do about messages the broker already holds.
///
/// The retained-replay decision is carried rather than remembered. R6 asks for
/// commands to be *subscribed* with retention off as well as published without
/// it, and that half is the easy one to lose: a broker that already holds a
/// retained message on a command topic — left by a previous integration, or by
/// a `mosquitto_pub -r` during debugging — replays it to every new subscriber.
/// Suppressing the replay is the only defence a subscriber has, and it is a
/// per-subscription option rather than anything the publisher can fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listen {
    topic: Topic,
    retained_replay: bool,
}

impl Listen {
    /// What to subscribe to.
    pub fn topic(&self) -> &Topic {
        &self.topic
    }

    /// Whether the broker should replay a retained message on this topic when
    /// the subscription is created. Always false for a command topic; see the
    /// type's docs.
    pub fn retained_replay(&self) -> bool {
        self.retained_replay
    }
}

/// One thing the firmware does to the broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step<'a> {
    /// Send a message.
    Send(Publish<'a>),
    /// Subscribe to a topic.
    Listen(Listen),
}

/// A shade topic the firmware publishes to.
///
/// The only constructor filters on [`ShadeTopic::role`], and
/// [`MqttConfig::state`] — the one retained per-shade publish — takes nothing
/// else. So R6's publish half is not a rule to remember: a command topic has no
/// route to a retained publish, because it cannot be made into the type that
/// one accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedTopic(ShadeTopic);

impl PublishedTopic {
    /// `Some` if the firmware publishes this topic, `None` if it subscribes to
    /// it.
    pub fn of(topic: ShadeTopic) -> Option<PublishedTopic> {
        matches!(topic.role(), TopicRole::Published).then_some(PublishedTopic(topic))
    }

    /// Every topic a shade's state is published on.
    pub fn for_shade(has_tilt: bool) -> impl Iterator<Item = PublishedTopic> {
        ShadeTopic::for_shade(has_tilt).filter_map(PublishedTopic::of)
    }
}

impl From<PublishedTopic> for ShadeTopic {
    fn from(topic: PublishedTopic) -> ShadeTopic {
        topic.0
    }
}

/// A shade topic the firmware subscribes to — a command topic.
///
/// The mirror of [`PublishedTopic`], and the reason both exist: the subscribe
/// set and the publish set are derived from the same
/// [`role`](ShadeTopic::role) rather than written out twice, so a topic cannot
/// end up in both or in neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscribedTopic(ShadeTopic);

impl SubscribedTopic {
    /// `Some` if the firmware subscribes to this topic, `None` if it publishes
    /// it.
    pub fn of(topic: ShadeTopic) -> Option<SubscribedTopic> {
        matches!(topic.role(), TopicRole::Subscribed).then_some(SubscribedTopic(topic))
    }

    /// Every topic a shade takes commands on.
    pub fn for_shade(has_tilt: bool) -> impl Iterator<Item = SubscribedTopic> {
        ShadeTopic::for_shade(has_tilt).filter_map(SubscribedTopic::of)
    }
}

impl From<SubscribedTopic> for ShadeTopic {
    fn from(topic: SubscribedTopic) -> ShadeTopic {
        topic.0
    }
}

/// The zero-length retained publish that removes whatever the broker holds at
/// `topic`.
///
/// The single place the removal is spelled, so "retained *and* empty" is stated
/// once rather than at each of the several sites that need it. Either half
/// alone does nothing: an empty publish without the retain flag deletes no
/// stored message, and a retained publish with a payload replaces it with a
/// message Home Assistant will try to parse as a config.
fn tombstone(topic: Topic) -> Publish<'static> {
    Publish {
        topic,
        payload: Payload::Nothing,
        retention: Retention::Retained,
    }
}

impl MqttConfig {
    /// The last will: `offline`, retained, at the availability topic.
    ///
    /// Registered in CONNECT, because that is the only moment a client can hand
    /// the broker something to say on its behalf — and the case it exists for
    /// is precisely the one where the client is no longer able to say anything.
    /// Retained for the same reason `online` is: a subscriber that connects
    /// after the device died must still learn that it is dead, rather than find
    /// the topic empty and treat the device as merely unknown.
    pub fn will(&self) -> Publish<'static> {
        Publish {
            topic: self.availability_topic(),
            payload: Payload::Bytes(OFFLINE),
            retention: Retention::Retained,
        }
    }

    /// `online`, retained, at the availability topic. Published after CONNACK.
    pub fn online(&self) -> Publish<'static> {
        Publish {
            topic: self.availability_topic(),
            payload: Payload::Bytes(ONLINE),
            retention: Retention::Retained,
        }
    }

    /// A retained publish of one of a shade's state values.
    ///
    /// Retained so that a subscriber connecting later sees the current position
    /// instead of waiting for the next change — which, for a shade nobody
    /// touches, may be days.
    ///
    /// Takes a [`PublishedTopic`], so a command topic cannot reach it. See that
    /// type for why the distinction is carried by the type system rather than
    /// by a check.
    pub fn state<'a>(&self, shade: ShadeId, topic: PublishedTopic, value: &'a [u8]) -> Publish<'a> {
        Publish {
            topic: self.shade_topic(shade, topic.into()),
            payload: Payload::Bytes(value),
            retention: Retention::Retained,
        }
    }

    /// A retained publish of one device-level entity's current reading.
    ///
    /// Retained for the same reason a shade's state is: a subscriber connecting
    /// later must see the current figure rather than wait out a publish
    /// interval for the next one.
    ///
    /// There is no `PublishedTopic` equivalent to guard this, because there is
    /// nothing to guard against — [`DeviceEntity`] has no subscribed variant.
    /// See its docs for what adding one would cost.
    pub fn device_state<'a>(&self, entity: DeviceEntity, value: &'a [u8]) -> Publish<'a> {
        Publish {
            topic: self.device_topic(entity),
            payload: Payload::Bytes(value),
            retention: Retention::Retained,
        }
    }

    /// Everything to say on a broker session that has just been established.
    ///
    /// `online` first: the availability topic is what an operator watching the
    /// broker uses to tell "connected" from "connecting", and it should not
    /// wait behind a discovery config per shade.
    ///
    /// Then the **shades**, and the device's own diagnostics after them. An
    /// announcement can be cut short by the session ending, and the covers are
    /// what the device is for; a diagnostic that arrives on the next attempt
    /// costs nothing, a cover that does not is the integration not working.
    ///
    /// State values are **not** here, because this crate does not know them.
    /// The firmware publishes them from its registry and its own counters after
    /// walking this plan; that is the "republish retained state on reconnect"
    /// half of R9.
    pub fn announce<'a>(
        &'a self,
        shades: &'a [ShadeId],
        has_tilt: bool,
    ) -> impl Iterator<Item = Step<'static>> + 'a {
        core::iter::once(Step::Send(self.online()))
            .chain(
                shades
                    .iter()
                    .flat_map(move |shade| self.announce_shade(*shade, has_tilt)),
            )
            .chain(self.announce_device())
    }

    /// The controller's own diagnostics: one retained discovery config each.
    ///
    /// Outside the loop over shades, because these describe the controller. A
    /// device with no shades provisioned still reports its heap, its uptime and
    /// the health of its rolling-code store — which is precisely the state an
    /// operator most needs to see it in.
    pub fn announce_device(&self) -> impl Iterator<Item = Step<'static>> + '_ {
        DeviceEntity::ALL.into_iter().map(move |entity| {
            Step::Send(Publish {
                topic: self.discovery_topic(entity.component(), &ObjectId::for_device(entity)),
                payload: Payload::DeviceDiscovery(entity),
                retention: Retention::Retained,
            })
        })
    }

    /// One shade's discovery configs and command subscriptions.
    pub fn announce_shade(
        &self,
        shade: ShadeId,
        has_tilt: bool,
    ) -> impl Iterator<Item = Step<'static>> + '_ {
        let object = ObjectId::for_shade(shade);
        SHADE_COMPONENTS
            .into_iter()
            .map(move |component| {
                Step::Send(Publish {
                    topic: self.discovery_topic(component, &object),
                    payload: Payload::Discovery { shade, component },
                    retention: Retention::Retained,
                })
            })
            .chain(SubscribedTopic::for_shade(has_tilt).map(move |topic| {
                Step::Listen(Listen {
                    topic: self.shade_topic(shade, topic.into()),
                    // See [`Listen`]: a retained message on a command topic is
                    // a command that replays on every reconnect.
                    retained_replay: false,
                })
            }))
    }

    /// Everything that removes one shade — its entities and its retained state.
    ///
    /// Every step is a zero-length retained publish. Deliberately takes no
    /// `has_tilt`: see this module's docs.
    ///
    /// This is the plan to run when a shade is deleted from the registry. It is
    /// **not** run at shutdown — a device going offline still owns its entities,
    /// and the will is what says so.
    pub fn retire_shade(&self, shade: ShadeId) -> impl Iterator<Item = Step<'static>> + '_ {
        let object = ObjectId::for_shade(shade);
        SHADE_COMPONENTS
            .into_iter()
            .map(move |component| Step::Send(tombstone(self.discovery_topic(component, &object))))
            .chain(
                PublishedTopic::for_shade(true)
                    .map(move |topic| Step::Send(tombstone(self.shade_topic(shade, topic.into())))),
            )
    }

    /// Everything that removes the controller's own diagnostics — their
    /// discovery configs and their retained readings.
    ///
    /// The mirror of [`MqttConfig::announce_device`], derived from the same
    /// [`DeviceEntity::ALL`], plus the readings an announcement cannot emit
    /// because it does not know them. A diagnostic's last reading outlives its
    /// entity in exactly the way a shade's position does, and the evidence
    /// behind R5 is 49 retained topics deleted by hand, most of them state.
    pub fn retire_device(&self) -> impl Iterator<Item = Step<'static>> + '_ {
        DeviceEntity::ALL
            .into_iter()
            .map(move |entity| {
                Step::Send(tombstone(self.discovery_topic(
                    entity.component(),
                    &ObjectId::for_device(entity),
                )))
            })
            .chain(
                DeviceEntity::ALL
                    .into_iter()
                    .map(move |entity| Step::Send(tombstone(self.device_topic(entity)))),
            )
    }

    /// Everything that removes every entity and every retained topic this
    /// configuration owns, availability included.
    ///
    /// Run when the configuration itself is being abandoned — a changed
    /// `state_root` or `discovery_prefix`, or discovery being switched off.
    ///
    /// The device's own diagnostics are cleared **outside** the loop over
    /// shades, so a controller with nothing provisioned still retires them.
    /// Availability is cleared last and only here: without it the old
    /// `{state_root}/status` keeps saying `online` forever, which is a worse
    /// orphan than a stale config because it is confidently wrong.
    pub fn retire<'a>(&'a self, shades: &'a [ShadeId]) -> impl Iterator<Item = Step<'static>> + 'a {
        shades
            .iter()
            .flat_map(move |shade| self.retire_shade(*shade))
            .chain(self.retire_device())
            .chain(core::iter::once(Step::Send(tombstone(
                self.availability_topic(),
            ))))
    }
}

/// Move to `new`: clear everything every configuration in `superseded` owns,
/// then announce `new` — once.
///
/// The ordering R5 requires, expressed as a function rather than as a note.
/// There is no argument order or flag that produces the other sequence, and
/// nothing that returns the two halves separately for a caller to interleave —
/// publishing the new configs first would leave Home Assistant holding both
/// sets for as long as the retirement took.
///
/// `superseded` is a **slice** rather than one configuration for two reasons. A
/// device can have been re-provisioned more than once between boots, so there
/// may be several sets of orphans to clear; and a caller that looped over them
/// itself would announce `new` once per old configuration, which is a broker's
/// worth of retained publishes repeated for no change. An empty slice is the
/// ordinary case and reduces to a plain announcement.
///
/// The superseded configurations are the ones the device previously announced
/// under, which the firmware recovers from its configuration ring rather than
/// from the running config: the values it needs are the ones it has already
/// overwritten.
///
/// # When only one of the two namespaces moved
///
/// The other one's topics still coincide, so a tombstone lands on an address
/// the announcement is about to use again. That is deliberate and it is safe
/// **because of the order**: Home Assistant sees the entity removed and
/// immediately recreated, and it comes back as itself because
/// [`UniqueId`](crate::UniqueId) does not follow either namespace. Reversed,
/// the device would announce its configuration and then delete it, and the
/// estate would be left with nothing — which is why the order is a property of
/// this function rather than of its caller. `tests/lifecycle.rs::
/// a_tombstone_never_outlives_a_publish_to_the_same_topic` is that check.
///
/// Suppressing the redundant pair would need the announcement's topic set
/// known while the retirement is being emitted, and would buy a moment of
/// steadiness in Home Assistant at the price of making the ordering rule
/// conditional. It is not done.
pub fn reconfigure<'a>(
    superseded: &'a [MqttConfig],
    new: &'a MqttConfig,
    shades: &'a [ShadeId],
    has_tilt: bool,
) -> impl Iterator<Item = Step<'static>> + 'a {
    superseded
        .iter()
        .flat_map(move |old| old.retire(shades))
        .chain(new.announce(shades, has_tilt))
}
