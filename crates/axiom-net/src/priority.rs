#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RequestPriority {
    VeryHigh = 0,
    High = 1,
    #[default]
    Medium = 2,
    Low = 3,
    VeryLow = 4,
}

impl RequestPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VeryHigh => "very-high",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::VeryLow => "very-low",
        }
    }
}
