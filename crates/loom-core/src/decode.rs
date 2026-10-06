/// Byte order for a signal packed into a CAN payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    /// Intel / little-endian. `start_bit` is the least significant bit.
    Little,
    /// Motorola / big-endian DBC numbering. `start_bit` is the most significant bit.
    Big,
}

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
                if end > 64 {
                    return Err(format!(
                        "little-endian signal starting at bit {} length {} does not fit in 8 bytes",
                        self.start_bit, self.bit_length
                    ));
                }
            }
            Endian::Big => walk_be(self.start_bit, self.bit_length)?,
        }
        Ok(())
    }

    pub fn decode(self, data: &[u8]) -> f64 {
        let raw = extract_bits(data, self.start_bit, self.bit_length, self.endian);
        let signed = if self.signed {
            sign_extend(raw, self.bit_length)
        } else {
            raw as i64
        };
        signed as f64 * self.factor + self.offset
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

fn extract_be(data: &[u8], start_bit: u16, bit_length: u16) -> u64 {
    let mut value = 0u64;
    let mut bit = start_bit;
    for _ in 0..bit_length {
        let byte = (bit / 8) as usize;
        let b = (bit % 8) as u8;
        value <<= 1;
        if byte < data.len() && (data[byte] >> b) & 1 == 1 {
            value |= 1;
        }
        if b == 0 {
            bit = bit.saturating_add(15);
        } else {
            bit -= 1;
        }
    }
    value
}

fn walk_be(start_bit: u16, bit_length: u16) -> Result<(), String> {
    let mut bit = start_bit;
    if (start_bit / 8) >= 8 {
        return Err(format!(
            "big-endian start bit {start_bit} is outside 8 bytes"
        ));
    }
    for _ in 0..bit_length {
        if (bit / 8) >= 8 {
            return Err("big-endian signal walks outside the 8-byte payload".into());
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
}
