//! Exact decimal money and quantity representation for ZoneBoard.
//!
//! Replaces binary floating point (`f64`) in all new arithmetic to guarantee exactness
//! for balances, fees, trade sizes, and prices.

use std::fmt;
use std::ops::{Add, AddAssign, Deref, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};
use std::str::FromStr;

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Exact decimal money and financial quantity type.
///
/// Wraps [`rust_decimal::Decimal`]. Always serialized as a string in JSON to preserve precision.
/// Deserializes transparently from JSON numbers (integers or shortest decimal floats) or JSON strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Money(pub Decimal);

impl Money {
    pub const ZERO: Money = Money(Decimal::ZERO);
    pub const ONE: Money = Money(Decimal::ONE);

    pub fn new(d: Decimal) -> Self {
        Self(d)
    }

    /// Rounds half-even (Banker's rounding) to `scale` decimal places.
    ///
    /// Ties round towards the nearest even number:
    /// - `2.5` to scale 0 -> `2`
    /// - `3.5` to scale 0 -> `4`
    /// - `2.55` to scale 1 -> `2.6`
    /// - `2.65` to scale 1 -> `2.6`
    pub fn round_half_even(&self, scale: u32) -> Self {
        Self(self.0.round_dp_with_strategy(scale, RoundingStrategy::MidpointNearestEven))
    }

    /// Converts an IEEE-754 float (`f64`) into [`Money`] using the number's shortest decimal text.
    ///
    /// # Conversion Rules
    /// 1. Non-finite values (`NaN`, `+Infinity`, `-Infinity`) return `None`.
    /// 2. Raw binary float conversion (which can turn `0.1` into `0.1000000000000000055511151231...`)
    ///    is deliberately **not** used. Instead, the float is converted to its shortest exact decimal
    ///    string (using the Ryu algorithm built into Rust's float formatting), and then parsed into [`Decimal`].
    /// 3. Scientific notation produced by tiny numbers (e.g. `1e-8` -> `0.00000001`) is correctly expanded.
    /// 4. This guarantees that numbers entered as `0.1`, `0.2`, `0.00000486`, or `123456789.12345678`
    ///    convert to their clean, expected decimal representations without binary floating point noise.
    pub fn from_legacy_f64(val: f64) -> Option<Self> {
        if !val.is_finite() {
            return None;
        }
        let s = val.to_string();
        Self::from_str(&s).ok()
    }
}

impl FromStr for Money {
    type Err = rust_decimal::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.contains('e') || trimmed.contains('E') {
            Decimal::from_scientific(trimmed).map(Money)
        } else {
            Decimal::from_str(trimmed).map(Money)
        }
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Deref for Money {
    type Target = Decimal;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Decimal> for Money {
    fn from(d: Decimal) -> Self {
        Self(d)
    }
}

impl From<i64> for Money {
    fn from(v: i64) -> Self {
        Self(Decimal::from(v))
    }
}

impl From<u64> for Money {
    fn from(v: u64) -> Self {
        Self(Decimal::from(v))
    }
}

impl From<i32> for Money {
    fn from(v: i32) -> Self {
        Self(Decimal::from(v))
    }
}

impl Add for Money {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Money {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sub for Money {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl SubAssign for Money {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl Mul for Money {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        Self(self.0 * rhs.0)
    }
}

impl MulAssign for Money {
    fn mul_assign(&mut self, rhs: Self) {
        self.0 *= rhs.0;
    }
}

impl Div for Money {
    type Output = Self;
    fn div(self, rhs: Self) -> Self::Output {
        Self(self.0 / rhs.0)
    }
}

impl DivAssign for Money {
    fn div_assign(&mut self, rhs: Self) {
        self.0 /= rhs.0;
    }
}

impl Neg for Money {
    type Output = Self;
    fn neg(self) -> Self::Output {
        Self(-self.0)
    }
}

impl Serialize for Money {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MoneyVisitor;

        impl<'de> de::Visitor<'de> for MoneyVisitor {
            type Value = Money;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a decimal string or number")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Money::from_str(v).map_err(de::Error::custom)
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&v)
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Money(Decimal::from(v)))
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Money(Decimal::from(v)))
            }

            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Money::from_legacy_f64(v)
                    .ok_or_else(|| de::Error::custom("non-finite float cannot be converted to Money"))
            }
        }

        deserializer.deserialize_any(MoneyVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_arithmetic_prevents_float_leakage() {
        let a = Money::from_str("0.1").unwrap();
        let b = Money::from_str("0.2").unwrap();
        let c = Money::from_str("0.3").unwrap();
        assert_eq!(a + b, c);
        assert_eq!((a + b).to_string(), "0.3");
    }

    #[test]
    fn bankers_rounding_half_even_table() {
        // Half-even table on ties:
        // 2.5 -> 2, 3.5 -> 4
        assert_eq!(Money::from_str("2.5").unwrap().round_half_even(0), Money::from(2));
        assert_eq!(Money::from_str("3.5").unwrap().round_half_even(0), Money::from(4));

        // 2.55 -> 2.6, 2.65 -> 2.6
        assert_eq!(Money::from_str("2.55").unwrap().round_half_even(1), Money::from_str("2.6").unwrap());
        assert_eq!(Money::from_str("2.65").unwrap().round_half_even(1), Money::from_str("2.6").unwrap());

        // Negative numbers:
        // -2.5 -> -2, -3.5 -> -4
        assert_eq!(Money::from_str("-2.5").unwrap().round_half_even(0), Money::from(-2));
        assert_eq!(Money::from_str("-3.5").unwrap().round_half_even(0), Money::from(-4));
    }

    #[test]
    fn from_legacy_f64_matches_shortest_text() {
        let cases = [
            (0.1f64, "0.1"),
            (0.2f64, "0.2"),
            (0.3f64, "0.3"),
            (0.00000486f64, "0.00000486"),
            (1e-8f64, "0.00000001"),
            (123456789.12345678f64, "123456789.12345678"),
        ];

        for (flt, expected_str) in cases {
            let m = Money::from_legacy_f64(flt).expect("finite conversion");
            assert_eq!(m.to_string(), expected_str, "float {flt} mismatch");
            let from_s = Money::from_str(expected_str).unwrap();
            assert_eq!(m, from_s);
        }

        assert!(Money::from_legacy_f64(f64::NAN).is_none());
        assert!(Money::from_legacy_f64(f64::INFINITY).is_none());
        assert!(Money::from_legacy_f64(f64::NEG_INFINITY).is_none());
    }

    #[test]
    fn serde_json_parsing_and_string_serialization() {
        // Serializes as JSON string
        let m = Money::from_str("123.456").unwrap();
        assert_eq!(serde_json::to_string(&m).unwrap(), "\"123.456\"");

        // Deserializes from JSON string
        let d1: Money = serde_json::from_str("\"123.456\"").unwrap();
        assert_eq!(d1, m);

        // Deserializes from JSON numbers (integers and floats)
        let d2: Money = serde_json::from_value(json!(0.1)).unwrap();
        assert_eq!(d2, Money::from_str("0.1").unwrap());

        let d3: Money = serde_json::from_value(json!(100)).unwrap();
        assert_eq!(d3, Money::from(100));

        let d4: Money = serde_json::from_value(json!(0.00000486)).unwrap();
        assert_eq!(d4, Money::from_str("0.00000486").unwrap());

        let d5: Money = serde_json::from_value(json!(1e-8)).unwrap();
        assert_eq!(d5, Money::from_str("0.00000001").unwrap());

        let d6: Money = serde_json::from_value(json!(123456789.12345678)).unwrap();
        assert_eq!(d6, Money::from_str("123456789.12345678").unwrap());
    }
}
