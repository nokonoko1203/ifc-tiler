//! Conversion from project units to SI units.
//!
//! IFC property values and quantities are written in the units declared by the file (e.g. millimetres).
//! The 3D Tiles columns use SI units (m, m², m³, kg), so the kind of quantity is determined from the value type
//! and multiplied by the project unit's conversion factor.

/// Kinds of quantities converted to SI units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Quantity {
    Length,
    Area,
    Volume,
    Mass,
}

impl Quantity {
    /// Determines the kind of quantity from a property value type (e.g. `IFCLENGTHMEASURE`).
    pub fn of_measure(value_type: &str) -> Option<Self> {
        let t = value_type.to_ascii_uppercase();
        match t.strip_prefix("IFC").unwrap_or(&t) {
            "LENGTHMEASURE" | "POSITIVELENGTHMEASURE" | "NONNEGATIVELENGTHMEASURE" => Some(Self::Length),
            "AREAMEASURE" => Some(Self::Area),
            "VOLUMEMEASURE" => Some(Self::Volume),
            "MASSMEASURE" => Some(Self::Mass),
            _ => None,
        }
    }

    /// Determines the kind of quantity from a quantity type name (e.g. `IfcQuantityLength`).
    pub fn of_quantity_kind(kind: &str) -> Option<Self> {
        match kind {
            "Length" => Some(Self::Length),
            "Area" => Some(Self::Area),
            "Volume" => Some(Self::Volume),
            "Weight" => Some(Self::Mass),
            _ => None,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Length => "m",
            Self::Area => "m2",
            Self::Volume => "m3",
            Self::Mass => "kg",
        }
    }
}

/// Multiplying a value in project units by this factor gives SI units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitScales {
    pub length: f64,
    pub area: f64,
    pub volume: f64,
    pub mass: f64,
}

impl Default for UnitScales {
    fn default() -> Self {
        Self { length: 1.0, area: 1.0, volume: 1.0, mass: 1.0 }
    }
}

impl UnitScales {
    pub fn si(&self, q: Quantity) -> f64 {
        match q {
            Quantity::Length => self.length,
            Quantity::Area => self.area,
            Quantity::Volume => self.volume,
            Quantity::Mass => self.mass,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_types_map_to_quantities() {
        assert_eq!(Quantity::of_measure("IFCPOSITIVELENGTHMEASURE"), Some(Quantity::Length));
        assert_eq!(Quantity::of_measure("IfcAreaMeasure"), Some(Quantity::Area));
        assert_eq!(Quantity::of_measure("IFCVOLUMEMEASURE"), Some(Quantity::Volume));
        assert_eq!(Quantity::of_measure("IFCMASSMEASURE"), Some(Quantity::Mass));
        // Ratios, thermal transmittance and strings are not converted
        assert_eq!(Quantity::of_measure("IFCPOSITIVERATIOMEASURE"), None);
        assert_eq!(Quantity::of_measure("IFCTHERMALTRANSMITTANCEMEASURE"), None);
        assert_eq!(Quantity::of_measure("IFCLABEL"), None);
    }

    #[test]
    fn quantity_kinds_map_to_quantities() {
        assert_eq!(Quantity::of_quantity_kind("Weight"), Some(Quantity::Mass));
        assert_eq!(Quantity::of_quantity_kind("Count"), None);
        assert_eq!(Quantity::of_quantity_kind("Time"), None);
    }

    #[test]
    fn scale_lookup() {
        let s = UnitScales { length: 1e-3, area: 1e-6, volume: 1e-9, mass: 1.0 };
        assert_eq!(s.si(Quantity::Area), 1e-6);
    }
}
