use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum UsagePeriod {
    Day,
    Week,
    All,
}

/// Calls to one model for one purpose over a period.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    pub provider: String,
    pub model: String,
    /// `chat`, `handoff`, `embed_index` or `embed_query`.
    pub kind: String,
    pub calls: u32,
    pub errors: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cached_tokens: u32,
    pub avg_latency_ms: u32,
    /// Estimated, summed over the calls that had a known price.
    pub cost_usd: f64,
    /// Calls with no known price, so the cost above leaves them out.
    pub unpriced_calls: u32,
    /// Some token counts were guessed from text length.
    pub estimated: bool,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub rows: Vec<UsageRow>,
    /// Where the price list comes from and when it was copied.
    pub prices_source: String,
    pub prices_fetched: String,
    /// The weekly price download is on.
    pub prices_auto_update: bool,
}
