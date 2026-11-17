//! `(message name, signal name)` resolves to the DBC `BO_` / `SG_` names.
//! A display name of `Name@<id>` is not the key.

use crate::error::ErrorKind;

#[test]
fn a_signal_is_found_by_its_message_and_dbc_name() {
    let map = crate::dbc::parse(
        r#"
BO_ 416 ECM_Engine: 8 ECM
 SG_ EngineRPM : 0|16@1+ (0.25,0) [0|16383.75] "rpm" TCU
"#,
    )
    .unwrap();
    let rpm = map.lookup("ECM_Engine", "EngineRPM").unwrap();
    assert_eq!(rpm.message_name, "ECM_Engine");
    assert_eq!(rpm.message_id, 416);
    assert_eq!(rpm.name, "EngineRPM");
    assert_eq!(rpm.unit, "rpm");
    assert!((rpm.spec.factor - 0.25).abs() < 1e-12);
}

#[test]
fn the_same_signal_name_in_two_messages_resolves_by_the_dbc_name() {
    let map = crate::dbc::parse(
        r#"
BO_ 1 Brake: 8 ESC
 SG_ Pressure : 0|8@1+ (1,0) [0|255] "bar" IC
 SG_ Checksum : 56|8@1+ (1,0) [0|255] "" IC
BO_ 416 Steer: 8 SWM
 SG_ Angle : 0|8@1+ (1,0) [0|255] "deg" IC
 SG_ Checksum : 56|8@1+ (1,0) [0|255] "" IC
"#,
    )
    .unwrap();
    let brake = map.lookup("Brake", "Checksum").unwrap();
    assert_eq!(brake.message_name, "Brake");
    assert_eq!(brake.message_id, 1);
    assert_eq!(brake.name, "Checksum");

    let steer = map.lookup("Steer", "Checksum").unwrap();
    assert_eq!(steer.message_name, "Steer");
    assert_eq!(steer.message_id, 416);
    // The second copy is shown as `Checksum@1A0`. The key is still `Checksum`.
    assert_eq!(steer.name, "Checksum@1A0");
    assert_eq!(map.lookup("Brake", "Pressure").unwrap().message_id, 1);
    assert_eq!(map.lookup("Steer", "Angle").unwrap().name, "Angle");

    // The display form is not a DBC name, so it is not a key.
    let err = map.lookup("Steer", "Checksum@1A0").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("Steer"), "{text}");
    assert!(text.contains("Checksum@1A0"), "{text}");
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[test]
fn a_multiplexed_signal_is_found_by_its_dbc_name() {
    let map = crate::dbc::parse(
        r#"
BO_ 100 MuxMsg: 8 ECM
 SG_ Mode M : 0|8@1+ (1,0) [0|3] "" Vector__XXX
 SG_ Gear m0 : 8|8@1+ (1,0) [0|7] "" Vector__XXX
 SG_ Temp m1 : 8|8@1- (1,-40) [0|200] "C" Vector__XXX
"#,
    )
    .unwrap();
    let mode = map.lookup("MuxMsg", "Mode").unwrap();
    assert!(mode.mux_switch);
    assert_eq!(mode.mux_value, None);
    assert_eq!(mode.message_id, 100);

    let gear = map.lookup("MuxMsg", "Gear").unwrap();
    assert!(!gear.mux_switch);
    assert_eq!(gear.mux_value, Some(0));
    assert_eq!(gear.name, "Gear");

    let temp = map.lookup("MuxMsg", "Temp").unwrap();
    assert_eq!(temp.mux_value, Some(1));
    assert_eq!(temp.unit, "C");
}

#[test]
fn two_signals_with_one_dbc_name_in_one_message_are_not_one_signal() {
    let map = crate::dbc::parse(
        r#"
BO_ 100 MuxMsg: 8 ECM
 SG_ Value m0 : 8|8@1+ (1,0) [0|255] "" Vector__XXX
 SG_ Value m1 : 16|8@1+ (1,0) [0|255] "" Vector__XXX
"#,
    )
    .unwrap();
    let err = map.lookup("MuxMsg", "Value").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Invalid);
    let text = err.to_string();
    assert!(text.contains("MuxMsg"), "{text}");
    assert!(text.contains("Value"), "{text}");
}

#[test]
fn an_unknown_message_and_signal_is_an_error_naming_the_pair() {
    let map = crate::dbc::parse(
        r#"
BO_ 416 ECM_Engine: 8 ECM
 SG_ EngineRPM : 0|16@1+ (0.25,0) [0|16383.75] "rpm" TCU
BO_ 256 Other: 8 ECM
 SG_ Status : 0|8@1+ (1,0) [0|255] "" ECM
"#,
    )
    .unwrap();

    let missing = map.lookup("ECM_Engine", "NotThere").unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::NotFound);
    let text = missing.to_string();
    assert!(text.contains("ECM_Engine"), "{text}");
    assert!(text.contains("NotThere"), "{text}");

    // `EngineRPM` exists, but not on `Other`. The pair is what is named.
    let wrong_message = map.lookup("Other", "EngineRPM").unwrap_err();
    assert_eq!(wrong_message.kind(), ErrorKind::NotFound);
    let text = wrong_message.to_string();
    assert!(text.contains("Other"), "{text}");
    assert!(text.contains("EngineRPM"), "{text}");
}
