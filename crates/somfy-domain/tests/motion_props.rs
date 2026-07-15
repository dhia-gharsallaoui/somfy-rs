use proptest::prelude::*;
use somfy_domain::{Direction, Motion, Pos};

proptest! {
    // Position always lands exactly on target given enough time.
    #[test]
    fn always_reaches_target(
        start in 0u16..=10_000,
        target in 0u16..=10_000,
        up in 1_000u32..60_000,
        down in 1_000u32..60_000,
    ) {
        let mut m = Motion::new(Pos::from_raw(start));
        m.set_target(Pos::from_raw(target), 0);
        m.tick(u64::from(up.max(down)) + 1_000, up, down);
        prop_assert_eq!(m.pos(), Pos::from_raw(target));
        prop_assert_eq!(m.direction(), Direction::Idle);
    }

    // Ticking twice at increasing times never moves backwards
    // relative to the travel direction, and never overshoots.
    #[test]
    fn monotonic_and_bounded(
        start in 0u16..=10_000,
        target in 0u16..=10_000,
        t1 in 0u64..30_000,
        dt in 0u64..30_000,
    ) {
        let mut m = Motion::new(Pos::from_raw(start));
        m.set_target(Pos::from_raw(target), 0);
        let a = m.tick(t1, 10_000, 10_000).pos;
        let b = m.tick(t1 + dt, 10_000, 10_000).pos;
        let (lo, hi) = if start <= target {
            (Pos::from_raw(start), Pos::from_raw(target))
        } else {
            (Pos::from_raw(target), Pos::from_raw(start))
        };
        prop_assert!(a >= lo && a <= hi);
        prop_assert!(b >= lo && b <= hi);
        if start <= target { prop_assert!(b >= a); } else { prop_assert!(b <= a); }
    }

    // Halting then waiting never changes position.
    #[test]
    fn halt_is_stable(
        start in 0u16..=10_000,
        target in 0u16..=10_000,
        t_halt in 1u64..9_999,
        t_later in 10_000u64..100_000,
    ) {
        let mut m = Motion::new(Pos::from_raw(start));
        m.set_target(Pos::from_raw(target), 0);
        m.halt(t_halt, 10_000, 10_000);
        let frozen = m.pos();
        let s = m.tick(t_later, 10_000, 10_000);
        prop_assert_eq!(s.pos, frozen);
        prop_assert_eq!(s.direction, Direction::Idle);
    }

    // arrived fires exactly once per completed movement.
    #[test]
    fn arrived_fires_exactly_once(
        start in 0u16..=10_000,
        target in 0u16..=10_000,
        steps in 2usize..20,
    ) {
        prop_assume!(start != target);
        let mut m = Motion::new(Pos::from_raw(start));
        m.set_target(Pos::from_raw(target), 0);
        let mut arrivals = 0;
        // 6_000 ms/tick guarantees the movement completes within `steps`
        // (>= 2) ticks: the full 0..10_000 range needs at most 10_000 ms of a
        // 10_000 ms travel, and 2 * 6_000 > 10_000. Smaller steps (e.g. the
        // brief's 2_000) can end mid-travel, leaving `arrived` legitimately
        // un-fired — that is correct Motion behaviour, not a missed arrival.
        for i in 1..=steps as u64 {
            let s = m.tick(i * 6_000, 10_000, 10_000);
            if s.arrived { arrivals += 1; }
        }
        prop_assert_eq!(arrivals, 1);
    }
}
