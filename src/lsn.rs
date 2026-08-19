use anyhow::Result;

use std::fmt::Display;

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub struct Lsn {
    value: u64,
}

impl Lsn {
    // Format of LSN is "X/Y" where X and Y are hexadecimal numbers
    // Both X and Y are 32 bits, so we can combine them into a single u64 for easier comparison and storage
    pub fn parse(s: &str) -> Result<Self> {
        let (high_str, low_str) = s
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("Invalid LSN format {s}: missing '/'"))?;

        let high = u32::from_str_radix(high_str, 16)
            .map_err(|e| anyhow::anyhow!("Invalid LSN high part '{high_str}': {e}"))?;
        let low = u32::from_str_radix(low_str, 16)
            .map_err(|e| anyhow::anyhow!("Invalid LSN low part '{low_str}': {e}"))?;

        Ok(Self {
            value: (u64::from(high) << 32) | u64::from(low),
        })
    }

    pub fn as_u64(self) -> u64 {
        self.value
    }

    pub fn from_u64(value: u64) -> Self {
        Self { value }
    }

    #[allow(dead_code)]
    pub fn bytes_since(self, other: Lsn) -> u64 {
        self.value.saturating_sub(other.value)
    }
}

impl Display for Lsn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let high = (self.value >> 32) as u32;
        #[allow(clippy::cast_possible_truncation)]
        let low = self.value as u32;
        write!(f, "{high:X}/{low:X}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let lsn = Lsn::parse("0/16B6310").unwrap();
        assert_eq!(format!("{lsn}"), "0/16B6310");
    }

    #[test]
    fn ordering() {
        let a = Lsn::parse("0/100").unwrap();
        let b = Lsn::parse("0/200").unwrap();
        let c = Lsn::parse("1/0").unwrap();
        assert!(a < b);
        assert!(b < c);
    }

    #[test]
    fn round_trip() {
        let cases = ["0/0", "0/FFFFFFFF", "1/0", "FF/FFFFFFFF"];
        for s in cases {
            let lsn = Lsn::parse(s).unwrap();
            assert_eq!(format!("{lsn}"), s);
        }
    }

    #[test]
    fn rejects_invalid() {
        assert!(Lsn::parse("not an lsn").is_err());
        assert!(Lsn::parse("0").is_err()); // no slash
        assert!(Lsn::parse("0/ZZZZ").is_err()); // not hex
    }
}
