use heapless::Vec;
use somfy_api::ShadeDto;
use somfy_domain::{PlannedTx, Pos, Shade, ShadeCommand, ShadeConfig, ShadeId};

fn shade() -> Shade {
    let mut s = Shade::new(ShadeConfig::new("Kitchen", 0x123456).unwrap());
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(
        ShadeCommand::SetMy(Some(Pos::from_percent(30))),
        0,
        &mut out,
    );
    s.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);
    s.tick(2_000, &mut out); // mid-travel: pos 20%, target 50%, moving down
    s
}

#[test]
fn shade_dto_snapshots_live_state() {
    let dto = ShadeDto::from_shade(ShadeId(3), &shade());
    assert_eq!(dto.id, 3);
    assert_eq!(dto.name.as_str(), "Kitchen");
    assert_eq!(dto.address, 0x123456);
    assert_eq!(dto.position, 20);
    assert_eq!(dto.target, 50);
    assert_eq!(dto.my_position, Some(30));
    assert_eq!(dto.direction, 1); // C++ sign: down = +1
    assert_eq!(dto.up_time_ms, 10_000);
}

#[test]
fn shade_dto_serializes_to_stable_json() {
    let dto = ShadeDto::from_shade(ShadeId(0), &shade());
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["name"], "Kitchen");
    assert_eq!(json["position"], 20);
    assert_eq!(json["target"], 50);
    assert_eq!(json["myPosition"], 30);
    assert_eq!(json["direction"], 1);
    // discriminants, not strings:
    assert_eq!(json["kind"], 0); // Roller = 0x00
    assert_eq!(json["tiltMode"], 0); // None = 0x00
}

#[test]
fn shade_dto_roundtrips() {
    let dto = ShadeDto::from_shade(ShadeId(0), &shade());
    let json = serde_json::to_string(&dto).unwrap();
    let back: ShadeDto = serde_json::from_str(&json).unwrap();
    assert_eq!(back, dto);
}

#[test]
fn optional_my_position_serializes_as_null_when_unset() {
    let s = Shade::new(ShadeConfig::new("Bare", 0x42).unwrap());
    let dto = ShadeDto::from_shade(ShadeId(1), &s);
    let json = serde_json::to_value(&dto).unwrap();
    assert!(json["myPosition"].is_null());
}
