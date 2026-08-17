//! Address allocation and the pairing command.
//!
//! Every MAC below is synthetic. The two boards this scheme was designed
//! against are real hardware and their addresses are live, so the properties
//! are asserted structurally — "no address this allocator can produce is one
//! the OUI-derived scheme could produce" — rather than against literals that
//! would put a real installation's radio addresses in a public repository.

use heapless::Vec;
use somfy_domain::{
    Controller, DomainError, PlannedTx, Pos, RemoteIdentity, Repeats, Shade, ShadeCommand,
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
