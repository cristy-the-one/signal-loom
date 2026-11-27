/// Byte order for a signal packed into a CAN payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    /// Intel / little-endian. `start_bit` is the least significant bit.
    Little,
    /// Motorola / big-endian DBC numbering. `start_bit` is the most significant bit.
    Big,
}

/// A CAN FD payload is up to 64 bytes. A frame shorter than a signal skips it.
const MAX_PAYLOAD_BITS: u32 = 64 * 8;

/// How to turn payload bits into a physical value.
#[derive(Debug, Clone, Copy)]
pub struct DecodeSpec {
    pub start_bit: u16,
    pub bit_length: u16,
    pub factor: f64,
    pub offset: f64,
    pub signed: bool,
    pub endian: Endian,
}

impl DecodeSpec {
    pub fn validate(self) -> Result<(), String> {
        if self.bit_length == 0 || self.bit_length > 64 {
            return Err(format!("bit length {} is outside 1..=64", self.bit_length));
        }
        if !self.factor.is_finite() || !self.offset.is_finite() {
            return Err("factor and offset must be finite".into());
        }
        match self.endian {
            Endian::Little => {
                let end = u32::from(self.start_bit) + u32::from(self.bit_length);
                if end > MAX_PAYLOAD_BITS {
                    return Err(format!(
                        "little-endian signal starting at bit {} length {} does not fit in 64 bytes",
                        self.start_bit, self.bit_length
                    ));
                }
            }
            Endian::Big => walk_be(self.start_bit, self.bit_length)?,
        }
        Ok(())
    }

    /// The unsigned field bits, before sign, factor and offset.
    pub fn raw(self, data: &[u8]) -> u64 {
        extract_bits(data, self.start_bit, self.bit_length, self.endian)
    }

    pub fn decode(self, data: &[u8]) -> f64 {
        let raw = self.raw(data);
        let raw = if self.signed {
            sign_extend(raw, self.bit_length) as f64
        } else {
            raw as f64
        };
        raw * self.factor + self.offset
    }

    /// The raw multiplexer value. `VAL_` and mux ids name raw values, not scaled ones.
    pub fn switch_value(self, data: &[u8]) -> Option<u32> {
        u32::try_from(self.raw(data)).ok()
    }

    /// The raw integer behind a decoded value, for `VAL_` lookups.
    pub fn raw_of(self, value: f64) -> Option<i64> {
        if self.factor == 0.0 {
            return None;
        }
        let raw = ((value - self.offset) / self.factor).round();
        raw.is_finite().then_some(raw as i64)
    }

    /// Bytes the signal reads. A shorter frame does not carry it.
    pub fn bytes_needed(self) -> usize {
        let len = u32::from(self.bit_length).max(1);
        match self.endian {
            Endian::Little => {
                let last = u32::from(self.start_bit) + len - 1;
                last as usize / 8 + 1
            }
            Endian::Big => {
                let start_byte = u32::from(self.start_bit / 8);
                let first_byte_bits = u32::from(self.start_bit % 8) + 1;
                let extra_bytes = len.saturating_sub(first_byte_bits).div_ceil(8);
                (start_byte + extra_bytes + 1) as usize
            }
        }
    }
}

pub fn extract_bits(data: &[u8], start_bit: u16, bit_length: u16, endian: Endian) -> u64 {
    match endian {
        Endian::Little => extract_le(data, start_bit, bit_length),
        Endian::Big => extract_be(data, start_bit, bit_length),
    }
}

pub fn sign_extend(value: u64, bits: u16) -> i64 {
    if bits == 0 {
        return 0;
    }
    if bits >= 64 {
        return value as i64;
    }
    let sign = 1u64 << (bits - 1);
    if value & sign != 0 {
        let mask = (1u64 << bits) - 1;
        (value | !mask) as i64
    } else {
        value as i64
    }
}

fn extract_le(data: &[u8], start_bit: u16, bit_length: u16) -> u64 {
    let mut rem = u32::from(bit_length);
    let mut byte_idx = usize::from(start_bit / 8);
    let mut bit_offset = u32::from(start_bit % 8);
    let mut out_shift = 0u32;
    let mut value = 0u64;
    while rem > 0 && byte_idx < data.len() {
        let take = rem.min(8 - bit_offset);
        let mask = ((1u16 << take) - 1) as u8;
        let chunk = u64::from((data[byte_idx] >> bit_offset) & mask);
        value |= chunk << out_shift;
        out_shift += take;
        rem -= take;
        byte_idx += 1;
        bit_offset = 0;
    }
    value
}

fn extract_be(data: &[u8], start_bit: u16, bit_length: u16) -> u64 {
    let mut rem = u32::from(bit_length);
    let mut byte_idx = usize::from(start_bit / 8);
    let mut avail = u32::from(start_bit % 8) + 1;
    let mut value = 0u64;
    while rem > 0 {
        let take = rem.min(avail);
        let chunk = if byte_idx < data.len() {
            let lsb = avail - take;
            let mask = ((1u16 << take) - 1) as u8;
            u64::from((data[byte_idx] >> lsb) & mask)
        } else {
            0
        };
        value = (value << take) | chunk;
        rem -= take;
        byte_idx += 1;
        avail = 8;
    }
    value
}

fn walk_be(start_bit: u16, bit_length: u16) -> Result<(), String> {
    let mut bit = start_bit;
    if u32::from(start_bit) >= MAX_PAYLOAD_BITS {
        return Err(format!(
            "big-endian start bit {start_bit} is outside 64 bytes"
        ));
    }
    for _ in 0..bit_length {
        if u32::from(bit) >= MAX_PAYLOAD_BITS {
            return Err("big-endian signal walks outside the 64-byte payload".into());
        }
        let b = bit % 8;
        if b == 0 {
            bit = bit.saturating_add(15);
        } else {
            bit -= 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn little_endian_rpm_layout() {
        // raw 3200 = 0x0C80, factor 0.25 -> 800 rpm
        let data = [0x80, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let spec = DecodeSpec {
            start_bit: 0,
            bit_length: 16,
            factor: 0.25,
            offset: 0.0,
            signed: false,
            endian: Endian::Little,
        };
        assert!((spec.decode(&data) - 800.0).abs() < 1e-9);
    }

    #[test]
    fn offset_and_sign() {
        let spec = DecodeSpec {
            start_bit: 0,
            bit_length: 8,
            factor: 1.0,
            offset: -40.0,
            signed: false,
            endian: Endian::Little,
        };
        assert!((spec.decode(&[120]) - 80.0).abs() < 1e-9);

        let signed = DecodeSpec {
            start_bit: 0,
            bit_length: 8,
            factor: 1.0,
            offset: 0.0,
            signed: true,
            endian: Endian::Little,
        };
        assert!((signed.decode(&[0xFF]) - -1.0).abs() < 1e-9);
    }

    #[test]
    fn big_endian_matches_byte_order() {
        // start bit 7, 16 bits over 0x12 0x34 -> 0x1234
        let data = [0x12, 0x34];
        let raw = extract_bits(&data, 7, 16, Endian::Big);
        assert_eq!(raw, 0x1234);
    }

    #[test]
    fn byte_wise_extraction_and_bytes_needed_match_bit_by_bit_reference() {
        fn ref_le(data: &[u8], start_bit: u16, bit_length: u16) -> u64 {
            let mut value = 0u64;
            for i in 0..bit_length {
                let bit_index = u32::from(start_bit) + u32::from(i);
                let byte = (bit_index / 8) as usize;
                let bit = (bit_index % 8) as u8;
                if byte < data.len() && (data[byte] >> bit) & 1 == 1 {
                    value |= 1u64 << i;
                }
            }
            value
        }

        fn ref_be(data: &[u8], start_bit: u16, bit_length: u16) -> u64 {
            let mut value = 0u64;
            let mut bit = start_bit;
            for _ in 0..bit_length {
                let byte = (bit / 8) as usize;
                let b = (bit % 8) as u8;
                value <<= 1;
                if byte < data.len() && (data[byte] >> b) & 1 == 1 {
                    value |= 1;
                }
                bit = if b == 0 {
                    bit.saturating_add(15)
                } else {
                    bit - 1
                };
            }
            value
        }

        fn ref_be_bytes(start_bit: u16, bit_length: u16) -> usize {
            let mut bit = start_bit;
            let mut last = bit / 8;
            for _ in 1..bit_length {
                bit = if bit.is_multiple_of(8) {
                    bit.saturating_add(15)
                } else {
                    bit - 1
                };
                last = last.max(bit / 8);
            }
            usize::from(last) + 1
        }

        let payload: [u8; 64] = std::array::from_fn(|i| ((i as u8).wrapping_mul(37)) ^ 0xA5);
        for slice_len in [0usize, 1, 5, 8, 64] {
            let data = &payload[..slice_len];
            for start_bit in (0..64u16).chain([448, 455, 504, 511]) {
                for bit_length in [0u16, 1, 3, 7, 8, 9, 12, 16, 23, 32, 48, 63, 64] {
                    assert_eq!(
                        extract_le(data, start_bit, bit_length),
                        ref_le(data, start_bit, bit_length),
                        "le start={start_bit} len={bit_length} slice={slice_len}"
                    );
                    assert_eq!(
                        extract_be(data, start_bit, bit_length),
                        ref_be(data, start_bit, bit_length),
                        "be start={start_bit} len={bit_length} slice={slice_len}"
                    );
                }
            }
        }
        for start_bit in 0..512u16 {
            for bit_length in 0..=64u16 {
                let spec = DecodeSpec {
                    start_bit,
                    bit_length,
                    factor: 1.0,
                    offset: 0.0,
                    signed: false,
                    endian: Endian::Big,
                };
                assert_eq!(
                    spec.bytes_needed(),
                    ref_be_bytes(start_bit, bit_length),
                    "bytes_needed start={start_bit} len={bit_length}"
                );
            }
        }
    }
}
