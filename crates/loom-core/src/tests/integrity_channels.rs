//! SL-19: integrity timeouts and checksums follow the frame's channel.
//! An SLOG log has no channels; the hypercar lap event tests cover that case.
//!
//! SL-20: a cyclic message that stops, and never comes back, times out at the
//! end of the log. The stamp is the last frame plus the threshold, rounded up.

use crate::index::integrity::IntegrityKind;
use crate::index::IndexedLog;
use crate::map::SignalMap;

fn asc_header() -> String {
    String::from("date Mon Nov 17 00:00:00.000 2026\nbase hex timestamps absolute\n")
}

/// One Vector ASC line, same shape as `asc_line` in `tests/judge_eval.rs`.
fn asc_line(t_us: u64, channel: u8, id: u32, data: &[u8]) -> String {
    let bytes = data
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{}.{:06} {channel} {id:X} Rx d {} {bytes}\n",
        t_us / 1_000_000,
        t_us % 1_000_000,
        data.len()
    )
}

fn open(text: String, map: &SignalMap) -> IndexedLog {
    IndexedLog::open_bytes(text.into_bytes(), Some(map)).unwrap()
}

/// The same DBC on each channel, named the way a second copy is stored.
fn dbc_on_channels(text: &str, channels: &[u8]) -> SignalMap {
    let mut map = crate::dbc::parse(text).unwrap();
    for signal in &mut map.signals {
        signal.channel = channels[0];
    }
    for &channel in &channels[1..] {
        let mut copy = crate::dbc::parse(text).unwrap();
        for signal in &mut copy.signals {
            signal.channel = channel;
            signal.name = format!("{}@{channel}", signal.name);
        }
        map.signals.extend(copy.signals);
    }
    map
}

/// (a) One DBC on channel 0. Message 0x200 at a 10 ms cycle runs on channels
/// 1 and 2. Channel 1 stops after 980 ms and its next frame is 30 ms later,
/// past the 2.5-cycle timeout. Channel 2 keeps the 10 ms cadence through that
/// gap, so a `last_seen` keyed only by id never goes quiet.
#[test]
fn a_silence_on_one_channel_is_a_timeout_when_the_other_channel_keeps_sending() {
    let dbc = "\
BO_ 512 Body: 8 ECU
 SG_ Value : 0|8@1+ (1,0) [0|255] \"\" ECU
BA_ \"GenMsgCycleTime\" BO_ 512 10;
";
    let map = crate::dbc::parse(dbc).unwrap();
    assert!(map.signals.iter().all(|signal| signal.channel == 0));

    let mut text = asc_header();
    let data = [0u8; 8];
    for t_us in (0..=1_010_000u64).step_by(10_000) {
        for channel in [1u8, 2] {
            let channel_1_in_the_gap = channel == 1 && t_us > 980_000 && t_us < 1_010_000;
            if !channel_1_in_the_gap {
                text.push_str(&asc_line(t_us, channel, 0x200, &data));
            }
        }
    }
    let log = open(text, &map);
    let timeouts: Vec<_> = log
        .events()
        .iter()
        .filter(|(_, label)| label.starts_with("Timeout"))
        .cloned()
        .collect();
    assert_eq!(timeouts, vec![(1_010_000, "Timeout Body".to_string())]);
}

fn checksum_payload(n: u8, bad: bool) -> [u8; 8] {
    let mut data = [0u8; 8];
    data[1] = n;
    data[2] = 0x21;
    let sum = data[1..].iter().fold(0u8, |acc, byte| acc ^ byte);
    data[0] = if bad { sum ^ 0x55 } else { sum };
    data
}

/// (b) The same DBC loaded on channels 1 and 2. One frame on channel 2 has a
/// bad checksum. Only the channel-2 copy raises a checksum event.
#[test]
fn a_bad_checksum_on_channel_2_does_not_raise_the_channel_1_copy() {
    let dbc = "\
BO_ 512 Body: 8 ECU
 SG_ Checksum : 0|8@1+ (1,0) [0|255] \"\" ECU
";
    let map = dbc_on_channels(dbc, &[1, 2]);
    assert_eq!(
        map.signals
            .iter()
            .map(|signal| (signal.channel, signal.name.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "Checksum"), (2, "Checksum@2")]
    );

    let mut text = asc_header();
    let bad_us = 200_000u64;
    for n in 0..=20u8 {
        let t_us = u64::from(n) * 10_000;
        text.push_str(&asc_line(t_us, 1, 0x200, &checksum_payload(n, false)));
        let bad = t_us == bad_us;
        text.push_str(&asc_line(t_us, 2, 0x200, &checksum_payload(n, bad)));
    }
    let log = open(text, &map);
    let checksums: Vec<_> = log
        .events()
        .iter()
        .filter(|(_, label)| label.starts_with("Checksum"))
        .cloned()
        .collect();
    assert_eq!(checksums, vec![(bad_us, "Checksum Checksum@2".to_string())]);

    let stored: Vec<_> = log
        .integrity_events()
        .iter()
        .map(|event| {
            (
                event.t_us,
                event.kind,
                event.message_id,
                event.signal.as_str(),
            )
        })
        .collect();
    assert_eq!(
        stored,
        vec![(bad_us, IntegrityKind::Checksum, 0x200, "Checksum@2")]
    );
}

/// 0x200 at a 10 ms cycle. The default timeout is 2.5 cycles, 25 ms.
const BODY_10MS: &str = "\
BO_ 512 Body: 8 ECU
 SG_ Value : 0|8@1+ (1,0) [0|255] \"\" ECU
BA_ \"GenMsgCycleTime\" BO_ 512 10;
";

/// `0x200` every 10 ms on `channel` from 0 through `last_us`.
fn frames_until(text: &mut String, channel: u8, last_us: u64) {
    let data = [0u8; 8];
    for t_us in (0..=last_us).step_by(10_000) {
        text.push_str(&asc_line(t_us, channel, 0x200, &data));
    }
}

/// A frame with no cycle time, so the log's end is 2000 ms even when every
/// watched message has already stopped.
fn marker_at_end(text: &mut String) {
    text.push_str(&asc_line(2_000_000, 1, 0x100, &[0]));
}

/// (a) A 2000 ms log. 0x200, cycle 10 ms, stops at 980 ms and never returns.
/// 980 ms + ceil(10 ms × 2.5) = 1005 ms. Today nothing is raised: the gap is
/// only noticed when a later frame arrives.
#[test]
fn a_message_that_stops_and_never_returns_times_out_at_1005_ms() {
    let map = crate::dbc::parse(BODY_10MS).unwrap();
    let mut text = asc_header();
    frames_until(&mut text, 1, 980_000);
    marker_at_end(&mut text);
    let log = open(text, &map);
    assert_eq!(log.t_end_us(), 2_000_000);
    assert_eq!(log.events(), &[(1_005_000, "Timeout Body".to_string())][..]);
}

/// (b) The same message stops at 1980 ms. The gap to the end is 20 ms, inside
/// the 25 ms threshold, so the end of the log raises nothing.
#[test]
fn a_stop_inside_the_end_threshold_raises_no_timeout() {
    let map = crate::dbc::parse(BODY_10MS).unwrap();
    let mut text = asc_header();
    frames_until(&mut text, 1, 1_980_000);
    marker_at_end(&mut text);
    let log = open(text, &map);
    assert_eq!(log.t_end_us(), 2_000_000);
    assert!(log.events().is_empty(), "{:?}", log.events());
}

/// (c) Two channels of 0x200. Channel 1 stops at 980 ms. Channel 2 keeps the
/// 10 ms cadence through 2000 ms. Only the channel that stopped times out.
#[test]
fn only_the_channel_that_stops_raises_an_end_of_log_timeout() {
    let map = crate::dbc::parse(BODY_10MS).unwrap();
    let mut text = asc_header();
    let data = [0u8; 8];
    for t_us in (0..=2_000_000u64).step_by(10_000) {
        if t_us <= 980_000 {
            text.push_str(&asc_line(t_us, 1, 0x200, &data));
        }
        text.push_str(&asc_line(t_us, 2, 0x200, &data));
    }
    let log = open(text, &map);
    assert_eq!(log.t_end_us(), 2_000_000);
    assert_eq!(log.events(), &[(1_005_000, "Timeout Body".to_string())][..]);
}

/// The timeout is stamped at 1005 ms, which is before an error frame recorded
/// at 1500 ms. `finish` must put that stamp in time order, not after the scan.
#[test]
fn an_end_of_log_timeout_is_kept_in_time_order() {
    let map = crate::dbc::parse(BODY_10MS).unwrap();
    let mut text = asc_header();
    frames_until(&mut text, 1, 980_000);
    text.push_str("1.500000 1 ErrorFrame\n");
    marker_at_end(&mut text);
    let log = open(text, &map);
    assert_eq!(
        log.events(),
        &[
            (1_005_000, "Timeout Body".to_string()),
            (1_500_000, "Error frame".to_string()),
        ][..]
    );
}
