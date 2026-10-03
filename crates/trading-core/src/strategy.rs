use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{env, fs, path::Path};

pub const STRATEGY_ENV: &str = "HYPER_FLY_STRATEGY_FILE";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StrategyLibrary {
    #[serde(default = "library_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub strategies: Vec<StrategyConfig>,
}

fn library_schema_version() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StrategyConfig {
    pub schema_version: u32,
    pub strategy_id: String,
    pub name: String,
    pub venue: String,
    pub dex_scope: String,
    pub signal_kinds: Vec<String>,
    pub min_funding_8h: f64,
    pub min_volume_usd: f64,
    pub hold_hours: u32,
    pub stop_loss_pct: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_profit_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_profit_price_pct: Option<f64>,
    pub max_positions: usize,
    pub cooldown_hours: u32,
    pub margin_fraction: f64,
    pub allowed_leverages: Vec<u32>,
    pub max_five_x_positions: usize,
    pub max_three_x_positions: usize,
    #[serde(default)]
    pub research_evidence: Option<serde_json::Value>,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            strategy_id: "hyper-main-up-positive-v1".into(),
            name: "Hyperliquid 主 DEX 上涨 + 正费率".into(),
            venue: "hyperliquid".into(),
            dex_scope: "main".into(),
            signal_kinds: vec!["up_pos_fund".into()],
            min_funding_8h: 0.0005,
            min_volume_usd: 500_000.0,
            hold_hours: 24,
            stop_loss_pct: 2.0,
            take_profit_usd: None,
            take_profit_price_pct: None,
            max_positions: 5,
            cooldown_hours: 24,
            margin_fraction: 0.20,
            allowed_leverages: vec![10, 5, 3],
            max_five_x_positions: 1,
            max_three_x_positions: 1,
            research_evidence: None,
        }
    }
}

impl StrategyConfig {
    pub fn includes_coin(&self, coin: &str) -> bool {
        match self.dex_scope.as_str() {
            "main" => !coin.contains(':'),
            "all" => true,
            dex => coin
                .split_once(':')
                .is_some_and(|(prefix, _)| prefix == dex),
        }
    }

    pub fn validate_deployment(&self) -> Result<()> {
        self.validate()?;
        let evidence = self
            .research_evidence
            .as_ref()
            .context("缺少执行回放及影子验证证据")?;
        anyhow::ensure!(
            evidence["execution_model"] == crate::EXECUTION_MODEL,
            "执行模型版本不同，重新回放"
        );
        anyhow::ensure!(
            evidence["rule_hash"] == crate::rule_hash(),
            "研究与实盘规则源码不同，请重新回放"
        );
        anyhow::ensure!(
            evidence["strategy_hash"] == crate::strategy_hash(self)?,
            "策略参数已变化，请重新回放"
        );
        anyhow::ensure!(evidence["deployable"] == true, "研究候选尚未通过执行回放");
        Ok(())
    }

    pub fn load_from_env() -> Result<Self> {
        match env::var(STRATEGY_ENV) {
            Ok(path) if !path.trim().is_empty() => Self::load(Path::new(&path)),
            _ => {
                let config = Self::default();
                config.validate()?;
                Ok(config)
            }
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read strategy file {}", path.display()))?;
        Self::from_json(&raw).with_context(|| format!("parse strategy file {}", path.display()))
    }

    pub fn from_json(raw: &str) -> Result<Self> {
        let config: Self = serde_json::from_str(raw).context("parse strategy JSON")?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == 1,
            "unsupported strategy schema_version"
        );
        anyhow::ensure!(
            !self.strategy_id.trim().is_empty(),
            "strategy_id is required"
        );
        anyhow::ensure!(
            self.venue == "hyperliquid",
            "only hyperliquid strategies are supported"
        );
        anyhow::ensure!(
            ["main", "all", "xyz", "para"].contains(&self.dex_scope.as_str()),
            "supported dex_scope: main, all, xyz, para"
        );
        anyhow::ensure!(
            !self.signal_kinds.is_empty(),
            "signal_kinds cannot be empty"
        );
        for kind in &self.signal_kinds {
            anyhow::ensure!(
                [
                    "pump_pos_fund",
                    "up_pos_fund",
                    "down_pos_fund",
                    "crash_pos_fund"
                ]
                .contains(&kind.as_str()),
                "unsupported signal kind {kind}"
            );
        }
        anyhow::ensure!(
            self.min_funding_8h.is_finite() && self.min_funding_8h >= 0.0,
            "invalid funding threshold"
        );
        anyhow::ensure!(
            self.min_volume_usd.is_finite() && self.min_volume_usd >= 0.0,
            "invalid volume threshold"
        );
        anyhow::ensure!(
            (1..=168).contains(&self.hold_hours),
            "hold_hours must be 1..=168"
        );
        anyhow::ensure!(
            self.stop_loss_pct.is_finite() && (0.1..=20.0).contains(&self.stop_loss_pct),
            "stop_loss_pct must be 0.1..=20"
        );
        anyhow::ensure!(
            self.take_profit_usd.is_none() || self.take_profit_price_pct.is_none(),
            "choose amount or price percentage take profit, not both"
        );
        for (value, max, name) in [
            (self.take_profit_usd, 1_000_000.0, "take_profit_usd"),
            (self.take_profit_price_pct, 50.0, "take_profit_price_pct"),
        ] {
            if let Some(v) = value {
                anyhow::ensure!(
                    v.is_finite()
                        && (0.01..=max).contains(&v)
                        && (v * 100.0 - (v * 100.0).round()).abs() < 1e-6,
                    "{name} must be positive, at most {max}, and use at most 2 decimals"
                );
            }
        }
        anyhow::ensure!(
            (1..=20).contains(&self.max_positions),
            "max_positions must be 1..=20"
        );
        anyhow::ensure!(
            (1..=168).contains(&self.cooldown_hours),
            "cooldown_hours must be 1..=168"
        );
        anyhow::ensure!(
            self.margin_fraction.is_finite()
                && self.margin_fraction > 0.0
                && self.margin_fraction <= 0.20,
            "margin_fraction must be >0 and <=0.20"
        );
        anyhow::ensure!(
            !self.allowed_leverages.is_empty(),
            "allowed_leverages cannot be empty"
        );
        anyhow::ensure!(
            self.allowed_leverages
                .iter()
                .all(|x| [3, 5, 10].contains(x)),
            "allowed_leverages may only contain 3, 5, 10"
        );
        anyhow::ensure!(
            self.max_positions as f64 * self.margin_fraction <= 1.0 + f64::EPSILON,
            "position limit exceeds available margin"
        );
        anyhow::ensure!(
            self.max_five_x_positions <= 1 && self.max_three_x_positions <= 1,
            "current executor supports at most one 5x slot and one 3x slot"
        );
        Ok(())
    }

    pub fn canonical_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn kind_for_return(prior_pct: f64) -> Option<&'static str> {
        if prior_pct < -10.0 {
            Some("crash_pos_fund")
        } else if prior_pct < -3.0 {
            Some("down_pos_fund")
        } else if (3.0..10.0).contains(&prior_pct) {
            Some("up_pos_fund")
        } else if prior_pct >= 10.0 {
            Some("pump_pos_fund")
        } else {
            None
        }
    }
}

impl StrategyLibrary {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                schema_version: 1,
                strategies: Vec::new(),
            });
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read strategy library {}", path.display()))?;
        let library: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parse strategy library {}", path.display()))?;
        anyhow::ensure!(
            library.schema_version == 1,
            "unsupported strategy library schema"
        );
        for strategy in &library.strategies {
            strategy.validate()?;
        }
        Ok(library)
    }

    pub fn upsert(path: &Path, strategy: StrategyConfig) -> Result<Self> {
        strategy.validate()?;
        let mut library = Self::load(path)?;
        if let Some(existing) = library
            .strategies
            .iter_mut()
            .find(|item| item.strategy_id == strategy.strategy_id)
        {
            *existing = strategy;
        } else {
            library.strategies.push(strategy);
        }
        library.strategies.sort_by(|a, b| a.name.cmp(&b.name));
        anyhow::ensure!(library.strategies.len() <= 100, "strategy library is full");
        let parent = path
            .parent()
            .context("strategy library has no parent directory")?;
        fs::create_dir_all(parent)?;
        let temp = path.with_extension("json.tmp");
        fs::write(
            &temp,
            format!("{}\n", serde_json::to_string_pretty(&library)?),
        )?;
        fs::rename(&temp, path)?;
        Ok(library)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_profit_validation_and_hash_bind_both_modes() {
        let base = StrategyConfig::default();
        let legacy = base.canonical_json().unwrap();
        assert!(!legacy.contains("take_profit"));
        assert_eq!(StrategyConfig::from_json(&legacy).unwrap(), base);
        let mut s = base.clone();
        s.take_profit_usd = Some(40.0);
        s.validate().unwrap();
        assert_ne!(
            crate::strategy_hash(&base).unwrap(),
            crate::strategy_hash(&s).unwrap()
        );
        s.take_profit_price_pct = Some(3.0);
        assert!(s.validate().is_err());
        s.take_profit_usd = None;
        s.validate().unwrap();
        for value in [0.0, -1.0, f64::NAN, 50.01, 1.001] {
            s.take_profit_price_pct = Some(value);
            assert!(s.validate().is_err());
        }
    }
    #[test]
    fn default_matches_current_live_signal() {
        let config = StrategyConfig::default();
        config.validate().unwrap();
        assert_eq!(config.signal_kinds, ["up_pos_fund"]);
        assert_eq!(config.hold_hours, 24);
        assert_eq!(config.stop_loss_pct, 2.0);
        assert_eq!(config.max_positions, 5);
    }

    #[test]
    fn refuses_research_config_that_can_overcommit_margin() {
        let config = StrategyConfig {
            max_positions: 6,
            ..StrategyConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn strategy_library_upserts_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("strategy-library.json");
        let first = StrategyConfig::default();
        StrategyLibrary::upsert(&path, first.clone()).unwrap();
        let changed = StrategyConfig {
            name: "更新后的名称".into(),
            ..first
        };
        let library = StrategyLibrary::upsert(&path, changed.clone()).unwrap();
        assert_eq!(library.strategies, [changed]);
        assert_eq!(StrategyLibrary::load(&path).unwrap(), library);
    }
}
