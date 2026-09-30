//! プロジェクト単位からSI単位への換算。
//!
//! IFCのプロパティ値と数量は、ファイルが宣言した単位（ミリメートルなど）のまま書かれている。
//! 3D Tilesの列はSI単位（m、m²、m³、kg）にそろえるため、値の型から量の種類を判定し、
//! プロジェクト単位の換算係数を掛ける。

/// SI単位へ換算する量の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Quantity {
    Length,
    Area,
    Volume,
    Mass,
}

impl Quantity {
    /// プロパティの値の型（`IFCLENGTHMEASURE`など）から量の種類を判定する。
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

    /// 数量（`IfcQuantityLength`など）の種類名から判定する。
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

/// プロジェクト単位の値にこの係数を掛けるとSI単位になる。
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
        // 比率・熱貫流率・文字列は換算しない
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
