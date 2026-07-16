use somfy_domain::{DomainError, Registry, ShadeConfig};

fn cfg(name: &str, addr: u32) -> ShadeConfig {
    ShadeConfig::new(name, addr).unwrap()
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
