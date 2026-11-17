//! SL-17: the `loom-dbc-dump/1` text `dump_dbc` returns.

use serde_json::Value;

use crate::dump_dbc;

fn dump_text(dbc: &str) -> String {
    let map = crate::dbc::parse(dbc).unwrap_or_else(|err| panic!("parse: {err}"));
    let text = dump_dbc(&map);
    assert_layout(&text);
    text
}

fn dump_parsed(dbc: &str) -> Value {
    let text = dump_text(dbc);
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("dump is not json: {err}\n{text}"))
}

fn signal<'a>(dump: &'a Value, name: &str) -> &'a Value {
    dump["signals"]
        .as_array()
        .expect("signals")
        .iter()
        .find(|signal| signal["signal"] == name)
        .unwrap_or_else(|| panic!("no signal {name} in {dump}"))
}

fn assert_layout(text: &str) {
    assert!(
        text.starts_with("{\n  \"format\": \"loom-dbc-dump/1\",\n  \"signals\": [\n"),
        "{text}"
    );
    assert!(text.ends_with("  ]\n}\n"), "{text}");
    let signal_lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("\"signal\""))
        .collect();
    assert!(!signal_lines.is_empty(), "{text}");
    let last = signal_lines.len() - 1;
    for (index, line) in signal_lines.iter().enumerate() {
        assert!(
            line.starts_with("    {"),
            "a signal is one line at indent 4: {line}"
        );
        assert_key_order(line);
        if index == last {
            assert!(!line.ends_with(','), "no trailing comma: {line}");
        } else {
            assert!(line.ends_with(','), "comma between signals: {line}");
        }
    }
}

fn assert_key_order(line: &str) {
    let keys = [
        "\"message\"",
        "\"id\"",
        "\"signal\"",
        "\"start_bit\"",
        "\"length\"",
        "\"byte_order\"",
        "\"signed\"",
        "\"factor\"",
        "\"offset\"",
        "\"unit\"",
        "\"multiplexer\"",
        "\"multiplexed_by\"",
        "\"labels\"",
    ];
    let mut from = 0;
    for key in keys {
        let pos = line[from..]
            .find(key)
            .unwrap_or_else(|| panic!("missing {key} after {from} in {line}"));
        from += pos + key.len();
    }
}

#[test]
fn a_motorola_signal_prints_start_bit_7_and_big_endian() {
    // `7|16@0+` is stored as written. The dump prints that start bit, not an
    // LSB conversion.
    let text = dump_text(
        "BO_ 256 MotorolaDemo: 8 ECM\n SG_ BigWord : 7|16@0+ (1,0) [0|65535] \"\" Vector__XXX\n",
    );
    assert!(text.contains("\"start_bit\": 7"), "{text}");
    assert!(text.contains("\"byte_order\": \"big_endian\""), "{text}");
    let dump = serde_json::from_str::<Value>(&text).unwrap();
    let signal = signal(&dump, "BigWord");
    assert_eq!(signal["message"], "MotorolaDemo");
    assert_eq!(signal["id"], 256);
    assert_eq!(signal["start_bit"], 7);
    assert_eq!(signal["length"], 16);
    assert_eq!(signal["byte_order"], "big_endian");
    assert_eq!(signal["signed"], false);
    assert_eq!(signal["factor"], 1);
    assert_eq!(signal["offset"], 0);
    assert_eq!(signal["multiplexer"], false);
    assert_eq!(signal["multiplexed_by"], Value::Null);
    assert_eq!(signal["labels"], serde_json::json!([]));
}

#[test]
fn multiplexed_signals_print_multiplexer_and_multiplexed_by() {
    let dump = dump_parsed(
        "\
BO_ 100 MuxMsg: 8 ECM
 SG_ Mode M : 0|8@1+ (1,0) [0|0] \"\" Vector__XXX
 SG_ Gear m0 : 8|8@1+ (1,0) [0|0] \"\" Vector__XXX
 SG_ Reverse m3 : 16|8@1+ (1,0) [0|0] \"\" Vector__XXX
",
    );
    let mode = signal(&dump, "Mode");
    assert_eq!(mode["multiplexer"], true);
    assert_eq!(mode["multiplexed_by"], Value::Null);
    let gear = signal(&dump, "Gear");
    assert_eq!(gear["multiplexer"], false);
    assert_eq!(gear["multiplexed_by"], 0);
    let reverse = signal(&dump, "Reverse");
    assert_eq!(reverse["multiplexer"], false);
    assert_eq!(reverse["multiplexed_by"], 3);
}

#[test]
fn an_extended_id_prints_without_bit_31() {
    // 2147483904 = 0x80000100, bit 31 set on top of id 256.
    let text = dump_text(
        "BO_ 2147483904 ExtMsg: 8 ECM\n SG_ Flag : 0|8@1+ (1,0) [0|1] \"\" Vector__XXX\n",
    );
    assert!(!text.contains("2147483904"), "{text}");
    let dump = serde_json::from_str::<Value>(&text).unwrap();
    assert_eq!(signal(&dump, "Flag")["id"], 256);
    assert_eq!(signal(&dump, "Flag")["message"], "ExtMsg");
}

#[test]
fn the_printed_name_is_the_dbc_name_not_the_display_name() {
    let text = dump_text(
        "\
BO_ 1 A: 8 ECM
 SG_ Counter : 0|4@1+ (1,0) [0|15] \"\" X
BO_ 2 B: 8 ECM
 SG_ Counter : 0|4@1+ (1,0) [0|15] \"\" X
",
    );
    assert!(
        !text.contains("Counter@"),
        "display name Name@<id> must not be printed\n{text}"
    );
    let dump: Value = serde_json::from_str(&text).unwrap();
    let names: Vec<&str> = dump["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|signal| signal["signal"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Counter", "Counter"]);
    assert_eq!(dump["signals"][0]["message"], "A");
    assert_eq!(dump["signals"][1]["message"], "B");
}

#[test]
fn labels_keep_every_entry_sorted_by_raw_value() {
    let dump = dump_parsed(
        "\
BO_ 1 Msg: 8 ECM
 SG_ Sig : 0|8@1+ (1,0) [0|0] \"\" X
VAL_ 1 Sig 2 \"b\" 1 \"a\" 2 \"c\" 0 \"z\";
",
    );
    assert_eq!(
        signal(&dump, "Sig")["labels"],
        serde_json::json!([[0, "z"], [1, "a"], [2, "b"], [2, "c"]])
    );
}

#[test]
fn a_whole_factor_and_offset_are_integers() {
    let text = dump_text(
        "BO_ 257 ECM_Slow: 8 ECM\n SG_ CoolantTemp : 0|8@1+ (1,-40) [-40|215] \"degC\" IC\n",
    );
    assert!(text.contains("\"factor\": 1,"), "{text}");
    assert!(text.contains("\"offset\": -40,"), "{text}");
    assert!(!text.contains("1.0"), "{text}");
    assert!(!text.contains("-40.0"), "{text}");
    let dump: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(signal(&dump, "CoolantTemp")["unit"], "degC");
    assert_eq!(signal(&dump, "CoolantTemp")["offset"], -40);
}
