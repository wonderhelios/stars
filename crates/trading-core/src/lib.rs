pub mod policy;
pub mod replay;
pub mod strategy;
pub const EXECUTION_MODEL: &str = "hyper-fly-core-v2";

/// Both applications expose the digest of the actual rule source, not just a label.
pub fn rule_hash() -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(include_str!("policy.rs"));
    h.update(include_str!("strategy.rs"));
    h.update(include_str!("replay.rs"));
    format!("{:x}", h.finalize())
}
pub fn strategy_hash(strategy: &strategy::StrategyConfig) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let mut clean = strategy.clone();
    clean.research_evidence = None;
    Ok(format!(
        "{:x}",
        Sha256::digest(clean.canonical_json()?.as_bytes())
    ))
}
