# stars 部署说明

`stars` 是一个单二进制 Rust 服务：从 Hyperliquid 官方 API 拉取全宇宙日线数据，运行横截面动量回测，并提供纸交易引擎验证执行成本。

- 默认监听 `0.0.0.0:3000`
- 数据缓存在 SQLite（`STARS_DB`），纸交易状态存 JSON（`STARS_PAPER`）
- 行情需要出网访问 `https://api.hyperliquid.xyz/info`；实盘交易另需已授权的 API 钱包密钥文件

## 首次部署

```bash
cd /opt/stars
git pull --ff-only
cargo build --release --locked

# 数据目录（运行用户可写）
install -d -m 755 /var/lib/stars
```

systemd 服务（`/etc/systemd/system/stars.service`）：

```ini
[Unit]
Description=stars momentum alpha lab
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/stars/target/release/stars
Restart=always
RestartSec=5
Environment=STARS_DB=/var/lib/stars/candles.sqlite
Environment=STARS_PAPER=/var/lib/stars/paper.json
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

启用：

```bash
cp deploy/stars.service /etc/systemd/system/stars.service
systemctl daemon-reload
systemctl enable --now stars
systemctl status stars --no-pager
```

验证（首次启动后约 5~15 分钟回填完历史 K 线）：

```bash
curl -s http://127.0.0.1:3000/api/health
curl -s http://127.0.0.1:3000/api/status | head -c 500
```

公网访问沿用原有 Nginx（3000 端口）即可，无需额外配置。

## 更新

```bash
cd /opt/stars
git pull --ff-only
cargo build --release --locked
systemctl restart stars
```

## 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `STARS_DB` | `/var/lib/stars/candles.sqlite` | K 线缓存 |
| `STARS_PAPER` | `/var/lib/stars/paper.json` | 纸交易状态 |

## API

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/status` | 数据回填进度、流动性宇宙、纸交易快照 |
| POST | `/api/backtest` | 运行动量回测（参数见下） |
| GET | `/api/paper` | 纸交易状态 |
| POST | `/api/paper/start` | 启动纸交易 |
| POST | `/api/paper/stop` | 停止 |
| POST | `/api/paper/reset` | 重置 |
| POST | `/api/paper/step` | 手动步进一天 |

回测参数（JSON body）：

```json
{
  "lookback": 14,
  "top_frac": 0.2,
  "min_vol_usd": 5000000,
  "hedge": "equal_weight"
}
```

`hedge` 取值：`equal_weight`（等权对冲，纯 alpha）或 `long_short`（多空对冲）。

## 账户收益口径（2026-10-10）

账户卡片展示记录起点资金、当前交易所净值、两者差额和变化率：

- 资金增减 = 当前净值 − 起点净值；资金变化率 = 资金增减 / 起点净值。
- 净值变化包含手续费、资金费和出入金。有后续出入金时，资金变化不能直接当作策略净收益。交易所返回的成交/流水可能截断，不再用它反推起始资金或宣称“真实总盈亏”。
- 起点存入账户状态的 `capital_baseline`，绑定账户；重启、清理成交记录不改变它。新账户首次真实调仓前先保存净值；保存失败时不开始执行。
- 旧状态优先保留原 `start_equity`，缺失时使用最早留存的有效净值。两者都没有时，从首次成功读取开始记录，无法补造历史启动资金。旧 `start_equity` 没有可信时间戳，因此页面显示时间未知。
