/// Whole numbers inside the exact integer range of `f64` are written without
/// a fraction, as the contract fixture writes `1` and `0`. Anything else uses
/// serde_json's number format. A non-finite value has no JSON number, so it
/// is `null`.
pub(crate) fn json_number(value: f64) -> String {
    const EXACT: f64 = (1_i64 << 53) as f64;
    if value.is_finite() && (-EXACT..=EXACT).contains(&value) {
        let int = value as i64;
        if int as f64 == value {
            return int.to_string();
        }
    }
    serde_json::Number::from_f64(value)
        .map(|number| number.to_string())
        .unwrap_or_else(|| "null".to_string())
}
