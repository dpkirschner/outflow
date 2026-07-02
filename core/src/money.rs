use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Money(i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyParseError {
    Empty,
    Invalid,
    Overflow,
}

impl Money {
    pub fn from_cents(c: i64) -> Self {
        Money(c)
    }

    pub fn cents(self) -> i64 {
        self.0
    }

    pub fn abs(self) -> Money {
        Money(self.0.abs())
    }

    pub fn is_outflow(self) -> bool {
        self.0 < 0
    }

    pub fn from_decimal_str(s: &str) -> Result<Self, MoneyParseError> {
        let t = s.trim();
        if t.is_empty() {
            return Err(MoneyParseError::Empty);
        }
        let (neg, rest) = match t.as_bytes()[0] {
            b'-' => (true, &t[1..]),
            b'+' => (false, &t[1..]),
            _ => (false, t),
        };
        let mut parts = rest.splitn(2, '.');
        let int_part = parts.next().unwrap_or("");
        let frac_part = parts.next().unwrap_or("");
        if int_part.is_empty() && frac_part.is_empty() {
            return Err(MoneyParseError::Invalid);
        }
        if !int_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(MoneyParseError::Invalid);
        }
        if !frac_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(MoneyParseError::Invalid);
        }
        let whole: i64 = if int_part.is_empty() {
            0
        } else {
            int_part.parse().map_err(|_| MoneyParseError::Overflow)?
        };
        let mut cents = whole.checked_mul(100).ok_or(MoneyParseError::Overflow)?;
        let fb = frac_part.as_bytes();
        let d0 = if !fb.is_empty() { (fb[0] - b'0') as i64 } else { 0 };
        let d1 = if fb.len() > 1 { (fb[1] - b'0') as i64 } else { 0 };
        let d2 = if fb.len() > 2 { (fb[2] - b'0') as i64 } else { 0 };
        let mut frac = d0 * 10 + d1;
        if d2 >= 5 {
            frac += 1;
        }
        cents = cents.checked_add(frac).ok_or(MoneyParseError::Overflow)?;
        Ok(Money(if neg { -cents } else { cents }))
    }

    pub fn to_display(self) -> String {
        let neg = self.0 < 0;
        let a = self.0.abs();
        format!("{}{}.{:02}", if neg { "-" } else { "" }, a / 100, a % 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_negative_decimal() {
        assert_eq!(Money::from_decimal_str("-33.72").unwrap().cents(), -3372);
    }

    #[test]
    fn parses_integer_only() {
        assert_eq!(Money::from_decimal_str("12").unwrap().cents(), 1200);
    }

    #[test]
    fn parses_single_decimal_place() {
        assert_eq!(Money::from_decimal_str("0.1").unwrap().cents(), 10);
    }

    #[test]
    fn parses_leading_dot() {
        assert_eq!(Money::from_decimal_str(".50").unwrap().cents(), 50);
    }

    #[test]
    fn rounds_third_decimal_place() {
        assert_eq!(Money::from_decimal_str("5.005").unwrap().cents(), 501);
        assert_eq!(Money::from_decimal_str("5.004").unwrap().cents(), 500);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Money::from_decimal_str("abc").is_err());
        assert!(Money::from_decimal_str("").is_err());
    }

    #[test]
    fn display_roundtrips() {
        assert_eq!(Money::from_cents(-3372).to_display(), "-33.72");
        assert_eq!(Money::from_cents(5).to_display(), "0.05");
    }
}
