use super::IndexedLog;

/// Speed is `i` km/h at `i * 1000` µs for i in 0..=1000, a dense rising ramp.
pub(super) fn ramp_log() -> IndexedLog {
    let mut text = String::from("t_us,signal,value,unit\n");
    for i in 0..=1000u64 {
        text.push_str(&format!("{},Speed,{},km/h\n", i * 1000, i));
    }
    IndexedLog::open_bytes(text.into_bytes(), None).unwrap()
}
