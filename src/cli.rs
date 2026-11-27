//! `stars trade` CLI subcommand (manual dry-run / live rebalance from shell).

use crate::store::Store;
use crate::{exchange, live, trader};
use anyhow::Context;

/// `stars trade [--live] [--positions N] [--leverage X] [--slippage 0.005]`
///
/// Defaults to a dry run that prints exactly what would be sent. Live trading
/// requires `--live` plus HL_ACCOUNT_ADDRESS and HL_API_WALLET_KEY_FILE.
pub async fn run_trade(args: &[String], db_path: &str) -> anyhow::Result<()> {
    let flag = |name: &str| args.iter().position(|a| a == name);
    let value = |name: &str| -> Option<String> {
        flag(name).and_then(|i| args.get(i + 1)).cloned()
    };
    let live = flag("--live").is_some();

    let mut cfg = trader::TradeConfig::default();
    if let Some(v) = value("--positions") {
        // 与网页端同一区间：仓位数过大时 k 会超过半宇宙，重叠区被判成多头，
        // 组合会变成净多头。
        cfg.target_positions = v.parse::<usize>().context("--positions")?.clamp(1, 30);
    }
    if let Some(v) = value("--leverage") {
        cfg.leverage = v.parse().context("--leverage")?;
    }
    if let Some(v) = value("--lookback") {
        cfg.lookback = v.parse().context("--lookback")?;
    }
    if let Some(v) = value("--top") {
        cfg.top_frac = v.parse::<f64>().context("--top")?.clamp(0.02, 0.5);
    }
    if let Some(v) = value("--minvol") {
        cfg.min_vol_usd = v.parse().context("--minvol")?;
    }
    if let Some(v) = value("--slippage") {
        cfg.slippage = v.parse().context("--slippage")?;
    }
    if let Some(v) = value("--buffer") {
        cfg.margin_buffer = v.parse().context("--buffer")?;
    }

    let store = Store::open(std::path::Path::new(db_path))?;
    let panel = trader::load_panel(&store)?;
    anyhow::ensure!(
        panel.len() >= 20,
        "本地 K 线缓存不足（{} 币），先启动一次服务完成回填",
        panel.len()
    );

    let equity_hint: f64 = std::env::var("HL_ACCOUNT_ADDRESS")
        .ok()
        .and_then(|_| None)
        .unwrap_or_else(|| {
            std::env::args()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|w| w[0] == "--capital")
                .and_then(|w| w[1].parse().ok())
                .unwrap_or(500.0)
        });
    let (weights, liquid) = trader::target_weights(&panel, &cfg, equity_hint, i64::MAX);
    anyhow::ensure!(!weights.is_empty(), "没有选出候选（流动性过滤后为空）");
    println!(
        "流动宇宙 {} 币 · 多头腿 {} · 空头腿 {}",
        liquid.len(),
        weights.iter().filter(|(_, w)| *w > 0.0).count(),
        weights.iter().filter(|(_, w)| *w < 0.0).count()
    );

    // Account + markets (signing client only when live).
    let addr_env = std::env::var("HL_ACCOUNT_ADDRESS").ok();
    let exec = match (&live, &addr_env) {
        (true, Some(addr)) => {
            let key = std::env::var("HL_API_WALLET_KEY_FILE")
                .context("live 交易需要 HL_API_WALLET_KEY_FILE")?;
            let vault = std::env::var("HL_VAULT_ADDRESS").ok();
            exchange::Exec::signer(addr, std::path::Path::new(&key), vault.as_deref()).await?
        }
        // Dry run still reads the real account (read-only) so the plan matches.
        (false, Some(addr)) => exchange::Exec::reader_for(Some(addr)).await?,
        (false, None) => exchange::Exec::reader().await?,
        (true, None) => anyhow::bail!("live 交易需要 HL_ACCOUNT_ADDRESS"),
    };
    let markets = exec.markets().await?;

    let acct = if addr_env.is_some() {
        let a = exec.account().await?;
        println!("账户净值 ${:.2} · 现有持仓 {} 个", a.equity, a.positions.len());
        if !a.positions.is_empty() {
            println!("现有仓位：");
            let mut list: Vec<_> = a.positions.iter().collect();
            list.sort_by(|x, y| x.0.cmp(y.0));
            for (coin, pos) in list {
                println!("  {coin}: {:.6}", pos.size);
            }
            if live {
                anyhow::ensure!(
                    flag("--close-existing").is_some(),
                    "账户已有仓位。本程序会把不在目标名单里的仓位全部平掉（reduce-only）。\
                     确认要接管请加 --close-existing；若是别的机器人开的仓，先停掉它并手动清空。"
                );
            } else {
                println!("（dry-run 只展示，不会平掉这些仓位）");
            }
        }
        a
    } else {
        // Dry run without an account: assume the configured capital.
        let equity: f64 = value("--capital")
            .map(|v| v.parse().unwrap_or(2000.0))
            .unwrap_or(2000.0);
        println!("[DRY-RUN] 未读取账户，按 --capital ${equity:.0} 计算（真实运行时用账户净值）");
        exchange::Acct {
            equity,
            positions: Default::default(),
        }
    };

    // Mids for every coin we might touch.
    let mut coins: Vec<String> = weights.iter().map(|(c, _)| c.clone()).collect();
    coins.extend(acct.positions.keys().cloned());
    coins.sort();
    coins.dedup();
    let mids = trader::fetch_mids(&exec, &coins).await;

    let plan = trader::build_plan(&weights, &acct, &markets, &mids, &cfg, None);
    // 调仓前撤掉自己的旧止盈单（与网页端同一流程）；失败即中止。
    if live {
        match live::cancel_our_take_profits(&exec).await {
            Ok(n) if n > 0 => println!("撤销旧止盈单 {n} 个"),
            Ok(_) => {}
            Err(e) => return Err(e.context("撤销旧止盈单失败，已在调仓前中止")),
        }
    }
    println!(
        "\n目标：每腿 {} 仓 · 每仓 ${:.2} · 目标总名义 ${:.0} · 杠杆 {}x · 保证金缓冲 {:.0}%",
        weights.iter().filter(|(_, w)| *w > 0.0).count(),
        plan.per_coin,
        plan.gross_notional,
        cfg.leverage,
        cfg.margin_buffer * 100.0
    );
    println!("多头腿: {}", plan.long_leg.join(" "));
    println!("空头腿: {}", plan.short_leg.join(" "));
    println!("账户净值 ${:.2}", plan.equity);
    if plan.orders.is_empty() {
        println!("无需调仓（已在目标状态）。");
    } else {
        println!("\n计划下单 {} 笔：", plan.orders.len());
        for o in &plan.orders {
            println!(
                "  {} {} {:.6} @≈{:.6} (${:.2}) · {}",
                if o.buy { "买入" } else { "卖出" },
                o.coin,
                o.size,
                o.mid,
                o.notional,
                o.reason
            );
        }
    }
    for n in &plan.notes {
        println!("  注意: {n}");
    }

    if !live {
        println!("\n[DRY-RUN] 未发送任何订单。确认无误后加 --live 执行。");
        return Ok(());
    }

    println!("\n发送订单…");
    let outcome = trader::execute(&exec, &plan, &cfg, &markets, true).await?;
    for line in &outcome.prelim {
        println!("  注意: {line}");
    }
    for line in &outcome.orders {
        if !line.is_empty() {
            println!("  {line}");
        }
    }
    // 调仓后按「本次调仓的参考价」挂止盈单，与网页端同一流程。
    // 止盈幅度取持久化的实盘配置，保证 CLI 与网页端一致
    let live_path = std::env::var("STARS_LIVE").unwrap_or_else(|_| "live.json".into());
    let tp_pct = crate::live::LiveState::load(std::path::Path::new(&live_path))
        .config
        .take_profit_pct;
    let tp_ref: std::collections::HashMap<String, f64> = plan
        .long_leg
        .iter()
        .chain(plan.short_leg.iter())
        .filter_map(|c| mids.get(c).map(|m| (c.clone(), *m)))
        .collect();
    match live::place_take_profits(&exec, &markets, tp_pct, &tp_ref).await {
        Ok((log, _oids)) => {
            for l in log {
                println!("  {l}");
            }
        }
        Err(e) => println!("  止盈单挂单失败: {e}"),
    }
    if let Ok(a) = exec.account().await {
        println!(
            "\n执行后：净值 ${:.2} · 持仓 {} 个",
            a.equity,
            a.positions.len()
        );
    }
    Ok(())
}



