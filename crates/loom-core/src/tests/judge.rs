//! One refusal test per §3.3 rule, a request that lists three problems at once,
//! and the channel rule: same DBC on two channels.

use crate::error::ErrorKind;
use crate::judge::{self, Request, Value};
use crate::map::SignalMap;

const CLUSTER: &str = r#"
BO_ 100 Ignition: 8 BCM
 SG_ IgnState : 0|8@1+ (1,0) [0|3] "" BCM
BO_ 256 BrakeCmd: 8 ESC
 SG_ BrakePedal : 0|8@1+ (1,0) [0|1] "" ESC
BO_ 512 Lamps: 8 BCM
 SG_ BrakeLamp : 0|1@1+ (1,0) [0|1] "" BCM
VAL_ 100 IgnState 0 "OFF" 1 "ON" ;
VAL_ 512 BrakeLamp 0 "OFF" 1 "ON" ;
"#;

fn cluster() -> SignalMap {
    crate::dbc::parse(CLUSTER).unwrap()
}

fn problems(json: &str, dbc: &SignalMap) -> Vec<String> {
    match judge::check(json, dbc) {
        Ok(_) => panic!("expected a refusal, got an accepted request"),
        Err(refusal) => refusal.problems,
    }
}

fn case(case_id: &str, checks: &str) -> String {
    format!(r#"{{"case_id":"{case_id}","verifies":["REQ"],"kind":"positive","checks":[{checks}]}}"#)
}

fn request(cases: &str) -> String {
    format!(r#"{{"format":"loom-judge/1","spec_id":"s","cases":[{cases}]}}"#)
}

fn cond(message: &str, signal: &str, op: &str, value: &str) -> String {
    format!(
        r#"{{"signal":{{"message":"{message}","signal":"{signal}"}},"op":"{op}","value":{value}}}"#
    )
}

fn cond_on(message: &str, signal: &str, channel: i64, op: &str, value: &str) -> String {
    format!(
        r#"{{"signal":{{"message":"{message}","signal":"{signal}","channel":{channel}}},"op":"{op}","value":{value}}}"#
    )
}

fn check_json(id: &str, when: &str, expect: &str, mode: &str, window: &str) -> String {
    format!(
        r#"{{"id":"{id}","when":{when},"expect":{expect},"mode":"{mode}","within_ms":{window}}}"#
    )
}

fn one(when: &str, expect: &str) -> String {
    request(&case(
        "TC-1",
        &check_json("c1", when, expect, "eventually", "[0, 100]"),
    ))
}

fn pedal(op: &str, value: &str) -> String {
    cond("BrakeCmd", "BrakePedal", op, value)
}

fn on_channels(text: &str, channels: &[u8]) -> SignalMap {
    let mut maps = channels.iter().map(|channel| {
        let mut map = crate::dbc::parse(text).expect("dbc");
        for signal in &mut map.signals {
            signal.channel = *channel;
        }
        map
    });
    let mut map = maps.next().expect("one channel");
    for other in maps {
        let channel = other
            .signals
            .first()
            .map(|signal| signal.channel)
            .unwrap_or(0);
        map.append(other, channel);
    }
    map
}

#[test]
fn the_contract_example_is_accepted() {
    let json = r#"{
      "format": "loom-judge/1",
      "spec_id": "default@REQ-BRK-001",
      "cases": [
        {
          "case_id": "TC-1",
          "verifies": ["REQ-BRK-001"],
          "kind": "positive",
          "checks": [
            {
              "id": "c1",
              "given": [
                {"signal": {"message": "Ignition", "signal": "IgnState"}, "op": "==", "value": "ON"}
              ],
              "when": {"signal": {"message": "BrakeCmd", "signal": "BrakePedal"}, "op": "==", "value": 1},
              "expect": {"signal": {"message": "Lamps", "signal": "BrakeLamp"}, "op": "==", "value": 1},
              "mode": "eventually",
              "within_ms": [0, 100]
            }
          ]
        },
        {
          "case_id": "TC-empty",
          "verifies": ["REQ-BRK-002"],
          "kind": "positive",
          "checks": []
        }
      ]
    }"#;
    let dbc = cluster();
    let request = judge::check(json, &dbc).expect("the contract example");
    assert_eq!(request.format, "loom-judge/1");
    assert_eq!(request.spec_id, "default@REQ-BRK-001");
    assert!(
        request.cases[1].checks.is_empty(),
        "zero checks is not an error"
    );
    let check = &request.cases[0].checks[0];
    assert_eq!(check.given[0].value, Value::Label("ON".to_string()));
    assert_eq!(check.when.value, Value::Number(1.0));
    assert_eq!(check.when.signal.channel, None);
    assert_eq!(check.within_ms, [0, 100]);
    assert_eq!(check.mode, judge::Mode::Eventually);
    let again: Request = serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(again, request);
}

#[test]
fn format_must_be_loom_judge_1() {
    let json = request(&case("TC-1", "")).replace("loom-judge/1", "loom-judge/2");
    assert_eq!(
        problems(&json, &cluster()),
        vec!["format is \"loom-judge/2\", expected loom-judge/1".to_string()]
    );
}

#[test]
fn an_unknown_key_is_refused() {
    let dbc = cluster();
    let top = request(&case("TC-1", "")).replace(
        r#""format":"loom-judge/1""#,
        r#""format":"loom-judge/1","note":true"#,
    );
    let err = serde_json::from_str::<Request>(&top).unwrap_err();
    assert!(err.to_string().contains("note"), "{err}");
    assert_eq!(
        problems(&top, &dbc),
        vec!["unknown key \"note\" on the request".to_string()]
    );

    let nested = one(
        r#"{"signal":{"message":"BrakeCmd","signal":"BrakePedal","bus":1},"op":"==","value":1}"#,
        &pedal("==", "1"),
    );
    let err = serde_json::from_str::<Request>(&nested).unwrap_err();
    assert!(err.to_string().contains("bus"), "{err}");
    assert_eq!(
        problems(&nested, &dbc),
        vec!["unknown key \"bus\" on a signal".to_string()]
    );
}

#[test]
fn a_missing_key_is_refused() {
    let dbc = cluster();
    let top = request(&case("TC-1", "")).replace(r#""spec_id":"s","#, "");
    assert_eq!(
        problems(&top, &dbc),
        vec!["missing key \"spec_id\" on the request".to_string()]
    );

    let nested = one(
        r#"{"signal":{"message":"BrakeCmd","signal":"BrakePedal"},"value":1}"#,
        &pedal("==", "1"),
    );
    assert_eq!(
        problems(&nested, &dbc),
        vec!["missing key \"op\" on a condition".to_string()]
    );
}

#[test]
fn two_cases_cannot_share_a_case_id() {
    let json = request(&format!("{},{}", case("TC-1", ""), case("TC-1", "")));
    assert_eq!(
        problems(&json, &cluster()),
        vec!["case_id \"TC-1\" is not unique".to_string()]
    );
}

#[test]
fn two_checks_in_one_case_cannot_share_an_id() {
    let dbc = cluster();
    let check = check_json(
        "c1",
        &pedal("==", "1"),
        &pedal("==", "1"),
        "eventually",
        "[0, 100]",
    );
    let shared = request(&case("TC-1", &format!("{check},{check}")));
    assert_eq!(
        problems(&shared, &dbc),
        vec!["check id \"c1\" is not unique in case \"TC-1\"".to_string()]
    );

    let across = request(&format!(
        "{},{}",
        case("TC-1", &check),
        case("TC-2", &check)
    ));
    judge::check(&across, &dbc).expect("the same check id in two cases is two checks");
}

#[test]
fn an_op_must_be_in_the_list() {
    let json = one(&pedal("~~", "1"), &pedal("==", "1"));
    assert_eq!(
        problems(&json, &cluster()),
        vec!["op \"~~\" is not one of ==, !=, >, >=, <, <=".to_string()]
    );
}

#[test]
fn a_mode_must_be_in_the_list() {
    let json = request(&case(
        "TC-1",
        &check_json(
            "c1",
            &pedal("==", "1"),
            &pedal("==", "1"),
            "sometimes",
            "[0, 100]",
        ),
    ));
    assert_eq!(
        problems(&json, &cluster()),
        vec!["mode \"sometimes\" is not one of eventually, always, never".to_string()]
    );
}

#[test]
fn a_negative_case_with_a_never_check_is_accepted() {
    let json = request(&case(
        "TC-neg",
        &check_json(
            "c1",
            &pedal("==", "1"),
            &pedal("==", "0"),
            "never",
            "[0, 100]",
        ),
    ))
    .replace(r#""kind":"positive""#, r#""kind":"negative""#);
    let accepted = judge::check(&json, &cluster()).expect("a negative case");
    assert_eq!(accepted.cases[0].kind, judge::Kind::Negative);
    assert_eq!(accepted.cases[0].checks[0].mode, judge::Mode::Never);
}

#[test]
fn a_channel_uses_a_signal_loaded_on_channel_zero() {
    let map = cluster();
    assert!(map.signals.iter().all(|signal| signal.channel == 0));
    let json = one(
        &cond_on("BrakeCmd", "BrakePedal", 2, "==", "1"),
        &cond_on("Lamps", "BrakeLamp", 2, "==", "1"),
    );
    let accepted = judge::check(&json, &map).expect("channel 0 applies on channel 2");
    assert_eq!(accepted.cases[0].checks[0].when.signal.channel, Some(2));
    assert_eq!(accepted.cases[0].checks[0].expect.signal.channel, Some(2));
}

#[test]
fn a_channel_must_be_a_can_channel_number() {
    let json = one(
        &cond_on("BrakeCmd", "BrakePedal", 256, "==", "1"),
        &cond_on("Lamps", "BrakeLamp", -1, "==", "1"),
    );
    assert_eq!(
        problems(&json, &cluster()),
        vec![
            "channel 256 is not between 0 and 255".to_string(),
            "channel -1 is not between 0 and 255".to_string(),
        ]
    );
}

#[test]
fn a_kind_must_be_in_the_list() {
    let json = request(&case("TC-1", "")).replace(r#""kind":"positive""#, r#""kind":"bogus""#);
    assert_eq!(
        problems(&json, &cluster()),
        vec!["kind \"bogus\" is not one of positive, negative".to_string()]
    );
}

#[test]
fn within_ms_must_be_an_inclusive_window_of_two_integers() {
    let dbc = cluster();
    let window = |spec: &str| {
        request(&case(
            "TC-1",
            &check_json(
                "c1",
                &pedal("==", "1"),
                &pedal("==", "1"),
                "eventually",
                spec,
            ),
        ))
    };
    for spec in ["[0, 0]", "[10, 10]", "[0, 3600000]", "[3600000, 3600000]"] {
        judge::check(&window(spec), &dbc)
            .unwrap_or_else(|err| panic!("{spec} is inside the window rule: {:?}", err.problems));
    }
    assert_eq!(
        problems(&window("[100, 0]"), &dbc),
        vec!["within_ms [100, 0] breaks 0 <= min <= max <= 3600000".to_string()]
    );
    assert_eq!(
        problems(&window("[-1, 0]"), &dbc),
        vec!["within_ms [-1, 0] breaks 0 <= min <= max <= 3600000".to_string()]
    );
    assert_eq!(
        problems(&window("[0, 3600001]"), &dbc),
        vec!["within_ms [0, 3600001] breaks 0 <= min <= max <= 3600000".to_string()]
    );
    let shape = "within_ms must be two integers [min, max] with 0 <= min <= max <= 3600000";
    assert_eq!(problems(&window("[0]"), &dbc), vec![shape.to_string()]);
    assert_eq!(problems(&window("[0, 1.5]"), &dbc), vec![shape.to_string()]);
}

#[test]
fn a_negative_tolerance_is_refused() {
    let dbc = cluster();
    let bad = one(
        r#"{"signal":{"message":"BrakeCmd","signal":"BrakePedal"},"op":"==","value":1,"tolerance":-1}"#,
        &pedal("==", "1"),
    );
    assert_eq!(
        problems(&bad, &dbc),
        vec!["tolerance -1 is negative".to_string()]
    );

    let ok = one(
        r#"{"signal":{"message":"BrakeCmd","signal":"BrakePedal"},"op":"==","value":1,"tolerance":0.5}"#,
        &pedal("!=", "0"),
    );
    let request = judge::check(&ok, &dbc).expect("a non-negative tolerance on == and !=");
    assert_eq!(request.cases[0].checks[0].when.tolerance, Some(0.5));
    assert_eq!(request.cases[0].checks[0].expect.op, judge::Op::Ne);
}

#[test]
fn a_tolerance_on_the_wrong_op_is_refused() {
    let dbc = cluster();
    let bad = one(
        r#"{"signal":{"message":"BrakeCmd","signal":"BrakePedal"},"op":">","value":1,"tolerance":0}"#,
        &pedal(">=", "0"),
    );
    assert_eq!(
        problems(&bad, &dbc),
        vec!["tolerance on op \">\" applies to == and != only".to_string()]
    );
    let ok = one(&pedal(">", "1"), &pedal("<", "1"));
    judge::check(&ok, &dbc).expect("no tolerance on > or < is allowed");
}

#[test]
fn a_signal_missing_from_the_dbc_is_refused() {
    let dbc = cluster();
    let json = one(&cond("Nope", "Missing", "==", "1"), &pedal("==", "1"));
    let expected = dbc.lookup("Nope", "Missing").unwrap_err().to_string();
    assert_eq!(problems(&json, &dbc), vec![expected]);

    let dup = crate::dbc::parse(
        r#"
BO_ 1 Brake: 8 ESC
 SG_ Pedal : 0|8@1+ (1,0) [0|1] "" IC
 SG_ Checksum : 56|8@1+ (1,0) [0|255] "" IC
BO_ 2 Steer: 8 SWM
 SG_ Angle : 0|8@1+ (1,0) [0|255] "" IC
 SG_ Checksum : 56|8@1+ (1,0) [0|255] "" IC
"#,
    )
    .unwrap();
    let steer = dup.lookup("Steer", "Checksum").unwrap();
    assert_ne!(steer.name, steer.defined_name);
    assert_eq!(steer.message_id, 2);
    let shown = one(
        &cond("Steer", &steer.name, "==", "1"),
        &cond("Brake", "Pedal", "==", "1"),
    );
    let expected = dup.lookup("Steer", &steer.name).unwrap_err().to_string();
    assert_eq!(problems(&shown, &dup), vec![expected]);
    assert_eq!(
        dup.lookup("Steer", &steer.name).unwrap_err().kind(),
        ErrorKind::NotFound
    );
}

#[test]
fn a_string_value_must_be_a_label_of_that_signal() {
    let dbc = cluster();
    let other_signals_label = one(
        &cond("BrakeCmd", "BrakePedal", "==", r#""ON""#),
        &pedal("==", "1"),
    );
    assert_eq!(
        problems(&other_signals_label, &dbc),
        vec!["value \"ON\" is not a label of signal (BrakeCmd, BrakePedal)".to_string()]
    );
    let missing = one(
        &pedal("==", "1"),
        &cond("Ignition", "IgnState", "==", r#""SPORT""#),
    );
    assert_eq!(
        problems(&missing, &dbc),
        vec!["value \"SPORT\" is not a label of signal (Ignition, IgnState)".to_string()]
    );
    let in_given = request(&case(
        "TC-1",
        &format!(
            r#"{{"id":"c1","given":[{}],"when":{},"expect":{},"mode":"eventually","within_ms":[0,100]}}"#,
            cond("Ignition", "IgnState", "==", r#""SPORT""#),
            pedal("==", "1"),
            pedal("==", "1"),
        ),
    ));
    assert_eq!(
        problems(&in_given, &dbc),
        vec!["value \"SPORT\" is not a label of signal (Ignition, IgnState)".to_string()]
    );
    judge::check(
        &one(
            &cond("Ignition", "IgnState", "==", r#""ON""#),
            &pedal("==", "1"),
        ),
        &dbc,
    )
    .expect("ON is a label of IgnState");
}

#[test]
fn three_problems_are_listed_together() {
    let json = r#"{
      "format": "loom-judge/1",
      "spec_id": "s",
      "extra": true,
      "cases": [
        {
          "case_id": "TC-1",
          "verifies": ["R"],
          "kind": "positive",
          "checks": [
            {
              "id": "c1",
              "when": {"signal": {"message": "BrakeCmd", "signal": "BrakePedal"}, "op": "==", "value": 1},
              "expect": {"signal": {"message": "BrakeCmd", "signal": "BrakePedal"}, "op": "~~", "value": 1},
              "mode": "eventually",
              "within_ms": [0, 10]
            }
          ]
        },
        {
          "case_id": "TC-1",
          "verifies": ["R"],
          "kind": "positive",
          "checks": []
        }
      ]
    }"#;
    assert_eq!(
        problems(json, &cluster()),
        vec![
            "unknown key \"extra\" on the request".to_string(),
            "op \"~~\" is not one of ==, !=, >, >=, <, <=".to_string(),
            "case_id \"TC-1\" is not unique".to_string(),
        ]
    );
}

#[test]
fn every_listed_op_and_mode_is_accepted() {
    let dbc = cluster();
    for op in ["==", "!=", ">", ">=", "<", "<="] {
        let json = one(&pedal(op, "1"), &pedal("==", "1"));
        judge::check(&json, &dbc).unwrap_or_else(|err| panic!("{op}: {:?}", err.problems));
    }
    for mode in ["eventually", "always", "never"] {
        let json = request(&case(
            "TC-1",
            &check_json("c1", &pedal("==", "1"), &pedal("==", "1"), mode, "[0, 100]"),
        ));
        judge::check(&json, &dbc).unwrap_or_else(|err| panic!("{mode}: {:?}", err.problems));
    }
}

#[test]
fn a_reference_without_a_channel_is_refused_when_the_dbc_is_on_two_channels() {
    let map = on_channels(CLUSTER, &[1, 2]);
    let channels: Vec<u8> = map
        .signals
        .iter()
        .filter(|signal| signal.defined_name == "BrakePedal")
        .map(|signal| signal.channel)
        .collect();
    assert_eq!(channels, vec![1, 2]);
    assert_eq!(
        map.lookup("BrakeCmd", "BrakePedal").unwrap_err().kind(),
        ErrorKind::Invalid
    );

    let json = one(
        &cond("BrakeCmd", "BrakePedal", "==", "1"),
        &cond("Lamps", "BrakeLamp", "==", "1"),
    );
    assert_eq!(
        problems(&json, &map),
        vec![
            "signal pair (BrakeCmd, BrakePedal) is ambiguous; it matched channels 1 and 2"
                .to_string(),
            "signal pair (Lamps, BrakeLamp) is ambiguous; it matched channels 1 and 2".to_string(),
        ]
    );
}

#[test]
fn a_channel_picks_the_loaded_copy() {
    let map = on_channels(CLUSTER, &[1, 2]);
    let json = one(
        &cond_on("BrakeCmd", "BrakePedal", 2, "==", "1"),
        &cond_on("Lamps", "BrakeLamp", 2, "==", "1"),
    );
    let request = judge::check(&json, &map).expect("channel 2 is loaded");
    assert_eq!(request.cases[0].checks[0].when.signal.channel, Some(2));
    assert_eq!(request.cases[0].checks[0].expect.signal.channel, Some(2));
}

#[test]
fn a_channel_that_matches_nothing_is_an_unknown_signal() {
    let map = on_channels(CLUSTER, &[1, 2]);
    let json = one(
        &cond_on("BrakeCmd", "BrakePedal", 3, "==", "1"),
        &cond_on("Lamps", "BrakeLamp", 3, "==", "1"),
    );
    assert_eq!(
        problems(&json, &map),
        vec![
            "unknown signal pair (BrakeCmd, BrakePedal) on channel 3".to_string(),
            "unknown signal pair (Lamps, BrakeLamp) on channel 3".to_string(),
        ]
    );
}

#[test]
fn optional_null_fields_including_given_are_accepted() {
    let json = r#"{
      "format": "loom-judge/1",
      "spec_id": "s",
      "cases": [
        {
          "case_id": "TC-1",
          "verifies": ["REQ"],
          "kind": "positive",
          "checks": [
            {
              "id": "c1",
              "given": null,
              "when": {"signal": {"message": "BrakeCmd", "signal": "BrakePedal", "channel": null}, "op": "==", "value": 1, "tolerance": null},
              "expect": {"signal": {"message": "Lamps", "signal": "BrakeLamp", "channel": null}, "op": "==", "value": 1, "tolerance": null},
              "mode": "eventually",
              "within_ms": [0, 100]
            }
          ]
        }
      ]
    }"#;
    let request = judge::check(json, &cluster()).expect("null optional fields");
    assert!(request.cases[0].checks[0].given.is_empty());
    assert_eq!(request.cases[0].checks[0].when.signal.channel, None);
    assert_eq!(request.cases[0].checks[0].when.tolerance, None);
}
