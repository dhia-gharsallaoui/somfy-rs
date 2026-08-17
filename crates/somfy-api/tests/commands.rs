use somfy_api::CommandDto;
use somfy_domain::{Pos, ShadeCommand};

#[test]
fn actions_parse_from_json() {
    let cases = [
        (r#"{"action":"up"}"#, ShadeCommand::Up),
        (r#"{"action":"down"}"#, ShadeCommand::Down),
        (r#"{"action":"my"}"#, ShadeCommand::My),
        (r#"{"action":"stepUp"}"#, ShadeCommand::StepUp),
        (r#"{"action":"stepDown"}"#, ShadeCommand::StepDown),
        (
            r#"{"action":"goTo","position":42}"#,
            ShadeCommand::GoTo(Pos::from_percent(42)),
        ),
        (
            r#"{"action":"setMy","position":30}"#,
            ShadeCommand::SetMy(Some(Pos::from_percent(30))),
        ),
        (
            r#"{"action":"setMy","position":null}"#,
            ShadeCommand::SetMy(None),
        ),
        (r#"{"action":"vent"}"#, ShadeCommand::Vent),
    ];
    for (json, expected) in cases {
        let dto: CommandDto = serde_json::from_str(json).unwrap();
        assert_eq!(dto.to_domain(), expected, "for {json}");
    }
}

#[test]
fn unknown_action_is_rejected() {
    assert!(serde_json::from_str::<CommandDto>(r#"{"action":"explode"}"#).is_err());
}

#[test]
fn goto_without_position_is_rejected() {
    // `goTo` MUST carry a target; a missing `position` is a malformed request,
    // not a silent default (the manual deserializer errors on missing_field).
    assert!(serde_json::from_str::<CommandDto>(r#"{"action":"goTo"}"#).is_err());
}

/// A vent names no position, and a body offering one is ignored rather than
/// honoured.
///
/// What it aims at is the shade's own measured slat-separation band — the whole
/// reason it needs no position estimate — so a caller-supplied target would be a
/// second answer to a question this command deliberately has only one answer to.
#[test]
fn vent_carries_no_position_and_ignores_one_that_is_offered() {
    let bare: CommandDto = serde_json::from_str(r#"{"action":"vent"}"#).unwrap();
    let with_position: CommandDto =
        serde_json::from_str(r#"{"action":"vent","position":42}"#).unwrap();
    assert_eq!(bare, with_position);
    assert_eq!(bare.to_domain(), ShadeCommand::Vent);
}

#[test]
fn goto_position_over_100_clamps_via_pos() {
    let dto: CommandDto = serde_json::from_str(r#"{"action":"goTo","position":250}"#).unwrap();
    assert_eq!(dto.to_domain(), somfy_domain::ShadeCommand::GoTo(Pos::FULL));
}
