//! Address allocation and the pairing command.
//!
//! Every MAC below is synthetic. The two boards this scheme was designed
//! against are real hardware and their addresses are live, so the properties
//! are asserted structurally — "no address this allocator can produce is one
//! the OUI-derived scheme could produce" — rather than against literals that
//! would put a real installation's radio addresses in a public repository.

use heapless::Vec;
use somfy_domain::{
    allocate_if_absent, allocate_with, AllocateError, Allocated, Controller, DomainError,
    PairingState, PlannedTx, Pos, Registry, RemoteIdentity, Repeats, Shade, ShadeCommand,
    ShadeConfig, ShadeId, StateDelta, DELTA_CAPACITY, MAX_SHADES, PAIR_REPEATS, TX_CAPACITY,
};
use somfy_rts::Command;

/// Nothing is ever taken.
fn free(_: u32) -> bool {
    false
}

/// A MAC whose first three bytes are a vendor OUI shared by every board of one
/// make, and whose last three are the part that differs between two boards on
/// one bench.
fn mac(unique: [u8; 3]) -> [u8; 6] {
    [0xAA, 0xBB, 0xCC, unique[0], unique[1], unique[2]]
}

/// The derivation this project deliberately did **not** copy, kept here as an
/// executable statement of the defect it avoids: the low 20 bits of the
/// little-endian eFuse MAC word are the OUI, which is a property of the chip
/// vendor and identical on every board they ship.
fn oui_derived_start(mac: [u8; 6]) -> u32 {
    (((mac[2] as u32) << 16) | ((mac[1] as u32) << 8) | mac[0] as u32) & 0x0F_FFFF
}

// ---------------------------------------------------------------------------
// The defect, and that this allocator does not have it
// ---------------------------------------------------------------------------

/// Two boards of one make differ only in the device-unique half of the MAC, so
/// a derivation that reads the OUI hands them the same starting address. This
/// is the collision that put two controllers on one identity with two
/// independent rolling-code counters.
#[test]
fn the_oui_derivation_gives_two_different_boards_the_same_start() {
    let one = mac([0x01, 0x02, 0x03]);
    let two = mac([0x01, 0x02, 0x04]);
    assert_ne!(one, two, "the two boards must differ somewhere");
    assert_eq!(
        oui_derived_start(one),
        oui_derived_start(two),
        "this is the defect being avoided, not a property to preserve",
    );
}

/// The same two boards, through this allocator: different bases, and therefore
/// no shade of one can ever be addressed by the other.
#[test]
fn two_boards_differing_only_in_the_device_half_get_different_bases() {
    let one = RemoteIdentity::from_mac(mac([0x01, 0x02, 0x03]));
    let two = RemoteIdentity::from_mac(mac([0x01, 0x02, 0x04]));
    assert_ne!(one.base(), two.base());
}

/// The fold is what makes the high nibble of the first device-unique byte
/// count. Without it these two MACs — which differ only there — would share a
/// base, because the low twenty bits of both are identical.
#[test]
fn the_top_nibble_of_the_device_half_changes_the_base() {
    let low = RemoteIdentity::from_mac(mac([0x0A, 0x00, 0x00]));
    let high = RemoteIdentity::from_mac(mac([0x1A, 0x00, 0x00]));
    assert_eq!(
        (0x0A_00_00u32) & 0x0F_FFFF,
        (0x1A_00_00u32) & 0x0F_FFFF,
        "the premise: truncation alone would lose this difference",
    );
    assert_ne!(low.base(), high.base());
}

/// Every byte of the vendor OUI is ignored, so two boards from different makes
/// that happen to share a serial land on the same base — and that is correct.
/// The OUI is exactly the part that carries no device identity.
#[test]
fn the_vendor_oui_does_not_reach_the_address() {
    let a = RemoteIdentity::from_mac([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    let b = RemoteIdentity::from_mac([0xFE, 0xDC, 0xBA, 0x33, 0x44, 0x55]);
    assert_eq!(a.base(), b.base());
}

// ---------------------------------------------------------------------------
// The space, and why it cannot collide with the OUI-derived one
// ---------------------------------------------------------------------------

/// The structural non-collision proof, checked rather than argued.
///
/// An OUI-derived start is a 20-bit value, and the scheme adds a shade id to
/// it, so the widest address that scheme can produce is
/// `0x0F_FFFF + u8::MAX`. Every address this allocator produces is above that,
/// for every MAC and every shade id — so no address of ours is reachable by
/// **any** controller allocating the OUI-derived way, not merely by the one
/// board that prompted this.
#[test]
fn no_allocated_address_is_reachable_by_the_oui_derivation() {
    const WIDEST_OUI_DERIVED: u32 = 0x0F_FFFF + u8::MAX as u32;
    // A compile-time check, because the claim is about two constants and a
    // runtime assertion on those is one clippy rightly refuses to let stand.
    const { assert!(WIDEST_OUI_DERIVED < RemoteIdentity::SPACE_START) };

    for unique in [
        [0x00, 0x00, 0x00],
        [0xFF, 0xFF, 0xFF],
        [0x38, 0x00, 0x01],
        [0x7A, 0x5C, 0x91],
    ] {
        let identity = RemoteIdentity::from_mac(mac(unique));
        for id in 0..=u8::MAX {
            let address = identity
                .address_for(ShadeId(id), free)
                .expect("nothing is taken");
            assert!(
                address > WIDEST_OUI_DERIVED,
                "{address:#08X} is inside the space an OUI-derived allocator can reach",
            );
        }
    }
}

/// Every allocated address is one `ShadeConfig` will accept: neither sentinel,
/// and inside 24 bits. An allocator that can produce an address the domain
/// refuses is an allocator that provisions a shade nothing can command.
#[test]
fn every_allocated_address_is_one_the_domain_accepts() {
    for unique in [[0x00, 0x00, 0x00], [0xFF, 0xFF, 0xFF], [0x0F, 0xF0, 0x0F]] {
        let identity = RemoteIdentity::from_mac(mac(unique));
        for id in 0..=u8::MAX {
            let address = identity
                .address_for(ShadeId(id), free)
                .expect("nothing is taken");
            assert!(
                address < 0xFF_FFFF,
                "{address:#08X} is at or past the ceiling"
            );
            ShadeConfig::new("probe", address)
                .unwrap_or_else(|e| panic!("{address:#08X} refused by ShadeConfig: {e:?}"));
        }
    }
}

/// Distinct shades of one board get distinct addresses when the table is
/// empty — the property the whole scheme exists for.
#[test]
fn distinct_shades_of_one_board_get_distinct_addresses() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut seen: std::vec::Vec<u32> = std::vec::Vec::new();
    for id in 0..MAX_SHADES {
        let address = identity
            .address_for(ShadeId(id as u8), free)
            .expect("nothing is taken");
        assert!(!seen.contains(&address), "{address:#08X} allocated twice");
        seen.push(address);
    }
}

// ---------------------------------------------------------------------------
// Collision avoidance within our own table
// ---------------------------------------------------------------------------

/// An address already in the table is stepped over, exactly as a table filled
/// from an import — where addresses came from somewhere else entirely — needs.
#[test]
fn an_address_already_in_the_table_is_stepped_over() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let first = identity.address_for(ShadeId(3), free).unwrap();
    let next = identity
        .address_for(ShadeId(3), |a| a == first)
        .expect("one taken address leaves plenty");
    assert_eq!(next, first + 1);
}

/// A run of taken addresses is walked past rather than giving up at the first.
#[test]
fn a_run_of_taken_addresses_is_walked_past() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let first = identity.address_for(ShadeId(0), free).unwrap();
    let taken = |a: u32| a < first + 5;
    assert_eq!(identity.address_for(ShadeId(0), taken), Some(first + 5));
}

/// The probe is bounded, so an allocator handed a predicate that is not backed
/// by a registry says so instead of looping. A registry holds at most
/// [`MAX_SHADES`] addresses, so this cannot happen with a real one — which is
/// what the next test asserts.
#[test]
fn an_everything_taken_predicate_is_refused_rather_than_looped_on() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    assert_eq!(identity.address_for(ShadeId(0), |_| true), None);
}

/// A full table still leaves an address, because the probe is one wider than
/// the registry is deep. This is the bound that makes `None` unreachable in
/// practice.
#[test]
fn a_full_table_still_yields_an_address() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    // The worst case for a shade at id 0: every address from its first
    // candidate upward is held by another shade.
    let first = identity.address_for(ShadeId(0), free).unwrap();
    let full: std::vec::Vec<u32> = (0..MAX_SHADES as u32).map(|k| first + k).collect();
    let address = identity
        .address_for(ShadeId(0), |a| full.contains(&a))
        .expect("MAX_SHADES taken addresses cannot exhaust MAX_SHADES + 1 candidates");
    assert!(!full.contains(&address));
}

// ---------------------------------------------------------------------------
// The ownership predicate
// ---------------------------------------------------------------------------

/// Everything the allocator can produce reports as allocated, for every shade
/// id and a spread of MACs — not just for the one board that prompted the
/// question.
#[test]
fn every_address_this_allocator_produces_reports_as_allocated() {
    for unique in [
        [0x00, 0x00, 0x00],
        [0xFF, 0xFF, 0xFF],
        [0x12, 0x34, 0x56],
        [0xF0, 0x0D, 0x1E],
        [0x00, 0x00, 0x01],
    ] {
        let identity = RemoteIdentity::from_mac(mac(unique));
        for id in 0..MAX_SHADES as u8 {
            let address = identity.address_for(ShadeId(id), free).expect("an address");
            assert!(
                RemoteIdentity::is_allocated(address),
                "{address:#08X} from {unique:?}/{id}",
            );
        }
    }
}

/// And nothing the *other* scheme can produce does. This is the same
/// structural separation `no_allocated_address_is_reachable_by_the_oui_derivation`
/// asserts, read from the predicate's side: an imported table carries addresses
/// a 20-bit prefix derivation produced, plus at most a `u8` shade offset, and
/// none of that reaches bit 23.
#[test]
fn no_address_the_other_scheme_can_produce_reports_as_allocated() {
    for unique in [[0x00, 0x00, 0x00], [0xFF, 0xFF, 0xFF], [0x12, 0x34, 0x56]] {
        let start = oui_derived_start(mac(unique));
        for id in 0..=u8::MAX as u32 {
            assert!(
                !RemoteIdentity::is_allocated(start + id),
                "{:#08X}",
                start + id,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Allocating once, and never again
// ---------------------------------------------------------------------------

/// The rule that costs a physical re-pairing when it is broken: a shade's
/// address does not move, whatever it is asked afterwards.
#[test]
fn an_allocated_address_is_never_reallocated() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();

    let first = allocate_if_absent(&mut registry, &identity, ShadeId(0), "Kitchen").unwrap();
    assert!(matches!(first, Allocated::Fresh(_)));

    // Asked again, with a different name, ten times over.
    for _ in 0..10 {
        let again = allocate_if_absent(&mut registry, &identity, ShadeId(0), "Something Else");
        assert_eq!(again, Ok(Allocated::Kept(first.address())));
    }
    let shade = registry.shade(ShadeId(0)).expect("still there");
    assert_eq!(shade.config.address, first.address());
    assert_eq!(shade.config.name.as_str(), "Kitchen");
}

/// Adding a shade beside one that already exists does not disturb it, and the
/// two get different addresses.
#[test]
fn a_second_shade_gets_its_own_address_and_leaves_the_first_alone() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();

    let one = allocate_if_absent(&mut registry, &identity, ShadeId(0), "Kitchen").unwrap();
    let two = allocate_if_absent(&mut registry, &identity, ShadeId(1), "Salon").unwrap();

    assert_ne!(one.address(), two.address());
    assert_eq!(
        registry.shade(ShadeId(0)).unwrap().config.address,
        one.address()
    );
    assert_eq!(
        registry.shade(ShadeId(1)).unwrap().config.address,
        two.address()
    );
}

/// An imported table is exactly the case the probe exists for: its addresses
/// belong to another controller, and allocating over one would be two
/// controllers transmitting as one remote.
#[test]
fn an_allocation_steps_over_an_imported_address() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();

    // Put a shade at the address the allocator would otherwise hand to id 0 —
    // an imported row would do this whenever the two spaces happened to meet.
    let wanted = identity.address_for(ShadeId(0), free).unwrap();
    registry
        .add_shade_with_id(ShadeId(7), ShadeConfig::new("Imported", wanted).unwrap())
        .unwrap();

    let fresh = allocate_if_absent(&mut registry, &identity, ShadeId(0), "Kitchen").unwrap();
    assert_ne!(fresh.address(), wanted);
    assert_eq!(registry.shade(ShadeId(7)).unwrap().config.address, wanted);
}

/// A wall remote's address is as taken as a shade's own.
///
/// If the allocator handed itself an address a linked remote already uses, the
/// controller could not tell its own frames from the remote's: every burst it
/// sent would come back as an overheard press and move the position estimate a
/// second time. The remote is a physical object in somebody's hall and cannot
/// be renumbered; the allocation can.
#[test]
fn an_allocation_steps_over_a_linked_remotes_address() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();

    let wanted = identity.address_for(ShadeId(0), free).unwrap();
    registry
        .add_shade_with_id(ShadeId(7), ShadeConfig::new("Imported", 0x00_1001).unwrap())
        .unwrap();
    registry
        .shade_mut(ShadeId(7))
        .unwrap()
        .link_remote(wanted)
        .unwrap();

    let fresh = allocate_if_absent(&mut registry, &identity, ShadeId(0), "Kitchen").unwrap();
    assert_ne!(
        fresh.address(),
        wanted,
        "the allocator took an address a wall remote already transmits at",
    );
}

/// An id no slot answers to is refused, rather than being wrapped or clamped
/// onto some other shade's slot.
#[test]
fn an_id_past_the_registry_is_refused() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();
    assert_eq!(
        allocate_if_absent(&mut registry, &identity, ShadeId(MAX_SHADES as u8), "X"),
        Err(DomainError::IdOutOfRange),
    );
}

/// A freshly allocated shade is one the persisted record and the registry both
/// accept: the address is in range, is not a sentinel, and carries the marker.
#[test]
fn a_fresh_allocation_is_a_shade_everything_downstream_accepts() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();
    for id in 0..MAX_SHADES as u8 {
        let fresh = allocate_if_absent(&mut registry, &identity, ShadeId(id), "S").unwrap();
        let address = fresh.address();
        assert!(RemoteIdentity::is_allocated(address));
        assert!(ShadeConfig::new("S", address).is_ok());
    }
    assert_eq!(registry.shades().count(), MAX_SHADES);
}

/// A name the registry cannot hold is refused **before** anything is placed, so
/// a failed add does not leave a half-created shade at a burnt address.
#[test]
fn a_refused_name_allocates_nothing() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();
    let long = "x".repeat(33);
    assert_eq!(
        allocate_if_absent(&mut registry, &identity, ShadeId(0), &long),
        Err(DomainError::NameTooLong),
    );
    assert!(registry.shade(ShadeId(0)).is_none());
    // And the address it would have taken is still free for the next attempt.
    let fresh = allocate_if_absent(&mut registry, &identity, ShadeId(0), "Kitchen").unwrap();
    assert_eq!(
        fresh,
        Allocated::Fresh(identity.address_for(ShadeId(0), free).unwrap())
    );
}

// ---------------------------------------------------------------------------
// The pairing command
// ---------------------------------------------------------------------------

fn shade() -> Shade {
    Shade::new(ShadeConfig::new("Test", 0x00_C0DE).unwrap())
}

fn tx(out: &Vec<PlannedTx, 4>) -> std::vec::Vec<Command> {
    out.iter().map(|t| t.command).collect()
}

/// Pairing plans one `Prog` frame at the shade's own address, and pins its
/// repeat count: the repeat count is how long the PROG button is held, and a
/// long hold **removes** a remote instead of adding one.
#[test]
fn pair_plans_one_prog_frame_at_the_shades_own_address() {
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(ShadeCommand::Pair, 0, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].command, Command::Prog);
    assert_eq!(out[0].address, 0x00_C0DE);
    assert_eq!(out[0].repeats, Repeats::Exactly(PAIR_REPEATS));
}

/// A configured profile cannot inflate a pairing burst into an unpairing one.
/// This is the whole reason the repeat count is a policy rather than a number.
#[test]
fn a_generous_profile_cannot_turn_a_pairing_into_an_unpairing() {
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(ShadeCommand::Pair, 0, &mut out);
    for profile in [0, 1, 2, 7, 20, u8::MAX] {
        assert_eq!(out[0].repeats.resolve(profile), PAIR_REPEATS);
    }
}

/// Pairing is not motion. The estimator must not treat a `Prog` frame as a
/// command that moves the lift, or a paired shade reports a position it never
/// travelled to.
#[test]
fn pairing_moves_neither_the_position_nor_the_target() {
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(40)), 0, &mut out);
    s.tick(4_000, &mut out);
    out.clear();

    let (pos, target, direction) = (s.pos(), s.target(), s.direction());
    s.handle(ShadeCommand::Pair, 4_000, &mut out);
    assert_eq!(s.pos(), pos);
    assert_eq!(s.target(), target);
    assert_eq!(s.direction(), direction);
    assert_eq!(out.len(), 1, "only the pairing frame");
}

/// An ordinary command still takes whatever the controller is configured to
/// send, so adding a policy did not quietly hard-code a repeat count into the
/// domain.
#[test]
fn an_ordinary_command_defers_to_the_configured_profile() {
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(ShadeCommand::Up, 0, &mut out);
    assert_eq!(out[0].repeats, Repeats::Profile);
    for profile in [0, 2, 9] {
        assert_eq!(out[0].repeats.resolve(profile), profile);
    }
}

/// The other half of the capability, for the arrival-stop frame that
/// `docs/specs/2026-08-15-position-accuracy-requirements.md` R1 needs: a floor
/// that a generous profile raises and a mean one cannot lower.
#[test]
fn at_least_is_a_floor_the_profile_can_raise_but_not_lower() {
    assert_eq!(Repeats::AtLeast(5).resolve(0), 5);
    assert_eq!(Repeats::AtLeast(5).resolve(4), 5);
    assert_eq!(Repeats::AtLeast(5).resolve(5), 5);
    assert_eq!(Repeats::AtLeast(5).resolve(9), 9);
}

/// **Pairing disarms a pending arrival stop.**
///
/// A mid-range seek arms a `My` that [`Shade::tick`] transmits when the
/// estimate says the target is reached. Pairing means a person has just put
/// this motor into programming mode, where `My` is not a stop at all — it is
/// how a favourite position is *stored*. So the pending stop is dropped rather
/// than allowed to fire into that window.
///
/// The trade is deliberate and one-sided: dropping it leaves the shade
/// travelling to its physical limit, which any later command undoes, while
/// firing it silently rewrites a setting inside the motor that only a visit to
/// the shade can undo.
#[test]
fn pairing_disarms_a_pending_arrival_stop() {
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    // A seek from 0% to 40% takes 4 s at the default travel time, so pairing at
    // 1 s lands squarely mid-seek with the stop armed.
    s.handle(ShadeCommand::GoTo(Pos::from_percent(40)), 0, &mut out);
    out.clear();

    s.handle(ShadeCommand::Pair, 1_000, &mut out);
    assert_eq!(tx(&out), [Command::Prog], "only the pairing frame");
    out.clear();

    s.tick(4_000, &mut out);
    assert!(
        out.is_empty(),
        "a My reached the air inside the programming window: {:?}",
        tx(&out),
    );
}

/// The same property from the receive side, and the reason
/// [`Shade::apply_overheard`] clears the flag before it looks at the command at
/// all.
///
/// Step one of the documented pairing procedure is a PROG press on an existing
/// wall remote. If that remote is linked, the frame arrives here — so this is
/// the *ordinary* path, not an exotic one.
#[test]
fn an_overheard_prog_frame_plans_no_stop_into_a_programming_window() {
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(40)), 0, &mut out);
    out.clear();

    s.apply_overheard(Command::Prog, 1_000);
    s.tick(4_000, &mut out);
    assert!(
        out.is_empty(),
        "a My reached the air inside the programming window: {:?}",
        tx(&out),
    );
}

/// Overheard pairing traffic — ours or a neighbour's wall remote in
/// programming mode — must not move the estimate either.
#[test]
fn an_overheard_prog_frame_moves_nothing() {
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(40)), 0, &mut out);
    s.tick(4_000, &mut out);

    let (pos, target, direction) = (s.pos(), s.target(), s.direction());
    s.apply_overheard(Command::Prog, 4_000);
    assert_eq!(s.pos(), pos);
    assert_eq!(s.target(), target);
    assert_eq!(s.direction(), direction);
}

/// **Pairing cannot be fanned out to a group.**
///
/// `ShadeCommand` is one enum and `Controller::command_group` takes any of it,
/// so without this guard a single call could put a `Prog` burst on the air at
/// every shade in the house. Every other command here is a movement somebody
/// can watch and undo; this one is the only shape of the command with no human
/// at any of the shades, and the only one whose effect a later command cannot
/// reverse.
///
/// Refused structurally rather than left to whatever happens to construct a
/// `ControlCommand` today, which is the same standard `Repeats::Exactly` holds
/// the burst length to.
#[test]
fn a_group_cannot_be_told_to_pair() {
    let mut c = Controller::new();
    let a = c
        .registry
        .add_shade(ShadeConfig::new("A", 0x00_C0DE).unwrap())
        .unwrap();
    let b = c
        .registry
        .add_shade(ShadeConfig::new("B", 0x00_C0DF).unwrap())
        .unwrap();
    let group = c.registry.add_group("All").unwrap();
    c.registry.group_add_shade(group, a).unwrap();
    c.registry.group_add_shade(group, b).unwrap();

    let mut plans: Vec<PlannedTx, TX_CAPACITY> = Vec::new();
    let mut deltas: Vec<StateDelta, DELTA_CAPACITY> = Vec::new();
    assert_eq!(
        c.command_group(group, ShadeCommand::Pair, 0, &mut plans, &mut deltas),
        Err(DomainError::NotAGroupCommand),
    );
    assert!(plans.is_empty(), "nothing may reach the air");

    // Every other command still fans out, so the guard is about this command
    // rather than about group commands in general.
    assert!(c
        .command_group(group, ShadeCommand::Down, 0, &mut plans, &mut deltas)
        .is_ok());
    assert_eq!(plans.len(), 2);
}

/// A single shade can still be paired — the guard is on the fan-out, not on the
/// command.
#[test]
fn one_shade_can_still_be_paired_through_the_controller() {
    let mut c = Controller::new();
    let id = c
        .registry
        .add_shade(ShadeConfig::new("A", 0x00_C0DE).unwrap())
        .unwrap();
    let mut plans: Vec<PlannedTx, TX_CAPACITY> = Vec::new();
    let mut deltas: Vec<StateDelta, DELTA_CAPACITY> = Vec::new();
    c.command_shade(id, ShadeCommand::Pair, 0, &mut plans, &mut deltas)
        .unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].command, Command::Prog);
}

// ---------------------------------------------------------------------------
// `allocate_with` — the same allocation, with the caller building the config
// ---------------------------------------------------------------------------

/// The configuration is built at the address the allocator chose, and the shade
/// exists holding it. This is the property that stops a freshly created shade
/// spending an instant with the factory travel times — which a reader taking a
/// snapshot in that instant would report as *uncalibrated*.
#[test]
fn allocate_with_places_the_configuration_the_caller_built() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();

    let made = allocate_with(&mut registry, &identity, ShadeId(0), |address| {
        let mut config = ShadeConfig::new("Kitchen", address)?;
        config.up_time_ms = 30_000;
        config.down_time_ms = 27_000;
        Ok::<_, DomainError>(config)
    })
    .unwrap();

    let shade = registry.shade(ShadeId(0)).unwrap();
    assert_eq!(made, Allocated::Fresh(shade.config.address));
    assert_eq!(shade.config.up_time_ms, 30_000);
    assert_eq!(shade.config.down_time_ms, 27_000);
    assert!(RemoteIdentity::is_allocated(shade.config.address));
}

/// A refusal from the caller's own validation leaves **nothing** behind: no
/// shade, and — the part that matters — no address spent. An address burned by
/// a rejected request is one the next shade does not get, and nothing in a
/// one-way protocol can give it back.
#[test]
fn a_refused_description_leaves_no_shade_and_spends_no_address() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();

    let refused = allocate_with(&mut registry, &identity, ShadeId(0), |_| {
        Err::<ShadeConfig, &str>("this caller refuses it")
    });

    assert_eq!(
        refused,
        Err(AllocateError::Description("this caller refuses it"))
    );
    assert!(registry.shade(ShadeId(0)).is_none());

    // The next attempt gets the address the refused one would have had, which
    // is what "spends no address" means concretely.
    let after = allocate_with(&mut registry, &identity, ShadeId(0), |address| {
        ShadeConfig::new("Kitchen", address)
    })
    .unwrap();
    assert_eq!(after, Allocated::Fresh(identity.base()));
}

/// An occupied slot is handed back untouched, with the caller's builder never
/// run — the read-first rule `allocate_if_absent` documents, inherited rather
/// than restated.
#[test]
fn allocate_with_never_reallocates_an_occupied_slot() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();

    let first = allocate_with(&mut registry, &identity, ShadeId(0), |address| {
        ShadeConfig::new("Kitchen", address)
    })
    .unwrap();

    let mut ran = false;
    let again = allocate_with(&mut registry, &identity, ShadeId(0), |address| {
        ran = true;
        ShadeConfig::new("Renamed", address)
    })
    .unwrap();

    assert!(!ran, "the builder must not run for an occupied slot");
    assert_eq!(again, Allocated::Kept(first.address()));
    assert_eq!(
        registry.shade(ShadeId(0)).map(|s| s.config.name.as_str()),
        Some("Kitchen"),
    );
}

/// `allocate_if_absent` is a call to `allocate_with` now, and still reports a
/// domain refusal as a bare [`DomainError`] — the flattening is what keeps its
/// existing callers unchanged.
#[test]
fn allocate_if_absent_still_reports_a_bare_domain_error() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();
    assert_eq!(
        allocate_if_absent(
            &mut registry,
            &identity,
            ShadeId(MAX_SHADES as u8),
            "Kitchen"
        ),
        Err(DomainError::IdOutOfRange),
    );
}

// ---------------------------------------------------------------------------
// What an operator's report is, and what it is not
// ---------------------------------------------------------------------------

/// A freshly allocated shade is **awaiting confirmation**, and the reason is
/// the address: the allocator invented it, so no motor has ever heard it.
///
/// This is what stops a created shade being announced to Home Assistant.
/// Asserted at the allocator rather than only at the constructor, because that
/// is the path the API actually takes.
#[test]
fn a_freshly_allocated_shade_is_awaiting_confirmation() {
    let identity = RemoteIdentity::from_mac(mac([0x12, 0x34, 0x56]));
    let mut registry = Registry::new();
    let made = allocate_with(&mut registry, &identity, ShadeId(0), |address| {
        ShadeConfig::new("Kitchen", address)
    })
    .unwrap();
    assert!(matches!(made, Allocated::Fresh(_)));

    assert_eq!(
        registry.shade(ShadeId(0)).unwrap().config.pairing_state,
        PairingState::AwaitingConfirmation,
    );
}

/// **The gate everything that publishes a shade walks through.**
///
/// `confirmed_shades` is a subset of `shades`, and the difference is the whole
/// feature: the Home Assistant announcement walks the former, so a shade nobody
/// has reported working has no entities — while the local API walks the latter,
/// because an unconfirmed shade has to be commandable or there would be no way
/// to test it and therefore no way to ever confirm it.
#[test]
fn only_confirmed_shades_are_the_ones_anything_publishes() {
    let mut registry = Registry::new();
    let awaiting = registry
        .add_shade(ShadeConfig::new("Fresh", 0x80_0001).unwrap())
        .unwrap();
    let confirmed = registry
        .add_shade(ShadeConfig::new("Working", 0x80_0002).unwrap())
        .unwrap();
    registry.shade_mut(confirmed).unwrap().confirm_pairing();

    let all: std::vec::Vec<ShadeId> = registry.shades().map(|(id, _)| id).collect();
    assert_eq!(all, std::vec![awaiting, confirmed], "both are commandable");

    let publishable: std::vec::Vec<ShadeId> =
        registry.confirmed_shades().map(|(id, _)| id).collect();
    assert_eq!(
        publishable,
        std::vec![confirmed],
        "a shade nobody has reported working must not reach an announcement",
    );

    // And it moves the moment somebody says so, without anything else changing.
    registry.shade_mut(awaiting).unwrap().confirm_pairing();
    let publishable: std::vec::Vec<ShadeId> =
        registry.confirmed_shades().map(|(id, _)| id).collect();
    assert_eq!(publishable, std::vec![awaiting, confirmed]);
}

/// **The complement of that gate, and it partitions.**
///
/// `unconfirmed_shades` exists so a device can say "a setup was started and not
/// finished" without offering a control on a shade that would transmit and move
/// nothing. That claim is only worth anything if the two halves are exactly the
/// whole: a shade in neither is one nothing reports at all, and a shade in both
/// would be counted as pending while its cover was live.
///
/// So this asserts the partition rather than the count — which is the thing a
/// `count()` at a call site cannot check for itself.
#[test]
fn confirmed_and_unconfirmed_partition_the_registry() {
    let mut registry = Registry::new();
    let awaiting = registry
        .add_shade(ShadeConfig::new("Fresh", 0x80_0001).unwrap())
        .unwrap();
    let confirmed = registry
        .add_shade(ShadeConfig::new("Working", 0x80_0002).unwrap())
        .unwrap();
    let second_awaiting = registry
        .add_shade(ShadeConfig::new("Also fresh", 0x80_0003).unwrap())
        .unwrap();
    registry.shade_mut(confirmed).unwrap().confirm_pairing();

    let pending: std::vec::Vec<ShadeId> = registry.unconfirmed_shades().map(|(id, _)| id).collect();
    assert_eq!(pending, std::vec![awaiting, second_awaiting]);

    // Disjoint, and together the whole.
    let mut both: std::vec::Vec<ShadeId> = registry
        .confirmed_shades()
        .chain(registry.unconfirmed_shades())
        .map(|(id, _)| id)
        .collect();
    both.sort_unstable_by_key(|id| id.0);
    let all: std::vec::Vec<ShadeId> = registry.shades().map(|(id, _)| id).collect();
    assert_eq!(both, all, "every shade is in exactly one half");

    // Confirming one moves it across, and moves nothing else.
    registry.shade_mut(awaiting).unwrap().confirm_pairing();
    let pending: std::vec::Vec<ShadeId> = registry.unconfirmed_shades().map(|(id, _)| id).collect();
    assert_eq!(pending, std::vec![second_awaiting]);

    // An empty registry is not a special case: nothing pending, nothing live.
    let empty = Registry::new();
    assert_eq!(empty.unconfirmed_shades().count(), 0);
}

/// Confirming is the operator's report, and it is news exactly once.
#[test]
fn confirming_is_news_once_and_then_a_no_op() {
    let mut s = shade();
    assert!(s.confirm_pairing(), "the first report is news");
    assert_eq!(s.config.pairing_state, PairingState::ConfirmedByOperator);
    assert!(!s.confirm_pairing(), "a repeat changes nothing");
}

/// **Confirmation transmits nothing and moves nothing.**
///
/// It is a record of what somebody saw, not an action on a motor: no frame is
/// planned and the position estimate is untouched. A confirmation that planned
/// a frame would put a burst on the air because somebody clicked "yes, it
/// moved" — at a shade they are standing next to, possibly in programming mode.
#[test]
fn confirming_plans_no_frame_and_moves_no_position() {
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(40)), 0, &mut out);
    out.clear();
    let before = (s.pos(), s.target());

    s.confirm_pairing();

    assert!(out.is_empty(), "confirming is not a transmission");
    assert_eq!((s.pos(), s.target()), before);
}

/// An edit is not a report, in either direction.
///
/// A rename must not confirm a shade nobody has watched move, and a corrected
/// travel time must not un-confirm one that works — the second would retire a
/// live Home Assistant entity out from under whatever is automating it. The
/// same protection `address` already has, for the same reason: the incoming
/// configuration is a client's, and this field is not a client's to set.
#[test]
fn reconfiguring_carries_neither_direction_of_the_report() {
    let mut s = shade();

    let mut claiming = s.config.clone();
    claiming.pairing_state = PairingState::ConfirmedByOperator;
    s.reconfigure(claiming, 0);
    assert_eq!(
        s.config.pairing_state,
        PairingState::AwaitingConfirmation,
        "a patch cannot claim a report nobody made",
    );

    s.confirm_pairing();
    let mut denying = s.config.clone();
    denying.pairing_state = PairingState::AwaitingConfirmation;
    denying.up_time_ms = 30_000;
    s.reconfigure(denying, 0);
    assert_eq!(
        s.config.pairing_state,
        PairingState::ConfirmedByOperator,
        "a patch cannot withdraw one either",
    );
    assert_eq!(s.config.up_time_ms, 30_000, "the rest still applied");
}

/// Pairing and confirming are separate acts, and the order is forced.
///
/// The `Prog` burst is planned by a command; the confirmation is a report made
/// afterwards by a person who watched. Transmitting must not confirm anything —
/// that would be the transmitter reporting its own success, which in a one-way
/// protocol is worth nothing at all.
#[test]
fn transmitting_a_pairing_burst_confirms_nothing() {
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(ShadeCommand::Pair, 0, &mut out);

    assert_eq!(tx(&out), std::vec![Command::Prog]);
    assert_eq!(
        s.config.pairing_state,
        PairingState::AwaitingConfirmation,
        "the device cannot observe what the motor did, so it must claim nothing",
    );
}
