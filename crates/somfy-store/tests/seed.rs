//! Seeding a rolling code for a newly provisioned address, from **outside**
//! the crate — the same reason `tests/ordering.rs` lives here.
//!
//! The property under test is the one that breaks a real motor pairing when it
//! is wrong: a shade record is read at *every* boot, so if seeding it wrote the
//! record's starting code every time, the counter would walk backwards on every
//! restart and the motor would reject everything until it was re-paired by
//! hand. `boot` below is a whole boot's worth of that sequence, run twice over
//! the same persistent cells.

use core::cell::RefCell;
use somfy_rts::{Command, RollingCode};
use somfy_store::{
    seed_if_absent, transmit, FrameBits, RegionState, RollingCodeStore, Seeded, TransmitPlan,
    TransmitQueue, TransmitTicket,
};

const ADDRESS: u32 = 0x00_C0DE;

/// The code a provisioning record carries. Deliberately low, so that a second
/// seeding of it after three transmissions would be visible as a counter
/// walking backwards rather than as a value that happens to look plausible.
const SEED: RollingCode = RollingCode(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    Load { address: u32 },
    Commit { address: u32, code: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoreFailed;

/// The bytes that survive a reboot. A store is built over one of these and
/// dropped, exactly as `FlashStore` is mounted at each boot and handed to a
/// task; the cell is the flash.
#[derive(Debug, Default)]
struct Flash {
    stored: Option<RollingCode>,
}

/// A store view over persistent `flash`, recording every call it makes.
struct MockStore<'a> {
    flash: &'a RefCell<Flash>,
    log: &'a RefCell<Vec<Event>>,
    load_fails: bool,
    commit_fails: bool,
}

impl<'a> MockStore<'a> {
    fn mount(flash: &'a RefCell<Flash>, log: &'a RefCell<Vec<Event>>) -> Self {
        MockStore {
            flash,
            log,
            load_fails: false,
            commit_fails: false,
        }
    }
}

impl RollingCodeStore for MockStore<'_> {
    type Error = StoreFailed;

    fn load(&mut self, address: u32) -> Result<Option<RollingCode>, StoreFailed> {
        self.log.borrow_mut().push(Event::Load { address });
        if self.load_fails {
            return Err(StoreFailed);
        }
        Ok(self.flash.borrow().stored)
    }

    fn commit(&mut self, address: u32, code: RollingCode) -> Result<(), StoreFailed> {
        self.log.borrow_mut().push(Event::Commit {
            address,
            code: code.0,
        });
        if self.commit_fails {
            return Err(StoreFailed);
        }
        self.flash.borrow_mut().stored = Some(code);
        Ok(())
    }
}

/// A queue that keeps the codes it was handed, so a test can assert on what
/// went on the air rather than only on what was stored.
#[derive(Default)]
struct MockQueue {
    sent: Vec<u16>,
}

impl TransmitQueue for MockQueue {
    type Error = ();

    fn enqueue(&mut self, ticket: TransmitTicket) -> Result<(), ()> {
        self.sent.push(ticket.request().frame.rolling_code);
        Ok(())
    }
}

fn plan() -> TransmitPlan {
    TransmitPlan {
        address: ADDRESS,
        command: Command::Up,
        bits: FrameBits::Bits56,
        repeats: 2,
    }
}

fn commits(log: &RefCell<Vec<Event>>) -> Vec<u16> {
    log.borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Commit { code, .. } => Some(*code),
            Event::Load { .. } => None,
        })
        .collect()
}

#[test]
fn an_address_with_no_stored_code_is_seeded() {
    let flash = RefCell::new(Flash::default());
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::mount(&flash, &log);

    let seeded = seed_if_absent(&mut store, ADDRESS, SEED, RegionState::Intact);

    assert_eq!(seeded, Ok(Seeded::Planted(SEED)));
    assert_eq!(flash.borrow().stored, Some(SEED));
    assert_eq!(
        log.into_inner(),
        vec![
            Event::Load { address: ADDRESS },
            Event::Commit {
                address: ADDRESS,
                code: SEED.0,
            },
        ]
    );
}

#[test]
fn an_address_that_already_has_a_code_is_never_written() {
    let flash = RefCell::new(Flash {
        stored: Some(RollingCode(9)),
    });
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::mount(&flash, &log);

    let seeded = seed_if_absent(&mut store, ADDRESS, SEED, RegionState::Intact);

    assert_eq!(seeded, Ok(Seeded::Kept(RollingCode(9))));
    assert_eq!(flash.borrow().stored, Some(RollingCode(9)));
    // The claim that matters is an absence: no write was even attempted.
    assert_eq!(commits(&log), Vec::<u16>::new());
}

/// The headline property. One boot seeds and transmits three times; the next
/// boot reads the same shade record, seeds again, and must change nothing.
#[test]
fn a_reboot_cannot_walk_a_rolling_code_backwards() {
    let flash = RefCell::new(Flash::default());
    let log = RefCell::new(Vec::new());

    /// One boot: mount a store over the persistent cells, apply the shade
    /// record's seed, then send `presses` commands.
    fn boot(
        flash: &RefCell<Flash>,
        log: &RefCell<Vec<Event>>,
        presses: usize,
    ) -> (Seeded, Vec<u16>) {
        let mut store = MockStore::mount(flash, log);
        let seeded = seed_if_absent(&mut store, ADDRESS, SEED, RegionState::Intact).expect("seed");
        let mut queue = MockQueue::default();
        for _ in 0..presses {
            transmit(&mut store, &mut queue, plan()).expect("transmit");
        }
        (seeded, queue.sent)
    }

    let (first, sent) = boot(&flash, &log, 3);
    assert_eq!(first, Seeded::Planted(SEED));
    assert_eq!(sent, vec![5, 6, 7]);
    assert_eq!(flash.borrow().stored, Some(RollingCode(8)));

    // Reboot. The shade record still says 5, and the store must ignore it.
    let (second, sent) = boot(&flash, &log, 1);
    assert_eq!(second, Seeded::Kept(RollingCode(8)));
    assert_eq!(
        sent,
        vec![8],
        "the frame after a reboot must carry the code that follows the last one \
         sent, not the record's seed"
    );
    assert_eq!(flash.borrow().stored, Some(RollingCode(9)));

    // And no commit anywhere in either boot went backwards. The first is the
    // seed itself; the second boot contributes only the transmission's, because
    // its seeding wrote nothing.
    let committed = commits(&log);
    assert_eq!(committed, vec![5, 6, 7, 8, 9]);
    assert!(
        committed.windows(2).all(|pair| pair[1] > pair[0]),
        "every committed code must be greater than the one before it: {committed:?}"
    );
}

/// Seeding the same record over and over — a device rebooting in a loop —
/// leaves the counter exactly where the transmissions left it.
#[test]
fn repeated_seeding_is_idempotent() {
    let flash = RefCell::new(Flash {
        stored: Some(RollingCode(400)),
    });
    let log = RefCell::new(Vec::new());

    for _ in 0..50 {
        let mut store = MockStore::mount(&flash, &log);
        assert_eq!(
            seed_if_absent(&mut store, ADDRESS, SEED, RegionState::Intact),
            Ok(Seeded::Kept(RollingCode(400))),
        );
    }

    assert_eq!(flash.borrow().stored, Some(RollingCode(400)));
    assert_eq!(commits(&log), Vec::<u16>::new());
}

/// A region that holds unreadable bytes is not a blank one. An empty read
/// there may be lost data rather than a new address, and planting a low code
/// over a lost high one is the same failure as re-seeding at every boot.
#[test]
fn a_damaged_region_is_not_seeded() {
    let flash = RefCell::new(Flash::default());
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::mount(&flash, &log);

    let seeded = seed_if_absent(&mut store, ADDRESS, SEED, RegionState::Damaged { slots: 2 });

    assert_eq!(seeded, Ok(Seeded::Refused { damaged: 2 }));
    assert_eq!(flash.borrow().stored, None);
    assert_eq!(commits(&log), Vec::<u16>::new());
}

/// Damage elsewhere in the region does not disturb an address whose code is
/// readable: that read answered, so there is nothing to decide and nothing to
/// write.
#[test]
fn a_damaged_region_still_keeps_a_readable_code() {
    let flash = RefCell::new(Flash {
        stored: Some(RollingCode(77)),
    });
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::mount(&flash, &log);

    let seeded = seed_if_absent(&mut store, ADDRESS, SEED, RegionState::Damaged { slots: 1 });

    assert_eq!(seeded, Ok(Seeded::Kept(RollingCode(77))));
    assert_eq!(commits(&log), Vec::<u16>::new());
}

#[test]
fn a_failed_read_seeds_nothing_and_is_reported() {
    let flash = RefCell::new(Flash::default());
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::mount(&flash, &log);
    store.load_fails = true;

    let seeded = seed_if_absent(&mut store, ADDRESS, SEED, RegionState::Intact);

    assert_eq!(seeded, Err(StoreFailed));
    assert_eq!(flash.borrow().stored, None);
    assert_eq!(commits(&log), Vec::<u16>::new());
}

#[test]
fn a_failed_write_is_reported_rather_than_reported_as_seeded() {
    let flash = RefCell::new(Flash::default());
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::mount(&flash, &log);
    store.commit_fails = true;

    let seeded = seed_if_absent(&mut store, ADDRESS, SEED, RegionState::Intact);

    assert_eq!(seeded, Err(StoreFailed));
    assert_eq!(flash.borrow().stored, None);
}

/// `RegionState::from_damaged` is what a caller builds from a store survey, so
/// the zero case has to be the intact one.
#[test]
fn a_survey_with_no_damage_is_an_intact_region() {
    assert_eq!(RegionState::from_damaged(0), RegionState::Intact);
    assert_eq!(
        RegionState::from_damaged(3),
        RegionState::Damaged { slots: 3 }
    );
}
