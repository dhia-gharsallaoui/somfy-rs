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
fn goto_position_over_100_clamps_via_pos() {
    let dto: CommandDto = serde_json::from_str(r#"{"action":"goTo","position":250}"#).unwrap();
    assert_eq!(dto.to_domain(), somfy_domain::ShadeCommand::GoTo(Pos::FULL));
}
