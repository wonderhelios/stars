use std::collections::HashMap;
#[derive(Clone,Debug)] pub struct MarketInfo{pub sz_decimals:i32,pub max_leverage:u32}
#[derive(Clone,Debug,Default)] pub struct Pos{pub size:f64,pub leverage:u32,pub is_cross:bool}
#[derive(Clone,Debug,Default)] pub struct Acct{pub equity:f64,pub positions:HashMap<String,Pos>}
pub struct OpenOrder{pub coin:String,pub sz:f64,pub px:f64,pub side:String,pub reduce_only:bool}
pub fn round_size(size: f64, sz_decimals: i32) -> f64 {
    if !size.is_finite() || size <= 0.0 {
        return 0.0;
    }
    if sz_decimals < 0 {
        // Integer lot sizes must use an integer step, not its inexact reciprocal.
        // 593 / 0.00001 becomes 59299999.99999999 and violates PEPE lot sizing.
        let step = 10_f64.powi(-sz_decimals);
        return (size / step).floor() * step;
    }
    let factor = 10_f64.powi(sz_decimals);
    (size * factor).floor() / factor
}

