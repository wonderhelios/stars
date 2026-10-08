use crate::hl::Candle;

/// One coin's daily candles, used by every engine in the project.
#[derive(Clone, Debug, Default)]
pub struct PanelEntry {
    pub coin: String,
    pub candles: Vec<Candle>,
}
