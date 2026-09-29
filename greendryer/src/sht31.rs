//! SHT31 single-shot sample, without clock stretching.
//!
//! The ESP32 I2C master can hang on the `0x2C` stretch family. The cabinet
//! writes `0x24` then `0x00`, waits, and checks both CRC bytes before it
//! converts the words.

use crate::{ProbeError, ProbeMeasure};

/// I2C address of the cabinet probe.
pub(crate) const ADDRESS: u8 = 0x44;

/// Single-shot, high-repeatability measurement with clock stretching disabled.
pub(crate) const MEASURE_HIGH_NO_STRETCH: [u8; 2] = [0x24, 0x00];

/// Milliseconds to wait after the measure command before reading six bytes.
pub(crate) const MEASURE_WAIT_MS: u64 = 16;

/// Decode one six-byte SHT31 response.
///
/// The bytes are temperature MSB, temperature LSB, temperature CRC, humidity
/// MSB, humidity LSB, humidity CRC. Either bad CRC rejects the sample.
/// Temperature is `raw_t * 175 / 65535 - 45` degrees Celsius. Humidity is the
/// truncating integer `raw_rh * 100 / 65535` percent.
pub(crate) fn decode(buf: &[u8; 6], time: i32) -> Result<ProbeMeasure, ProbeError> {
    if crc8(&[buf[0], buf[1]]) != buf[2] || crc8(&[buf[3], buf[4]]) != buf[5] {
        return Err(ProbeError);
    }
    let raw_t = u16::from_be_bytes([buf[0], buf[1]]);
    let raw_rh = u16::from_be_bytes([buf[3], buf[4]]);
    let temp = f32::from(raw_t) * 175.0 / 65535.0 - 45.0;
    let hygr = u8::try_from((u32::from(raw_rh) * 100) / 65_535).unwrap_or(100);
    Ok(ProbeMeasure::new(time, temp, hygr))
}

/// CRC-8, initial value `0xFF`, polynomial `0x31`, high bit first.
fn crc8(data: &[u8; 2]) -> u8 {
    let mut crc = 0xFFu8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = crc.wrapping_shl(1) ^ 0x31;
            } else {
                crc = crc.wrapping_shl(1);
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_command_disables_clock_stretch() {
        assert_eq!(ADDRESS, 0x44);
        assert_eq!(MEASURE_HIGH_NO_STRETCH, [0x24, 0x00]);
        assert_eq!(MEASURE_WAIT_MS, 16);
    }

    #[test]
    fn sensirion_example_word_decodes() {
        // Datasheet CRC example: the word 0xBEEF covers to 0x92.
        let buf = [0xBE, 0xEF, 0x92, 0xBE, 0xEF, 0x92];
        let sample = decode(&buf, 7).unwrap();
        assert_eq!(sample.time, 7);
        assert!((sample.temp - 85.523).abs() < 1.0e-2);
        assert_eq!(sample.hygr, 74);
    }

    #[test]
    fn raw_ends_of_scale() {
        let zero = [0x00, 0x00, 0x81, 0x00, 0x00, 0x81];
        let full = [0xFF, 0xFF, 0xAC, 0xFF, 0xFF, 0xAC];

        let cold = decode(&zero, 1).unwrap();
        assert_eq!(cold.temp, -45.0);
        assert_eq!(cold.hygr, 0);

        let hot = decode(&full, 2).unwrap();
        assert_eq!(hot.temp, 130.0);
        assert_eq!(hot.hygr, 100);
    }

    #[test]
    fn either_bad_crc_rejects_the_sample() {
        let mut buf = [0xBE, 0xEF, 0x92, 0xBE, 0xEF, 0x92];
        buf[2] = 0x00;
        assert!(decode(&buf, 0).is_err());

        buf[2] = 0x92;
        buf[5] = 0x00;
        assert!(decode(&buf, 0).is_err());
    }
}
