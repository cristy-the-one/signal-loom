//! SL-13: the evaluator of plan §4, one test per row of the §4.4 table and one
//! per rule. Every recording here is generated in the test, in SLOG text.

use crate::index::IndexedLog;
use crate::judge::{self, CheckOutcome, Evaluation, Evidence, Reason, Verdict};
use crate::map::{SignalMap, TimeoutFactor};

const BRAKE_DBC: &str = r#"
BO_ 100 Ignition: 8 BCM
 SG_ IgnState : 0|8@1+ (1,0) [0|3] "" BCM
BO_ 256 BrakeCmd: 8 ESC
 SG_ BrakePedal : 0|8@1+ (1,0) [0|1] "" ESC
BO_ 512 Lamps: 8 BCM
 SG_ BrakeLamp : 0|1@1+ (1,0) [0|1] "" BCM
BO_ 768 LampStatus: 8 BCM
 SG_ LampCounter : 0|4@1+ (1,0) [0|15] "" BCM
BO_ 1024 Absent: 8 BCM
 SG_ Ghost : 0|8@1+ (0.5,0) [0|100] "" BCM
BA_DEF_ BO_ "GenMsgCycleTime" INT 0 3600000;
BA_ "GenMsgCycleTime" BO_ 100 10;
BA_ "GenMsgCycleTime" BO_ 256 10;
BA_ "GenMsgCycleTime" BO_ 512 10;
BA_ "GenMsgCycleTime" BO_ 768 10;
VAL_ 100 IgnState 0 "OFF" 1 "ON" ;
VAL_ 512 BrakeLamp 0 "OFF" 1 "ON" ;
"#;

fn dbc() -> SignalMap {
    crate::dbc::parse(BRAKE_DBC).unwrap()
}

fn frame(t_ms: u64, id: u32, first: u8) -> String {
    format!("F {} {id:X} {first:02X}00000000000000\n", t_ms * 1000)
}

/// Plan §4.4: BrakePedal and BrakeLamp every 10 ms from 0 to 10 000 ms.
/// The lamp follows the first press and misses the second; Lamps is silent
/// from 4990 to 5200 ms; the pedal rises a last time at 9990 ms.
fn brake_log() -> String {
    brake_log_with(10_000, |_| String::new())
}

/// The §4.4 frames up to 10 000 ms, plus `extra(t)` at every 10 ms step up to
/// `end_ms`, in time order: the indexer only tolerates a small step back.
fn brake_log_with(end_ms: u64, mut extra: impl FnMut(u64) -> String) -> String {
    let mut text = String::new();
    for t in (0..=end_ms).step_by(10) {
        if t <= 10_000 {
            let pedal = matches!(t, 1000..=1999 | 3000..=3999 | 5000..=5999 | 9990..) as u8;
            let lamp = matches!(t, 1040..=2029) as u8;
            text.push_str(&frame(t, 0x100, pedal));
            if !(4990..=5200).contains(&t) {
                text.push_str(&frame(t, 0x200, lamp));
            }
        }
        text.push_str(&extra(t));
    }
    text
}

fn open(text: String) -> IndexedLog {
    IndexedLog::open_bytes(text.into_bytes(), Some(&dbc())).unwrap()
}

fn evaluate(log: &IndexedLog, request: &str) -> Evaluation {
    let map = dbc();
    let request = judge::check(request, &map).expect("request is accepted");
    judge::evaluate(log, &map, TimeoutFactor::default(), &request).unwrap()
}

fn cond(message: &str, signal: &str, op: &str, value: &str) -> String {
    format!(
        r#"{{"signal":{{"message":"{message}","signal":"{signal}"}},"op":"{op}","value":{value}}}"#
    )
}

fn pedal_is(value: u8) -> String {
    cond("BrakeCmd", "BrakePedal", "==", &value.to_string())
}

fn lamp_is(value: u8) -> String {
    cond("Lamps", "BrakeLamp", "==", &value.to_string())
}

fn check(when: &str, expect: &str, mode: &str, window: [u64; 2], given: &str) -> String {
    format!(
        r#"{{"id":"c1","given":[{given}],"when":{when},"expect":{expect},"mode":"{mode}","within_ms":[{},{}]}}"#,
        window[0], window[1]
    )
}

fn request(kind: &str, checks: &str) -> String {
    format!(
        r#"{{"format":"loom-judge/1","spec_id":"s","cases":[{{"case_id":"TC-1","verifies":["REQ"],"kind":"{kind}","checks":[{checks}]}}]}}"#
    )
}

fn only_check(evaluation: &Evaluation) -> &CheckOutcome {
    assert_eq!(evaluation.cases.len(), 1);
    assert_eq!(evaluation.cases[0].checks.len(), 1);
    &evaluation.cases[0].checks[0]
}

/// (anchor_ms, verdict, evidence) for every occurrence, for compact asserts.
fn rows(check: &CheckOutcome) -> Vec<(u64, Verdict, Evidence)> {
    check
        .occurrences
        .iter()
        .map(|occ| (occ.anchor_us / 1000, occ.verdict, occ.evidence.clone()))
        .collect()
}

fn sample(t_ms: u64, value: f64) -> Evidence {
    Evidence::Sample {
        t_us: t_ms * 1000,
        value,
    }
}

fn reason(reason: Reason) -> Evidence {
    Evidence::Reason {
        reason,
        event: None,
    }
}

fn timeout_at(t_ms: u64) -> Evidence {
    Evidence::Reason {
        reason: Reason::Integrity,
        event: Some((t_ms * 1000, judge::EventKind::Timeout)),
    }
}

fn section_4_4() -> Evaluation {
    let log = open(brake_log());
    evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "eventually", [0, 100], ""),
        ),
    )
}

// ---- §4.4, one test per row ------------------------------------------------

#[test]
fn the_first_sample_is_not_an_anchor_and_four_rises_are() {
    let evaluation = section_4_4();
    let check = only_check(&evaluation);
    let anchors: Vec<u64> = check
        .occurrences
        .iter()
        .map(|occ| occ.anchor_us / 1000)
        .collect();
    assert_eq!(anchors, vec![1000, 3000, 5000, 9990]);
    assert_eq!(check.anchors, 4);
    assert_eq!(check.skipped, 0);
}

#[test]
fn row_1000_ms_lamp_on_at_1040_passes_with_that_sample() {
    let evaluation = section_4_4();
    assert_eq!(
        rows(only_check(&evaluation))[0],
        (1000, Verdict::Pass, sample(1040, 1.0))
    );
}

#[test]
fn row_3000_ms_lamp_stays_off_fails_at_the_window_end() {
    let evaluation = section_4_4();
    assert_eq!(
        rows(only_check(&evaluation))[1],
        (3000, Verdict::Fail, sample(3100, 0.0))
    );
}

#[test]
fn row_5000_ms_silent_lamps_is_inconclusive_with_the_timeout_at_5005() {
    let evaluation = section_4_4();
    assert_eq!(
        rows(only_check(&evaluation))[2],
        (5000, Verdict::Inconclusive, timeout_at(5005))
    );
}

#[test]
fn row_9990_ms_window_past_the_end_is_inconclusive() {
    let evaluation = section_4_4();
    assert_eq!(
        rows(only_check(&evaluation))[3],
        (9990, Verdict::Inconclusive, reason(Reason::WindowPastEnd))
    );
}

#[test]
fn section_4_4_rolls_up_to_a_failed_check_and_case() {
    let evaluation = section_4_4();
    let check = only_check(&evaluation);
    assert_eq!(
        (check.verdict, check.pass, check.fail, check.inconclusive),
        (Verdict::Fail, 1, 1, 2)
    );
    assert_eq!(evaluation.cases[0].verdict, Verdict::Fail);
    assert!(evaluation.integrity_complete);
}

// ---- the other modes --------------------------------------------------------

#[test]
fn never_on_the_same_recording() {
    let log = open(brake_log());
    let evaluation = evaluate(
        &log,
        &request(
            "negative",
            &check(&pedal_is(1), &lamp_is(1), "never", [0, 100], ""),
        ),
    );
    let check = only_check(&evaluation);
    assert_eq!(
        rows(check),
        vec![
            (1000, Verdict::Fail, sample(1040, 1.0)),
            (3000, Verdict::Pass, sample(3100, 0.0)),
            (5000, Verdict::Inconclusive, timeout_at(5005)),
            (9990, Verdict::Inconclusive, reason(Reason::WindowPastEnd)),
        ]
    );
    assert_eq!(check.verdict, Verdict::Fail);
}

#[test]
fn always_passes_fails_and_sees_a_silence_that_began_before_the_window() {
    let log = open(brake_log());
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "always", [50, 900], ""),
        ),
    );
    assert_eq!(
        rows(only_check(&evaluation)),
        vec![
            // Lamp is on from 1040 to 2029: the whole window 1050..1900 holds.
            (1000, Verdict::Pass, sample(1900, 1.0)),
            // Lamp is off at the window start.
            (3000, Verdict::Fail, sample(3050, 0.0)),
            // Lamps went silent at 5005, before this window opened at 5050, and
            // is still silent: the held 0 cannot be trusted.
            (5000, Verdict::Inconclusive, timeout_at(5050)),
            // The window opens after the recording ends.
            (9990, Verdict::Inconclusive, reason(Reason::WindowPastEnd)),
        ]
    );
}

#[test]
fn a_when_that_never_rises_is_not_exercised() {
    let log = open(brake_log());
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(7), &lamp_is(1), "eventually", [0, 100], ""),
        ),
    );
    let check = only_check(&evaluation);
    assert_eq!((check.verdict, check.anchors), (Verdict::NotExercised, 0));
    assert_eq!(evaluation.cases[0].verdict, Verdict::NotExercised);
}

#[test]
fn a_case_with_no_checks_is_not_judgeable() {
    let log = open(brake_log());
    let evaluation = evaluate(&log, &request("positive", ""));
    assert_eq!(evaluation.cases[0].verdict, Verdict::NotJudgeable);
    assert!(evaluation.cases[0].checks.is_empty());
}

// ---- given -----------------------------------------------------------------

/// Ignition is first sent at 2500 ms, ON until 4500 ms, then OFF.
fn with_ignition() -> String {
    brake_log_with(10_000, |t| {
        if t >= 2500 {
            frame(t, 0x64, (t < 4500) as u8)
        } else {
            String::new()
        }
    })
}

#[test]
fn given_unknown_is_inconclusive_and_given_false_is_skipped() {
    let log = open(with_ignition());
    let given = cond("Ignition", "IgnState", "==", r#""ON""#);
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "eventually", [0, 100], &given),
        ),
    );
    let check = only_check(&evaluation);
    assert_eq!(
        rows(check),
        vec![
            (1000, Verdict::Inconclusive, reason(Reason::GivenUnknown)),
            (3000, Verdict::Fail, sample(3100, 0.0)),
        ]
    );
    // 5000 and 9990: ignition OFF, skipped and not judged.
    assert_eq!((check.anchors, check.skipped), (4, 2));
    assert_eq!(check.verdict, Verdict::Fail);
}

// ---- window edges, values, tolerance ---------------------------------------

#[test]
fn a_sample_on_either_window_edge_is_inside() {
    // Pedal rises at 100 ms. Lamp is on only at exactly 200 ms and at 300 ms.
    let mut text = String::new();
    for t in (0..=1000).step_by(10) {
        text.push_str(&frame(t, 0x100, (t >= 100) as u8));
        text.push_str(&frame(t, 0x200, (t == 200 || t == 300) as u8));
    }
    let log = open(text);
    for (window, at) in [([100, 150], 200), ([150, 200], 300)] {
        let evaluation = evaluate(
            &log,
            &request(
                "positive",
                &check(&pedal_is(1), &lamp_is(1), "eventually", window, ""),
            ),
        );
        assert_eq!(
            rows(only_check(&evaluation)),
            vec![(100, Verdict::Pass, sample(at, 1.0))],
            "window {window:?}"
        );
    }
}

#[test]
fn a_label_stands_for_its_physical_value() {
    let log = open(brake_log());
    let on = cond("Lamps", "BrakeLamp", "==", r#""ON""#);
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &on, "eventually", [0, 100], ""),
        ),
    );
    assert_eq!(
        rows(only_check(&evaluation))[0],
        (1000, Verdict::Pass, sample(1040, 1.0))
    );
}

#[test]
fn equality_defaults_to_half_the_factor_and_an_explicit_tolerance_wins() {
    // Ghost has factor 0.5; raw 3 is 1.5. Absent is sent here, with one change.
    let mut text = String::new();
    for t in (0..=1000).step_by(10) {
        text.push_str(&frame(t, 0x100, (t >= 100) as u8));
        text.push_str(&frame(t, 0x400, if t >= 150 { 3 } else { 0 }));
    }
    let log = open(text);
    let run = |expect: String| {
        let evaluation = evaluate(
            &log,
            &request(
                "positive",
                &check(&pedal_is(1), &expect, "eventually", [0, 100], ""),
            ),
        );
        only_check(&evaluation).verdict
    };
    // |1.5 - 1.7| = 0.2 <= 0.25: equal under the default tolerance.
    assert_eq!(run(cond("Absent", "Ghost", "==", "1.7")), Verdict::Pass);
    // |1.5 - 1.8| = 0.3 > 0.25.
    assert_eq!(run(cond("Absent", "Ghost", "==", "1.8")), Verdict::Fail);
    let wide =
        r#"{"signal":{"message":"Absent","signal":"Ghost"},"op":"==","value":1.8,"tolerance":0.5}"#;
    assert_eq!(run(wide.to_string()), Verdict::Pass);
}

// ---- the inconclusive reasons ------------------------------------------------

#[test]
fn an_expect_signal_never_sent_has_no_value() {
    let log = open(brake_log());
    let ghost = cond("Absent", "Ghost", ">", "0");
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &ghost, "eventually", [0, 100], ""),
        ),
    );
    assert_eq!(
        rows(only_check(&evaluation))[0],
        (1000, Verdict::Inconclusive, reason(Reason::NoValue))
    );
}

#[test]
fn a_counter_event_on_the_expect_signal_inside_the_window_is_integrity() {
    // LampStatus counts up every 10 ms and skips one step at 1050 ms.
    let mut counter: u8 = 0;
    let text = brake_log_with(10_000, |t| {
        let line = frame(t, 0x300, counter & 0x0F);
        counter = counter.wrapping_add(if t == 1040 { 2 } else { 1 });
        line
    });
    let log = open(text);
    let any = cond("LampStatus", "LampCounter", ">=", "0");
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &any, "always", [0, 100], ""),
        ),
    );
    assert_eq!(
        rows(only_check(&evaluation))[0],
        (
            1000,
            Verdict::Inconclusive,
            Evidence::Reason {
                reason: Reason::Integrity,
                event: Some((1_050_000, judge::EventKind::Counter)),
            }
        )
    );
}

#[test]
fn a_truncated_event_list_turns_every_pass_into_integrity_unknown() {
    // A 1 ms message sent every 10 ms times out on every frame: more than the
    // event cap, so the event list is truncated.
    let dbc_text = format!("{BRAKE_DBC}\nBO_ 1280 Chatty: 8 BCM\n SG_ Noise : 0|8@1+ (1,0) [0|255] \"\" BCM\nBA_ \"GenMsgCycleTime\" BO_ 1280 1;\n");
    let map = crate::dbc::parse(&dbc_text).unwrap();
    let text = brake_log_with(60_000, |t| frame(t, 0x500, 0));
    let log = IndexedLog::open_bytes(text.into_bytes(), Some(&map)).unwrap();
    assert!(log.events_truncated());
    let request = judge::check(
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "eventually", [0, 100], ""),
        ),
        &map,
    )
    .unwrap();
    let evaluation = judge::evaluate(&log, &map, TimeoutFactor::default(), &request).unwrap();
    let check = only_check(&evaluation);
    assert!(!evaluation.integrity_complete);
    assert_eq!(
        rows(check)[0],
        (
            1000,
            Verdict::Inconclusive,
            reason(Reason::IntegrityUnknown)
        )
    );
    // The fail stays a fail.
    assert_eq!(rows(check)[1], (3000, Verdict::Fail, sample(3100, 0.0)));
}

// ---- added after a mutation run: each of these was a break the tests above missed

#[test]
fn a_signal_already_true_at_its_first_sample_does_not_anchor_there() {
    // Pedal is pressed from the start, released at 500 ms, pressed again at 700 ms.
    let mut text = String::new();
    for t in (0..=1000).step_by(10) {
        text.push_str(&frame(t, 0x100, !(500..700).contains(&t) as u8));
        text.push_str(&frame(t, 0x200, 1));
    }
    let log = open(text);
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "eventually", [0, 100], ""),
        ),
    );
    let anchors: Vec<u64> = only_check(&evaluation)
        .occurrences
        .iter()
        .map(|occ| occ.anchor_us / 1000)
        .collect();
    assert_eq!(anchors, vec![700]);
}

#[test]
fn always_and_never_cannot_pass_from_an_unknown_start() {
    // Pedal rises at 100 ms. The lamp is first sent at 150 ms, off, and stays off.
    let mut text = String::new();
    for t in (0..=1000).step_by(10) {
        text.push_str(&frame(t, 0x100, (t >= 100) as u8));
        if t >= 150 {
            text.push_str(&frame(t, 0x200, 0));
        }
    }
    let log = open(text);
    for mode in ["always", "never"] {
        let expect = if mode == "always" {
            lamp_is(0)
        } else {
            lamp_is(1)
        };
        let evaluation = evaluate(
            &log,
            &request(
                "positive",
                &check(&pedal_is(1), &expect, mode, [0, 200], ""),
            ),
        );
        assert_eq!(
            rows(only_check(&evaluation)),
            vec![(100, Verdict::Inconclusive, reason(Reason::NoValue))],
            "{mode}"
        );
    }
}

#[test]
fn a_message_that_stops_before_the_log_ends_is_silent_after_its_last_frame() {
    // Lamps stops at 500 ms; the pedal keeps the log going to 2000 ms.
    let mut text = String::new();
    for t in (0..=2000).step_by(10) {
        text.push_str(&frame(t, 0x100, (t >= 1000) as u8));
        if t <= 500 {
            text.push_str(&frame(t, 0x200, 0));
        }
    }
    let log = open(text);
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "eventually", [0, 100], ""),
        ),
    );
    // Silent from 500 + 25 ms; at the window start (1000 ms) it still is.
    assert_eq!(
        rows(only_check(&evaluation)),
        vec![(1000, Verdict::Inconclusive, timeout_at(1000))]
    );
}

// ---- added in the rework, from the two Grok reviews ------------------------

fn frame_us(t_us: u64, id: u32, first: u8) -> String {
    format!("F {t_us} {id:X} {first:02X}00000000000000\n")
}

/// One Vector ASC line: SLOG has no channel column.
fn asc_line(t_us: u64, channel: u8, id: u32, first: u8) -> String {
    format!(
        "{}.{:06} {channel} {id:X} Rx d 8 {first:02X} 00 00 00 00 00 00 00\n",
        t_us / 1_000_000,
        t_us % 1_000_000
    )
}

#[test]
fn a_decision_strictly_before_a_problem_in_the_same_window_stands() {
    // The lamp comes on at 1020 ms; Lamps then goes silent after 1030 ms, a
    // timeout at 1055 ms, inside the window. The pass at 1020 came first.
    let mut text = String::new();
    for t in (0..=2000).step_by(10) {
        text.push_str(&frame(t, 0x100, (t >= 1000) as u8));
        if t <= 1030 || t >= 1200 {
            text.push_str(&frame(t, 0x200, (t >= 1020) as u8));
        }
    }
    let log = open(text);
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "eventually", [0, 100], ""),
        ),
    );
    assert_eq!(
        rows(only_check(&evaluation)),
        vec![(1000, Verdict::Pass, sample(1020, 1.0))]
    );
}

#[test]
fn a_window_ending_exactly_at_the_recording_end_is_judged() {
    // The log ends at 1100 ms; the window [1000, 1100] ends there too.
    let mut text = String::new();
    for t in (0..=1100).step_by(10) {
        text.push_str(&frame(t, 0x100, (t >= 1000) as u8));
        text.push_str(&frame(t, 0x200, 0));
    }
    let log = open(text);
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "eventually", [0, 100], ""),
        ),
    );
    assert_eq!(
        rows(only_check(&evaluation)),
        vec![(1000, Verdict::Fail, sample(1100, 0.0))]
    );
}

#[test]
fn a_gap_of_exactly_the_threshold_is_not_silence_and_one_microsecond_more_is() {
    // Lamps every 10 ms, with one gap of 25 000 µs (2.5 cycles) and, later,
    // one of 25 001 µs. Each gap opens just after the pedal rises.
    let mut lines: Vec<(u64, String)> = Vec::new();
    for t in (0..=3_000_000u64).step_by(10_000) {
        let pedal = (1_000_000..1_500_000).contains(&t) || t >= 2_000_000;
        lines.push((t, frame_us(t, 0x100, pedal as u8)));
        let dropped = (1_020_000..1_031_000).contains(&t) || (2_020_000..2_031_000).contains(&t);
        if !dropped {
            lines.push((t, frame_us(t, 0x200, 0)));
        }
    }
    lines.push((1_035_000, frame_us(1_035_000, 0x200, 0)));
    lines.push((2_035_001, frame_us(2_035_001, 0x200, 0)));
    lines.sort_by_key(|(t, _)| *t);
    let log = open(lines.into_iter().map(|(_, line)| line).collect());
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "eventually", [0, 100], ""),
        ),
    );
    let got = rows(only_check(&evaluation));
    // 1 010 000 -> 1 035 000 is exactly 25 000 µs: not silent, so a plain fail.
    assert_eq!(got[0], (1000, Verdict::Fail, sample(1100, 0.0)));
    // 2 010 000 -> 2 035 001 is 25 001 µs: silent from 2 035 000.
    assert_eq!(
        got[1],
        (
            2000,
            Verdict::Inconclusive,
            Evidence::Reason {
                reason: Reason::Integrity,
                event: Some((2_035_000, judge::EventKind::Timeout)),
            }
        )
    );
}

#[test]
fn silence_follows_the_channel_the_signal_is_decoded_on() {
    // The same DBC on channels 1 and 2. On channel 1 Lamps stops at 980 ms;
    // on channel 2 it keeps going. A channel-1 check must see the silence.
    let mut map = crate::dbc::parse(BRAKE_DBC).unwrap();
    let mut copy = crate::dbc::parse(BRAKE_DBC).unwrap();
    for signal in &mut map.signals {
        signal.channel = 1;
    }
    for signal in &mut copy.signals {
        signal.channel = 2;
        signal.name = format!("{}@2", signal.name);
    }
    map.signals.extend(copy.signals);
    let mut text =
        String::from("date Mon Nov 17 00:00:00.000 2026\nbase hex timestamps absolute\n");
    for t in (0..=2000u64).step_by(10) {
        for channel in [1u8, 2] {
            text.push_str(&asc_line(t * 1000, channel, 0x100, (t >= 1000) as u8));
            if channel == 2 || t <= 980 {
                text.push_str(&asc_line(t * 1000, channel, 0x200, 0));
            }
        }
    }
    let log = IndexedLog::open_bytes(text.into_bytes(), Some(&map)).unwrap();
    let on = |channel: u8, message: &str, signal: &str, value: u8| {
        format!(
            r#"{{"signal":{{"message":"{message}","signal":"{signal}","channel":{channel}}},"op":"==","value":{value}}}"#
        )
    };
    let judge_on = |channel: u8| {
        let text = request(
            "positive",
            &check(
                &on(channel, "BrakeCmd", "BrakePedal", 1),
                &on(channel, "Lamps", "BrakeLamp", 1),
                "eventually",
                [0, 100],
                "",
            ),
        );
        let request = judge::check(&text, &map).unwrap();
        let evaluation = judge::evaluate(&log, &map, TimeoutFactor::default(), &request).unwrap();
        rows(only_check(&evaluation))
    };
    // Last channel-1 frame at 980 ms: silent from 980 + 25 = 1005 ms, inside the window.
    assert_eq!(
        judge_on(1),
        vec![(1000, Verdict::Inconclusive, timeout_at(1005))]
    );
    assert_eq!(judge_on(2), vec![(1000, Verdict::Fail, sample(1100, 0.0))]);
}

#[test]
fn a_reference_on_channel_2_finds_a_signal_loaded_on_channel_0() {
    let log = open(brake_log());
    let on2 = |message: &str, signal: &str, value: u8| {
        format!(
            r#"{{"signal":{{"message":"{message}","signal":"{signal}","channel":2}},"op":"==","value":{value}}}"#
        )
    };
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(
                &on2("BrakeCmd", "BrakePedal", 1),
                &on2("Lamps", "BrakeLamp", 1),
                "eventually",
                [0, 100],
                "",
            ),
        ),
    );
    assert_eq!(
        rows(only_check(&evaluation))[0],
        (1000, Verdict::Pass, sample(1040, 1.0))
    );
}

#[test]
fn the_default_tolerance_applies_to_equality_only() {
    // Ghost is 1.5 after 150 ms. 1.5 > 1.6 is false even though |1.5 - 1.6| is
    // inside the default tolerance of 0.25; 1.5 >= 1.5 holds.
    let mut text = String::new();
    for t in (0..=1000).step_by(10) {
        text.push_str(&frame(t, 0x100, (t >= 100) as u8));
        text.push_str(&frame(t, 0x400, if t >= 150 { 3 } else { 0 }));
    }
    let log = open(text);
    let run = |op: &str, value: &str| {
        let evaluation = evaluate(
            &log,
            &request(
                "positive",
                &check(
                    &pedal_is(1),
                    &cond("Absent", "Ghost", op, value),
                    "eventually",
                    [0, 100],
                    "",
                ),
            ),
        );
        only_check(&evaluation).verdict
    };
    assert_eq!(run(">", "1.6"), Verdict::Fail);
    // Ghost is 0 before 150 ms and 1.5 after: never below -0.1, though 0 is
    // within 0.25 of it.
    assert_eq!(run("<", "-0.1"), Verdict::Fail);
    assert_eq!(run(">=", "1.5"), Verdict::Pass);
}

#[test]
fn a_counter_event_on_another_signal_of_the_same_message_is_not_this_ones() {
    // LampStatus carries LampCounter and, here, a second counter that skips.
    let dbc_text = BRAKE_DBC.replace(
        " SG_ LampCounter : 0|4@1+ (1,0) [0|15] \"\" BCM\n",
        " SG_ LampCounter : 0|4@1+ (1,0) [0|15] \"\" BCM\n SG_ AuxCounter : 8|4@1+ (1,0) [0|15] \"\" BCM\n",
    );
    let map = crate::dbc::parse(&dbc_text).unwrap();
    let mut lamp: u8 = 0;
    let mut aux: u8 = 0;
    let text = brake_log_with(10_000, |t| {
        let line = format!(
            "F {} 300 {:02X}{:02X}000000000000\n",
            t * 1000,
            lamp & 0x0F,
            aux & 0x0F
        );
        lamp = lamp.wrapping_add(1);
        aux = aux.wrapping_add(if t == 1040 { 2 } else { 1 });
        line
    });
    let log = IndexedLog::open_bytes(text.into_bytes(), Some(&map)).unwrap();
    let any = cond("LampStatus", "LampCounter", ">=", "0");
    let text = request(
        "positive",
        &check(&pedal_is(1), &any, "always", [0, 100], ""),
    );
    let request = judge::check(&text, &map).unwrap();
    let evaluation = judge::evaluate(&log, &map, TimeoutFactor::default(), &request).unwrap();
    assert_eq!(
        rows(only_check(&evaluation))[0],
        (1000, Verdict::Pass, sample(1100, 14.0))
    );
}

#[test]
fn a_label_written_twice_stands_for_its_first_value() {
    // "ON" is listed for raw 1 and raw 3; the first, 1, is the one meant.
    let dbc_text = BRAKE_DBC.replace(
        "VAL_ 512 BrakeLamp 0 \"OFF\" 1 \"ON\" ;",
        "VAL_ 512 BrakeLamp 0 \"OFF\" 1 \"ON\" 3 \"ON\" ;",
    );
    let map = crate::dbc::parse(&dbc_text).unwrap();
    let log = IndexedLog::open_bytes(brake_log().into_bytes(), Some(&map)).unwrap();
    let on = cond("Lamps", "BrakeLamp", "==", r#""ON""#);
    let text = request(
        "positive",
        &check(&pedal_is(1), &on, "eventually", [0, 100], ""),
    );
    let request = judge::check(&text, &map).unwrap();
    let evaluation = judge::evaluate(&log, &map, TimeoutFactor::default(), &request).unwrap();
    assert_eq!(
        rows(only_check(&evaluation))[0],
        (1000, Verdict::Pass, sample(1040, 1.0))
    );
}

#[test]
fn every_sample_sitting_on_the_window_start_is_evaluated() {
    // At exactly 1000 ms the lamp is sent twice: on, then off. The window
    // [0, 50] starts there. The "on" sample counts, though "off" is held after.
    let mut text = String::new();
    for t in (0..=2000).step_by(10) {
        text.push_str(&frame(t, 0x100, (t >= 1000) as u8));
        if t == 1000 {
            text.push_str(&frame(t, 0x200, 1));
        }
        text.push_str(&frame(t, 0x200, 0));
    }
    let log = open(text);
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "eventually", [0, 50], ""),
        ),
    );
    assert_eq!(
        rows(only_check(&evaluation)),
        vec![(1000, Verdict::Pass, sample(1000, 1.0))]
    );
}

#[test]
fn a_sample_on_the_window_start_replaces_the_value_held_from_before() {
    // The lamp is on until 990 ms and turns off at exactly 1000 ms, the window
    // start. The "on" held from before must not count at the start.
    let mut text = String::new();
    for t in (0..=2000).step_by(10) {
        text.push_str(&frame(t, 0x100, (t >= 1000) as u8));
        text.push_str(&frame(t, 0x200, (t < 1000) as u8));
    }
    let log = open(text);
    let evaluation = evaluate(
        &log,
        &request(
            "positive",
            &check(&pedal_is(1), &lamp_is(1), "eventually", [0, 50], ""),
        ),
    );
    assert_eq!(
        rows(only_check(&evaluation)),
        vec![(1000, Verdict::Fail, sample(1050, 0.0))]
    );
}
