use super::support::*;
use crate::index::IndexedLog;
use crate::map::SignalMap;
use crate::scan::hex_payload;
use crate::session::Session;

#[test]
fn decodes_known_powertrain_frame() {
    // rpm 800, speed 50 km/h, coolant 80 C, throttle 40%
    let text = "SLOGv1\nF 0 1A0 800C881378640000\nF 1000 1A0 800C881378640000\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&tiny_map())).unwrap();
    let values = log.values_at(0).unwrap();
    let get = |name: &str| values.iter().find(|v| v.name == name).unwrap().value;
    assert!((get("EngineRPM") - 800.0).abs() < 1e-6);
    assert!((get("VehicleSpeed") - 50.0).abs() < 1e-6);
    assert!((get("CoolantTemp") - 80.0).abs() < 1e-6);
    assert!((get("Throttle") - 40.0).abs() < 1e-6);
}

#[test]
fn big_endian_signal_through_the_map() {
    let map = SignalMap::parse(
        r#"{"name":"be","version":1,"messages":[{"id":"0x10","name":"M","signals":[
            {"name":"Word","startBit":7,"bitLength":16,"endian":"big","factor":1}
        ]}]}"#,
    )
    .unwrap();
    let text = "F 0 10 1234000000000000\nF 1 10 1234000000000000\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&map)).unwrap();
    let value = log.values_at(0).unwrap()[0].value;
    assert!((value - f64::from(0x1234)).abs() < 1e-6);
}

#[test]
fn dbc_text_decodes_the_open_log() {
    let text = "SLOGv1\nF 0 1A0 800C881378640000\nF 10000 1A0 800C881378640000\n";
    let mut session = Session::new();
    session
        .open_bytes("tiny.slog", text.as_bytes().to_vec())
        .unwrap();
    let summary = session
        .open_map_json(
            r#"
VERSION ""
BO_ 416 ECM_Engine: 8 ECM
 SG_ EngineRPM : 0|16@1+ (0.25,0) [0|16383.75] "rpm" TCU
BA_ "GenMsgCycleTime" BO_ 416 10;
"#,
        )
        .unwrap();
    assert!(summary
        .map_label
        .as_deref()
        .unwrap_or("")
        .starts_with("DBC import"));
    let rpm = session
        .values_at(0)
        .unwrap()
        .into_iter()
        .find(|value| value.name == "EngineRPM")
        .unwrap();
    assert!((rpm.value - 800.0).abs() < 1e-6, "{}", rpm.value);
}

#[test]
fn channel_filter_ignores_the_other_bus() {
    let map = SignalMap::parse(
        r#"{"name":"ch","version":1,"messages":[{"id":"0x1A0","name":"M","channel":1,"signals":[
            {"name":"Byte","startBit":0,"bitLength":8,"factor":1}
        ]}]}"#,
    )
    .unwrap();
    let text = "\
base hex timestamps absolute
0.000000 1 1A0 Rx d 1 0A
0.001000 2 1A0 Rx d 1 14
";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&map)).unwrap();
    let value = log.values_at(1_000).unwrap()[0].value;
    assert!((value - 10.0).abs() < 1e-6, "{value}");
}

#[test]
fn value_tables_and_mux_ids_match_raw_values() {
    let dbc = r#"
BO_ 200 Scaled: 8 ECM
 SG_ Sel M : 0|8@1+ (0.5,10) [0|255] "" Vector__XXX
 SG_ Mode : 8|8@1+ (2,-4) [0|255] "" Vector__XXX
 SG_ Branch m3 : 16|8@1+ (1,0) [0|255] "" Vector__XXX
VAL_ 200 Mode 3 "Sport" 2 "Comfort" ;
"#;
    let map = crate::dbc::parse(dbc).unwrap();
    // Sel raw 3 (11.5 scaled), Mode raw 3 (2 scaled), Branch 42.
    let log = IndexedLog::open_bytes(b"F 0 C8 03032A0000000000\n".to_vec(), Some(&map)).unwrap();
    let values = log.values_at(0).unwrap();
    let get = |name: &str| values.iter().find(|value| value.name == name);
    let mode = get("Mode").expect("Mode");
    assert_eq!(mode.value, 2.0);
    assert_eq!(mode.label.as_deref(), Some("Sport"));
    assert_eq!(get("Branch").map(|value| value.value), Some(42.0));
}

#[test]
fn short_frames_leave_their_missing_signals_unset() {
    let dbc = r#"
BO_ 300 Short: 8 ECM
 SG_ Head : 0|8@1+ (1,0) [0|255] "" Vector__XXX
 SG_ TailLe : 48|16@1+ (1,0) [0|65535] "" Vector__XXX
 SG_ TailBe : 55|16@0+ (1,0) [0|65535] "" Vector__XXX
BO_ 400 Wide: 8 ECM
 SG_ Odo : 0|64@1+ (1,0) [0|0] "" Vector__XXX
"#;
    let map = crate::dbc::parse(dbc).unwrap();
    let text = "F 0 12C 07\nF 1000 190 FFFFFFFFFFFFFFFF\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&map)).unwrap();
    let values = log.values_at(1000).unwrap();
    let get = |name: &str| {
        values
            .iter()
            .find(|value| value.name == name)
            .map(|value| value.value)
    };
    assert_eq!(get("Head"), Some(7.0));
    assert_eq!(
        get("TailLe"),
        None,
        "a 1-byte frame does not carry bytes 6-7"
    );
    assert_eq!(
        get("TailBe"),
        None,
        "a 1-byte frame does not carry bytes 6-7"
    );
    assert_eq!(
        get("Odo"),
        Some(u64::MAX as f64),
        "unsigned 64-bit stays positive"
    );
}

#[test]
fn multiplex_value_table_and_a_second_dbc() {
    let dbc = r#"
BO_ 100 MuxMsg: 8 ECM
 SG_ Mode M : 0|8@1+ (1,0) [0|3] "" Vector__XXX
 SG_ Gear m0 : 8|8@1+ (1,0) [0|7] "" Vector__XXX
 SG_ Temp m1 : 8|8@1- (1,-40) [0|200] "C" Vector__XXX
VAL_ 100 Gear 0 "N" 1 "D" 2 "R" ;
"#;
    let map = crate::dbc::parse(dbc).unwrap();
    let text = "F 0 64 0001000000000000\nF 1000 64 0105000000000000\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&map)).unwrap();
    let at0 = log.values_at(0).unwrap();
    let gear = at0.iter().find(|value| value.name == "Gear").unwrap();
    assert!((gear.value - 1.0).abs() < 1e-6);
    assert_eq!(gear.label.as_deref(), Some("D"));
    assert!(at0.iter().all(|value| value.name != "Temp"));
    let at1 = log.values_at(1_000).unwrap();
    let temp = at1.iter().find(|value| value.name == "Temp").unwrap();
    assert!((temp.value - (-35.0)).abs() < 1e-6);
    let gear_held = at1.iter().find(|value| value.name == "Gear").unwrap();
    assert!((gear_held.value - 1.0).abs() < 1e-6);

    let mut session = Session::new();
    session
        .open_bytes("mux.slog", text.as_bytes().to_vec())
        .unwrap();
    session
        .open_map_json(
            r#"{"name":"left","version":1,"messages":[{"id":100,"name":"Mux","signals":[
        {"name":"Mode","startBit":0,"bitLength":8,"muxSwitch":true}
    ]}]}"#,
        )
        .unwrap();
    let summary = session.add_map_json(
        r#"{"name":"right","version":1,"messages":[{"id":100,"name":"Mux","channel":2,"signals":[
            {"name":"Mode","startBit":0,"bitLength":8,"muxSwitch":true}
        ]}]}"#,
        2,
    )
    .unwrap();
    let names: Vec<_> = summary
        .signals
        .iter()
        .map(|signal| signal.name.as_str())
        .collect();
    assert!(names.contains(&"Mode"), "{names:?}");
    assert!(
        names.iter().any(|name| name.starts_with("Mode@")),
        "{names:?}"
    );
}

#[test]
fn checksum_crcs_match_their_published_check_values() {
    use crate::index::ChecksumAlgo;
    assert_eq!(ChecksumAlgo::CrcJ1850.compute(b"123456789"), 0x4B);
    assert_eq!(ChecksumAlgo::Crc8H2F.compute(b"123456789"), 0xDF);
}

/// An SWM-style message: CRC-8/SAE-J1850 in byte 0 over bytes 1..DLC-1, a
/// 4-bit counter in the low nibble of byte 1, DLC 5, 100 ms apart.
fn swm_style_log(checksum_of: impl Fn(usize, &[u8]) -> u8) -> String {
    let mut text = String::new();
    for i in 0..20usize {
        let mut frame = [0u8, ((i + 1) % 16) as u8 | 0x30, 0x12, i as u8, 0x7F];
        frame[0] = checksum_of(i, &frame[1..]);
        text.push_str(&format!(
            "F {} 334 {}\n",
            i * 100_000,
            hex_payload(&frame, 5)
        ));
    }
    text
}

const SWM_DBC: &str = r#"
BO_ 820 ZcuLeds: 5 ZCU
 SG_ Checksum : 0|8@1+ (1,0) [0|255] "" SWM
 SG_ Counter : 8|4@1+ (1,0) [0|15] "" SWM
 SG_ Led : 12|4@1+ (1,0) [0|15] "" SWM
"#;

#[test]
fn a_crc_checksum_is_recognised_and_a_bad_frame_is_seen_as_the_ecu_sees_it() {
    use crate::index::ChecksumAlgo;
    let map = crate::dbc::parse(SWM_DBC).unwrap();
    // Frame 10 is corrupted in its checksum byte only.
    let text = swm_style_log(|i, covered| {
        let crc = ChecksumAlgo::CrcJ1850.compute(covered);
        if i == 10 {
            crc ^ 0x55
        } else {
            crc
        }
    });
    let log = IndexedLog::open_bytes(text.into_bytes(), Some(&map)).unwrap();
    let mut events: Vec<(u64, String)> = log.events().to_vec();
    events.sort();
    // The rejected frame leaves the counter reference at frame 9, so frame 11
    // arrives as a jump, as the receiving ECU sees it.
    assert_eq!(
        events,
        vec![
            (1_000_000, "Checksum Checksum".to_string()),
            (1_100_000, "Counter Counter".to_string()),
        ]
    );
}

#[test]
fn an_unknown_checksum_scheme_is_noted_not_flagged() {
    let map = crate::dbc::parse(SWM_DBC).unwrap();
    let text = swm_style_log(|i, _| (i as u8).wrapping_mul(37).wrapping_add(11));
    let log = IndexedLog::open_bytes(text.into_bytes(), Some(&map)).unwrap();
    assert!(log.events().is_empty(), "{:?}", log.events());
    assert!(
        log.warnings()
            .iter()
            .any(|warning| warning.contains("not checked")),
        "{:?}",
        log.warnings()
    );
}

#[test]
fn repeated_dbc_signal_names_are_renamed_and_still_decode() {
    let text = r#"
BO_ 1 Brake: 8 ESC
 SG_ Pressure : 0|8@1+ (1,0) [0|255] "bar" IC
 SG_ Checksum : 56|8@1+ (1,0) [0|255] "" IC
BO_ 2 Steer: 8 SWM
 SG_ Angle : 0|8@1+ (1,0) [0|255] "deg" IC
 SG_ Checksum : 56|8@1+ (1,0) [0|255] "" IC
VAL_ 2 Checksum 171 "Fixed" ;
"#;
    let map = crate::dbc::parse(text).expect("a repeated name must not reject the DBC");
    let names: Vec<&str> = map
        .signals
        .iter()
        .map(|signal| signal.name.as_str())
        .collect();
    assert_eq!(names, vec!["Pressure", "Checksum", "Angle", "Checksum@2"]);
    assert!(map
        .warnings
        .iter()
        .any(|warning| warning.contains("Checksum@2")));

    let log = "F 0 1 11000000000000AA\nF 0 2 22000000000000AB\n";
    let log = IndexedLog::open_bytes(log.as_bytes().to_vec(), Some(&map)).unwrap();
    let values = log.values_at(0).unwrap();
    let get = |name: &str| values.iter().find(|value| value.name == name).unwrap();
    assert_eq!(get("Checksum").value, 0xAA as f64);
    assert_eq!(get("Checksum@2").value, 0xAB as f64);
    assert_eq!(get("Checksum@2").label.as_deref(), Some("Fixed"));
}

#[test]
fn a_bad_layout_skips_one_signal_and_can_fd_signals_decode_past_byte_8() {
    let text = r#"
BO_ 1 Fd: 64 ADAS
 SG_ Head : 0|8@1+ (1,0) [0|255] "" IC
 SG_ LateLe : 400|16@1+ (0.5,0) [0|1000] "" IC
 SG_ LateBe : 407|16@0+ (1,0) [0|65535] "" IC
 SG_ PastTheEnd : 600|16@1+ (1,0) [0|65535] "" IC
"#;
    let map = crate::dbc::parse(text).expect("one bad layout must not reject the DBC");
    let names: Vec<&str> = map
        .signals
        .iter()
        .map(|signal| signal.name.as_str())
        .collect();
    assert_eq!(names, vec!["Head", "LateLe", "LateBe"]);
    assert!(map
        .warnings
        .iter()
        .any(|warning| warning.contains("PastTheEnd")));

    // Byte 50-51 little-endian 0x0123 (291 × 0.5), byte 50-51 big-endian 0x2301.
    let mut payload = [0u8; 64];
    payload[0] = 7;
    payload[50] = 0x23;
    payload[51] = 0x01;
    let line = format!("F 0 1 {}\n", hex_payload(&payload, 64));
    let log = IndexedLog::open_bytes(line.into_bytes(), Some(&map)).unwrap();
    let values = log.values_at(0).unwrap();
    let get = |name: &str| {
        values
            .iter()
            .find(|value| value.name == name)
            .map(|value| value.value)
    };
    assert_eq!(get("Head"), Some(7.0));
    assert_eq!(get("LateLe"), Some(145.5));
    assert_eq!(get("LateBe"), Some(0x2301 as f64));
}

#[test]
fn a_broken_signal_does_not_drop_the_rest_of_the_dbc() {
    let text = "\
BO_ 1 Only: 8 ECM
 SG_ Broken : not-a-layout
 SG_ Ok : 0|8@1+ (1,0) [0|255] \"\" Vector__XXX
";
    let map = crate::dbc::parse(text).unwrap();
    assert_eq!(map.signals.len(), 1);
    assert!(map
        .warnings
        .iter()
        .any(|warning| warning.contains("Broken")));
}
