use somfy_api::{ShadeStateEvent, WsEvent};
use somfy_domain::{Direction, Pos, ShadeId, StateDelta};

#[test]
fn shade_state_event_from_delta_and_json_shape() {
    let delta = StateDelta {
        id: ShadeId(2),
        pos: Pos::from_percent(40),
        tilt_pos: Pos::ZERO,
        direction: Direction::Down,
    };
    let ev = WsEvent::ShadeState(ShadeStateEvent::from(&delta));
    let json = serde_json::to_value(&ev).unwrap();
    assert_eq!(json["ev"], "shadeState");
    assert_eq!(json["id"], 2);
    assert_eq!(json["position"], 40);
    assert_eq!(json["direction"], 1);
}

#[test]
fn ws_event_roundtrips() {
    let ev = WsEvent::ShadeState(ShadeStateEvent {
        id: 1,
        position: 5,
        tilt_position: 0,
        direction: -1,
    });
    let json = serde_json::to_string(&ev).unwrap();
    let back: WsEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ev);
}
