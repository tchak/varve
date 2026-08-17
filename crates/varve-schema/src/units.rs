//! Units on numbers (§2.14): a closed kernel set with exact integer
//! factors to each dimension's base unit. Ratios are cast-table
//! semantics — never user data.

/// Calendar time is deliberately its own dimension: days ↔ months has
/// no exact ratio, and the kernel refuses the conversion rather than
/// invent a 30.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dimension {
    Length,
    Mass,
    DurationExact,
    DurationCalendar,
    Area,
    Volume,
    Percent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    Millimetre,
    Centimetre,
    Metre,
    Kilometre,
    Gram,
    Kilogram,
    Tonne,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
    SquareMetre,
    Hectare,
    SquareKilometre,
    Litre,
    CubicMetre,
    Percent,
}

impl Unit {
    pub fn dimension(self) -> Dimension {
        use Unit::*;
        match self {
            Millimetre | Centimetre | Metre | Kilometre => Dimension::Length,
            Gram | Kilogram | Tonne => Dimension::Mass,
            Minute | Hour | Day | Week => Dimension::DurationExact,
            Month | Year => Dimension::DurationCalendar,
            SquareMetre | Hectare | SquareKilometre => Dimension::Area,
            Litre | CubicMetre => Dimension::Volume,
            Percent => Dimension::Percent,
        }
    }

    /// Exact factor to the dimension's base unit (mm, g, minute, month,
    /// m², L, %).
    pub fn factor(self) -> u64 {
        use Unit::*;
        match self {
            Millimetre | Gram | Minute | Month | SquareMetre | Litre | Percent => 1,
            Centimetre => 10,
            Metre | Kilogram => 1_000,
            Kilometre | Tonne => 1_000_000,
            Hour => 60,
            Day => 1_440,
            Week => 10_080,
            Year => 12,
            Hectare => 10_000,
            SquareKilometre => 1_000_000,
            CubicMetre => 1_000,
        }
    }
}

/// The exact conversion `value_in_from × num ⁄ den = value_in_to`, or
/// `None` across dimensions.
pub fn conversion(from: Unit, to: Unit) -> Option<(u64, u64)> {
    (from.dimension() == to.dimension()).then(|| (from.factor(), to.factor()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(conversion(Unit::Metre, Unit::Kilometre), Some((1_000, 1_000_000)));
        assert_eq!(conversion(Unit::Hour, Unit::Minute), Some((60, 1)));
        assert_eq!(conversion(Unit::Year, Unit::Month), Some((12, 1)));
        // Days ↔ months: refused, not fictionalized.
        assert_eq!(conversion(Unit::Day, Unit::Month), None);
        assert_eq!(conversion(Unit::Percent, Unit::Percent), Some((1, 1)));
    }
}
