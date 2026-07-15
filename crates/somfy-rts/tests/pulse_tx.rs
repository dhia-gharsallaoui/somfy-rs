use heapless::Vec;
use somfy_rts::{encode56, render_pulses, Command, Frame, FrameKind, Pulse, TIMINGS};

fn bytes() -> [u8; 7] {
    encode56(&Frame {
        key: 0xA7,
        command: Command::Up,
        rolling_code: 7,
        address: 0xAABBCC,
    })
}

#[test]
fn first_frame_starts_with_wakeup_then_two_hw_syncs() {
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::First, &mut out);
    assert!(out[0].high && out[0].micros == TIMINGS::WAKEUP_HIGH);
    assert!(!out[1].high && out[1].micros == TIMINGS::WAKEUP_LOW);
    // 2 hardware syncs = 4 half-pulses of HW_SYNC_HALF
    for p in &out[2..6] {
        assert_eq!(p.micros, TIMINGS::HW_SYNC_HALF);
    }
    assert_eq!(out[6].micros, TIMINGS::SW_SYNC_HIGH);
}

#[test]
fn repeat_frame_has_no_wakeup_and_seven_hw_syncs() {
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::Repeat, &mut out);
    for p in &out[0..14] {
        assert_eq!(p.micros, TIMINGS::HW_SYNC_HALF);
    }
    assert_eq!(out[14].micros, TIMINGS::SW_SYNC_HIGH);
}

#[test]
fn data_section_is_manchester_with_constant_energy() {
    // 56 data bits -> exactly 112 half-symbols of HALF_SYMBOL µs each,
    // adjacent same-level halves merged is NOT done at this layer.
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::Repeat, &mut out);
    let data: Vec<&Pulse, 320> = out
        .iter()
        .filter(|p| p.micros == TIMINGS::HALF_SYMBOL)
        .collect();
    // 112 half symbols + the SW-sync trailing HALF_SYMBOL low half
    assert_eq!(data.len(), 113);
    let highs = data.iter().filter(|p| p.high).count();
    assert_eq!(highs, 56); // Manchester: every bit contributes one high half
}

#[test]
fn frame_ends_with_inter_frame_gap() {
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::Repeat, &mut out);
    let last = out.last().unwrap();
    assert!(!last.high && last.micros == TIMINGS::INTER_FRAME_GAP);
}
