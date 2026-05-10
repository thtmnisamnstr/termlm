#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerformanceProfile {
    Eco,
    Balanced,
    Performance,
}

impl PerformanceProfile {
    pub fn from_str(s: &str) -> Self {
        match s {
            "eco" => Self::Eco,
            "balanced" => Self::Balanced,
            _ => Self::Performance,
        }
    }
}
