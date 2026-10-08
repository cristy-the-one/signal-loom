//! CAN identifier parsing and the 11-bit / 29-bit decision, in one place.
//!
//! Each log format keeps its own number base. This module only holds the code,
//! so a format picks a base and says whether the frame is extended.

/// Highest 11-bit identifier. A format that does not say whether a frame is
/// extended is read as extended above this.
pub(crate) const MAX_STANDARD_ID: u32 = 0x7FF;

const ID_29_MASK: u32 = 0x1FFF_FFFF;
const EXTENDED_FLAG: u32 = 0x8000_0000;

/// How a format writes its identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdBase {
    /// Bare hex digits: Vector ASC under `base hex`, candump.
    Hex,
    /// Hex digits with an optional `0x` prefix: SLOGv1.
    PrefixedHex,
    /// Decimal digits: Vector ASC under `base dec`.
    Decimal,
    /// Hex with a `0x` prefix or any `A-F` digit, decimal otherwise: CAN CSV and
    /// signal maps.
    Auto,
}

/// The number in `text` as `base` reads it, or `None` when it is empty or not a
/// number. The value is not masked.
pub(crate) fn parse_id(text: &str, base: IdBase) -> Option<u32> {
    let text = text.trim();
    let hex = |digits| u32::from_str_radix(digits, 16).ok();
    match base {
        IdBase::Hex => hex(text),
        IdBase::PrefixedHex => hex(strip_0x(text).unwrap_or(text)),
        IdBase::Decimal => text.parse().ok(),
        IdBase::Auto => {
            if let Some(digits) = strip_0x(text) {
                hex(digits)
            } else if text
                .chars()
                .any(|c| c.is_ascii_hexdigit() && !c.is_ascii_digit())
            {
                hex(text)
            } else {
                text.parse().ok()
            }
        }
    }
}

fn strip_0x(text: &str) -> Option<&str> {
    text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"))
}

/// The low 29 bits of a raw identifier.
pub(crate) fn mask_29(raw: u32) -> u32 {
    raw & ID_29_MASK
}

/// An identifier whose bit 31 marks it extended (BLF, DBC): the 29-bit id and
/// that flag.
pub(crate) fn split_flagged(raw: u32) -> (u32, bool) {
    (mask_29(raw), raw & EXTENDED_FLAG != 0)
}

/// Extended-ness for a format that does not say: any id above 11 bits.
pub(crate) fn implied_extended(id: u32) -> bool {
    id > MAX_STANDARD_ID
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{LogFormat, RecKind, Scanner};
    use std::io::Cursor;

    fn frames(format: LogFormat, text: &str) -> Vec<(u32, bool)> {
        let mut reader = Cursor::new(text.as_bytes().to_vec());
        let mut scanner = Scanner::open(&mut reader, format).unwrap();
        std::iter::from_fn(|| scanner.next_rec().unwrap())
            .filter_map(|rec| match rec.kind {
                RecKind::Frame { id, extended, .. } => Some((id, extended)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn each_base_reads_its_own_digits() {
        assert_eq!(parse_id("1A0", IdBase::Hex), Some(0x1A0));
        assert_eq!(parse_id("0x1A0", IdBase::Hex), None);
        assert_eq!(parse_id("0x1A0", IdBase::PrefixedHex), Some(0x1A0));
        assert_eq!(parse_id("1A0", IdBase::PrefixedHex), Some(0x1A0));
        assert_eq!(parse_id("416", IdBase::Decimal), Some(416));
        assert_eq!(parse_id("1A0", IdBase::Decimal), None);
        assert_eq!(parse_id("", IdBase::Hex), None);
        assert_eq!(parse_id("0x", IdBase::PrefixedHex), None);
    }

    #[test]
    fn auto_is_decimal_unless_prefixed_or_a_to_f() {
        assert_eq!(parse_id("100", IdBase::Auto), Some(100));
        assert_eq!(parse_id("0x100", IdBase::Auto), Some(0x100));
        assert_eq!(parse_id("1a0", IdBase::Auto), Some(0x1A0));
        assert_eq!(parse_id(" 18FF50E5 ", IdBase::Auto), Some(0x18FF_50E5));
        assert_eq!(parse_id("12G", IdBase::Auto), None);
    }

    #[test]
    fn flagged_ids_split_into_a_29_bit_id_and_the_extended_flag() {
        assert_eq!(split_flagged(0x8123_4567), (0x0123_4567, true));
        assert_eq!(split_flagged(0x0000_01A0), (0x1A0, false));
        assert_eq!(mask_29(0xFFFF_FFFF), 0x1FFF_FFFF);
    }

    #[test]
    fn implied_extended_starts_above_0x7ff() {
        assert!(!implied_extended(0x7FF));
        assert!(implied_extended(0x800));
    }

    #[test]
    fn slog_and_can_csv_apply_the_0x7ff_rule() {
        let slog = "SLOGv1\nF 0 7FF 01\nF 1 800 02\nF 2 0x18FF50E5 03\n";
        assert_eq!(
            frames(LogFormat::Slog, slog),
            vec![(0x7FF, false), (0x800, true), (0x18FF_50E5, true)]
        );
        let csv = "t_us,id,data\n0,2047,01\n1,2048,02\n2,0x7FF,03\n";
        assert_eq!(
            frames(LogFormat::CanCsv, csv),
            vec![(2047, false), (2048, true), (0x7FF, false)]
        );
    }

    #[test]
    fn asc_extended_comes_from_the_x_suffix_only() {
        let text = "base hex timestamps absolute\n\
            0.000000 1 1A0 Rx d 1 01\n\
            0.001000 1 1A0x Rx d 1 02\n\
            0.002000 1 800 Rx d 1 03\n\
            0.003000 CANFD 1 Rx 18FF50E5x 0 0 d 15 1 04\n";
        assert_eq!(
            frames(LogFormat::Asc, text),
            vec![
                (0x1A0, false),
                (0x1A0, true),
                (0x800, false),
                (0x18FF_50E5, true)
            ]
        );
    }

    #[test]
    fn candump_eight_digit_ids_are_extended_whatever_their_value() {
        let text = "(0.000000) can0 00000123#01\n\
            (0.001000) can0 123#02\n\
            (0.002000) can0 800#03\n\
            (0.003000) can0 18FF50E5#04\n\
            can0 00000100 [1] 05\n";
        assert_eq!(
            frames(LogFormat::Candump, text),
            vec![
                (0x123, true),
                (0x123, false),
                (0x800, true),
                (0x18FF_50E5, true),
                (0x100, true)
            ]
        );
    }
}
