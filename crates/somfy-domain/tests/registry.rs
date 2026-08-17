use somfy_domain::{DomainError, Registry, ShadeConfig, ShadeId, MAX_SHADES};

fn cfg(name: &str, addr: u32) -> ShadeConfig {
    ShadeConfig::new(name, addr).unwrap()
}

/// A provisioned table, as a record holds one: an id and an address per row.
/// Loading it is what a boot does, and the ids it hands over are what Home
/// Assistant's entities are named after.
fn load(table: &[(u8, u32)]) -> Registry {
    let mut r = Registry::new();
    for (id, address) in table {
        r.add_shade_with_id(ShadeId(*id), cfg("s", *address))
            .unwrap();
    }
    r
}

/// Which shade sits at which id, in id order.
fn placement(r: &Registry) -> std::vec::Vec<(u8, u32)> {
    r.shades()
        .map(|(id, shade)| (id.0, shade.config.address))
        .collect()
}

#[test]
fn add_and_fetch_shade() {
    let mut r = Registry::new();
    let id = r.add_shade(cfg("Kitchen", 0x100)).unwrap();
    assert_eq!(r.shade(id).unwrap().config.name.as_str(), "Kitchen");
}

#[test]
fn duplicate_address_rejected() {
    let mut r = Registry::new();
    r.add_shade(cfg("A", 0x100)).unwrap();
    assert!(matches!(
        r.add_shade(cfg("B", 0x100)),
        Err(DomainError::DuplicateAddress)
    ));
}

#[test]
fn capacity_32_shades() {
    let mut r = Registry::new();
    for i in 1..=32u32 {
        r.add_shade(cfg("s", 0x1000 + i)).unwrap();
    }
    assert!(matches!(
        r.add_shade(cfg("x", 0x9999)),
        Err(DomainError::RegistryFull)
    ));
}

#[test]
fn ids_are_stable_across_removals() {
    let mut r = Registry::new();
    let a = r.add_shade(cfg("A", 0x101)).unwrap();
    let b = r.add_shade(cfg("B", 0x102)).unwrap();
    r.remove_shade(a).unwrap();
    assert_eq!(r.shade(b).unwrap().config.name.as_str(), "B");
    assert!(r.shade(a).is_none());
    let c = r.add_shade(cfg("C", 0x103)).unwrap(); // reuses the free slot
    assert_eq!(c, a);
}

// ---------------------------------------------------------------------------
// Ids a caller chooses, rather than ids insertion order chooses.
// ---------------------------------------------------------------------------

#[test]
fn add_shade_with_id_places_at_the_id_asked_for() {
    let mut r = Registry::new();
    let id = r
        .add_shade_with_id(ShadeId(5), cfg("Kitchen", 0x101))
        .unwrap();
    assert_eq!(id, ShadeId(5));
    assert_eq!(r.shade(id).unwrap().config.name.as_str(), "Kitchen");
}

/// The point of the method: the slots below the chosen id stay **empty**, and
/// iteration reports only the shade that exists. A registry that quietly slid
/// the shade down to slot 0 would be the positional behaviour under a new name.
#[test]
fn add_shade_with_id_leaves_the_slots_below_it_free() {
    let r = load(&[(5, 0x101)]);
    for empty in 0..5 {
        assert!(r.shade(ShadeId(empty)).is_none(), "slot {empty}");
    }
    assert_eq!(placement(&r), [(5, 0x101)]);
}

/// The refusal that makes the record the authority: two rows may not claim the
/// same id, because the second would otherwise overwrite the first and a
/// provisioned shade would vanish.
#[test]
fn add_shade_with_id_refuses_an_id_already_taken() {
    let mut r = load(&[(2, 0x101)]);
    assert_eq!(
        r.add_shade_with_id(ShadeId(2), cfg("B", 0x102)),
        Err(DomainError::DuplicateId)
    );
    // And the sitting tenant is untouched.
    assert_eq!(placement(&r), [(2, 0x101)]);
}

/// An id no slot array can hold is a different fault from a taken one, and is
/// named differently. `MAX_SHADES` is one past the last slot; `255` is the
/// widest a `ShadeId` can carry.
#[test]
fn add_shade_with_id_refuses_an_id_past_the_last_slot() {
    let mut r = Registry::new();
    for out_of_range in [MAX_SHADES as u8, 255] {
        assert_eq!(
            r.add_shade_with_id(ShadeId(out_of_range), cfg("A", 0x101)),
            Err(DomainError::IdOutOfRange),
            "ShadeId({out_of_range})"
        );
    }
    // Nothing was placed, and the registry is still empty.
    assert_eq!(placement(&r), []);
}

/// The last in-range slot is in range. Guards the off-by-one in the check
/// above from being written as `>` instead of `>=`.
#[test]
fn add_shade_with_id_accepts_the_last_slot() {
    let mut r = Registry::new();
    let last = (MAX_SHADES - 1) as u8;
    assert_eq!(
        r.add_shade_with_id(ShadeId(last), cfg("A", 0x101)),
        Ok(ShadeId(last))
    );
}

/// The rules the two adds share, they share exactly: an address already in the
/// registry is refused the same way whichever door it comes through, so moving
/// a call site from one to the other cannot change which error a bad address
/// produces.
#[test]
fn both_adds_refuse_a_duplicate_address_alike() {
    let mut r = load(&[(0, 0x101)]);
    assert_eq!(
        r.add_shade_with_id(ShadeId(7), cfg("B", 0x101)),
        Err(DomainError::DuplicateAddress)
    );
    assert_eq!(
        r.add_shade(cfg("B", 0x101)),
        Err(DomainError::DuplicateAddress)
    );
}

/// An out-of-range id is refused *before* the shade itself is looked at: the
/// request cannot be satisfied whatever the config says, and reporting the
/// address would send whoever reads the log to the wrong field.
#[test]
fn an_out_of_range_id_is_reported_before_a_duplicate_address() {
    let mut r = load(&[(0, 0x101)]);
    assert_eq!(
        r.add_shade_with_id(ShadeId(200), cfg("B", 0x101)),
        Err(DomainError::IdOutOfRange)
    );
}

/// `add_shade_with_id` can never report a full registry: an id in range either
/// names a hole, which it fills, or an occupant, which it refuses. Pinned
/// because the two errors would otherwise be easy to conflate at a call site
/// that has to decide whether re-provisioning would help.
#[test]
fn add_shade_with_id_never_reports_a_full_registry() {
    let mut r = Registry::new();
    for slot in 0..MAX_SHADES as u8 {
        r.add_shade_with_id(ShadeId(slot), cfg("s", 0x1000 + slot as u32))
            .unwrap();
    }
    assert_eq!(
        r.add_shade_with_id(ShadeId(0), cfg("x", 0x9999)),
        Err(DomainError::DuplicateId)
    );
}

/// The two adds interoperate: a hole `add_shade_with_id` skipped is ordinary
/// free space, and the lowest-free-slot rule fills it.
#[test]
fn add_shade_fills_a_hole_left_by_add_shade_with_id() {
    let mut r = load(&[(3, 0x103)]);
    assert_eq!(r.add_shade(cfg("A", 0x101)), Ok(ShadeId(0)));
    assert_eq!(placement(&r), [(0, 0x101), (3, 0x103)]);
}

/// **The bug this exists for.** A table of three shades is provisioned, the
/// middle one is deleted from it, and the board reboots. With ids taken from
/// the record, the survivors keep the ids they had — so their MQTT discovery
/// topics, and therefore their Home Assistant entities, are the same ones. The
/// positional rule renumbers the third shade to 1 and orphans `shade_2`'s
/// retained config on the broker.
#[test]
fn deleting_a_row_does_not_renumber_the_rows_after_it() {
    let before = load(&[(0, 0x101), (1, 0x102), (2, 0x103)]);
    let after = load(&[(0, 0x101), (2, 0x103)]);

    assert_eq!(placement(&before), [(0, 0x101), (1, 0x102), (2, 0x103)]);
    assert_eq!(placement(&after), [(0, 0x101), (2, 0x103)]);

    // What the positional rule got wrong: 0x103 stays at 2 rather than sliding
    // into the hole the deleted shade left.
    assert_eq!(after.shade(ShadeId(2)).unwrap().config.address, 0x103);
    assert!(after.shade(ShadeId(1)).is_none());
}

/// And the same for a reordered table: the row order in the record is not the
/// id order, so a table sorted differently by a host tool places every shade
/// exactly where it was.
#[test]
fn reordering_the_rows_does_not_move_any_shade() {
    let in_order = load(&[(0, 0x101), (1, 0x102), (2, 0x103)]);
    let shuffled = load(&[(2, 0x103), (0, 0x101), (1, 0x102)]);
    assert_eq!(placement(&in_order), placement(&shuffled));
}

#[test]
fn shade_by_address_matches_own_and_linked() {
    let mut r = Registry::new();
    let id = r.add_shade(cfg("A", 0x101)).unwrap();
    r.shade_mut(id).unwrap().link_remote(0x202).unwrap();
    assert_eq!(r.shade_by_address(0x101), Some(id));
    assert_eq!(r.shade_by_address(0x202), Some(id));
    assert_eq!(r.shade_by_address(0x999), None);
}

#[test]
fn groups_collect_shades_and_forget_removed_ones() {
    let mut r = Registry::new();
    let a = r.add_shade(cfg("A", 0x101)).unwrap();
    let b = r.add_shade(cfg("B", 0x102)).unwrap();
    let g = r.add_group("South").unwrap();
    r.group_add_shade(g, a).unwrap();
    r.group_add_shade(g, b).unwrap();
    assert_eq!(r.group_shades(g).count(), 2);
    r.remove_shade(a).unwrap();
    let members: std::vec::Vec<_> = r.group_shades(g).collect();
    assert_eq!(members, [b]);
}

#[test]
fn rooms_forget_removed_shades() {
    let mut r = Registry::new();
    let a = r.add_shade(cfg("A", 0x101)).unwrap();
    let rm = r.add_room("Living").unwrap();
    r.room_assign(rm, a).unwrap();
    assert_eq!(r.room_shades(rm).count(), 1);
    r.remove_shade(a).unwrap();
    assert_eq!(r.room_shades(rm).count(), 0);
}

#[test]
fn room_assignment_is_exclusive() {
    let mut r = Registry::new();
    let a = r.add_shade(cfg("A", 0x101)).unwrap();
    let r1 = r.add_room("Living").unwrap();
    let r2 = r.add_room("Bed").unwrap();
    r.room_assign(r1, a).unwrap();
    r.room_assign(r2, a).unwrap(); // moves, not duplicates
    assert_eq!(r.room_shades(r1).count(), 0);
    assert_eq!(r.room_shades(r2).count(), 1);
}

#[test]
fn group_and_room_capacities() {
    let mut r = Registry::new();
    for i in 0..16 {
        r.add_group("g").unwrap();
        r.add_room("r").unwrap();
        let _ = i;
    }
    assert!(matches!(r.add_group("x"), Err(DomainError::RegistryFull)));
    assert!(matches!(r.add_room("x"), Err(DomainError::RegistryFull)));
}

#[test]
fn group_and_room_names_are_readable() {
    let mut r = Registry::new();
    let g = r.add_group("South").unwrap();
    let rm = r.add_room("Living").unwrap();
    assert_eq!(r.group_name(g), Some("South"));
    assert_eq!(r.room_name(rm), Some("Living"));
}

#[test]
fn names_of_absent_group_and_room_are_none() {
    use somfy_domain::{GroupId, RoomId};
    let r = Registry::new();
    assert_eq!(r.group_name(GroupId(0)), None);
    assert_eq!(r.room_name(RoomId(0)), None);
}

#[test]
fn remove_missing_shade_is_not_found() {
    let mut r = Registry::new();
    let a = r.add_shade(cfg("A", 0x101)).unwrap();
    r.remove_shade(a).unwrap();
    // Removing an already-empty slot, and an out-of-range slot, both fail.
    assert!(matches!(r.remove_shade(a), Err(DomainError::NotFound)));
    assert!(matches!(
        r.remove_shade(somfy_domain::ShadeId(31)),
        Err(DomainError::NotFound)
    ));
}

#[test]
fn group_add_shade_rejects_unknown_shade_or_group() {
    let mut r = Registry::new();
    let a = r.add_shade(cfg("A", 0x101)).unwrap();
    let g = r.add_group("G").unwrap();
    assert!(matches!(
        r.group_add_shade(g, somfy_domain::ShadeId(9)),
        Err(DomainError::NotFound)
    ));
    assert!(matches!(
        r.group_add_shade(somfy_domain::GroupId(9), a),
        Err(DomainError::NotFound)
    ));
}

#[test]
fn group_add_shade_is_idempotent() {
    let mut r = Registry::new();
    let a = r.add_shade(cfg("A", 0x101)).unwrap();
    let g = r.add_group("G").unwrap();
    r.group_add_shade(g, a).unwrap();
    r.group_add_shade(g, a).unwrap(); // no duplicate
    assert_eq!(r.group_shades(g).count(), 1);
}

#[test]
fn room_assign_rejects_unknown_shade_or_room() {
    let mut r = Registry::new();
    let a = r.add_shade(cfg("A", 0x101)).unwrap();
    let rm = r.add_room("R").unwrap();
    assert!(matches!(
        r.room_assign(rm, somfy_domain::ShadeId(9)),
        Err(DomainError::NotFound)
    ));
    assert!(matches!(
        r.room_assign(somfy_domain::RoomId(9), a),
        Err(DomainError::NotFound)
    ));
}

#[test]
fn overlong_group_and_room_names_rejected() {
    let mut r = Registry::new();
    let long = "x".repeat(33);
    assert!(matches!(r.add_group(&long), Err(DomainError::NameTooLong)));
    assert!(matches!(r.add_room(&long), Err(DomainError::NameTooLong)));
}

#[test]
fn shades_iterates_live_slots_only() {
    let mut r = Registry::new();
    let a = r.add_shade(cfg("A", 0x101)).unwrap();
    let b = r.add_shade(cfg("B", 0x102)).unwrap();
    r.remove_shade(a).unwrap();
    let live: std::vec::Vec<_> = r.shades().map(|(id, _)| id).collect();
    assert_eq!(live, [b]);
}

#[test]
fn group_exists_reflects_presence() {
    let mut r = Registry::new();
    let g = r.add_group("G").unwrap();
    assert!(r.group_exists(g));
    assert!(!r.group_exists(somfy_domain::GroupId(9)));
}
