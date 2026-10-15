use std::time::SystemTime;

#[derive(Clone, Debug, Default)]
pub struct UsageSection {
    /// Distinguishes an observed zero-usage window from a missing window.
    pub available: bool,
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
    /// Explicit provider-reported quota window length; `None` means unknown.
    pub window_seconds: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct UsageData {
    pub session: UsageSection,
    pub weekly: UsageSection,
}

#[derive(Clone, Debug, Default)]
pub struct AppUsageData {
    pub claude_code: Option<UsageData>,
    pub codex: Option<UsageData>,
    pub antigravity: Option<UsageData>,
}
