//! ASR-33 seven-bit parity operations.

/// Encode even parity in the most significant bit of every byte.
///
/// This deliberately mirrors the Python implementation for all eight-bit
/// inputs: an input MSB is considered while parity is calculated, then the
/// resulting MSB is set or cleared.
#[must_use]
pub fn encode_even_parity(data: &[u8]) -> Vec<u8> {
    data.iter()
        .map(|&byte| {
            if byte.count_ones() % 2 == 1 {
                byte | 0x80
            } else {
                byte & 0x7f
            }
        })
        .collect()
}

/// Clear the parity bit from every byte.
#[must_use]
pub fn mask_parity_bit(data: &[u8]) -> Vec<u8> {
    data.iter().map(|byte| byte & 0x7f).collect()
}

#[cfg(test)]
mod tests {
    use super::{encode_even_parity, mask_parity_bit};

    #[test]
    fn even_parity_is_exhaustive_for_seven_bit_values() {
        for byte in 0_u8..=0x7f {
            let encoded = encode_even_parity(&[byte]);
            assert_eq!(encoded.len(), 1);
            assert_eq!(encoded[0] & 0x7f, byte);
            assert_eq!(encoded[0].count_ones() % 2, 0, "input byte {byte:#04x}");
        }
    }

    #[test]
    fn masking_is_exhaustive_for_all_bytes() {
        let input: Vec<u8> = (u8::MIN..=u8::MAX).collect();
        let expected: Vec<u8> = input.iter().map(|byte| byte % 128).collect();
        assert_eq!(mask_parity_bit(&input), expected);
    }
}
