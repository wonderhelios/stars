# stars 部署说明

`stars` 是一个单二进制 Rust 服务：从 Hyperliquid 官方 API 拉取全宇宙日线数据，运行横截面动量回测，并提供纸交易引擎验证执行成本。

- 默认监听 `0.0.0.0:3000`
- 数据缓存在 SQLite（`STARS_DB`），纸交易状态存 JSON（`STARS_PAPER`）
- 行情需要出网访问 `https://api.hyperliquid.xyz/info` 和 `https://api.txflow.com/info`；实盘交易另需已授权的 API / Agent 钱包密钥文件

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

## 更新到 TxFlow 页面

沿用现有服务和 Nginx 配置：

```bash
cd /opt/stars
git pull --ff-only origin main
cargo build --release --locked
sudo systemctl restart stars
sudo systemctl status stars --no-pager
curl --fail --silent --show-error http://127.0.0.1:3000/api/health
curl --fail --silent --show-error http://127.0.0.1:3000/api/txflow/status
```

访问原站点的 `/txflow`。首次启动会后台回填 TxFlow 日线，可在页面状态栏或 `/api/txflow/status` 查看进度。

默认新增三个独立持久文件，与原 Hyperliquid 数据文件位于同一目录，无需迁移原数据：

- `txflow-candles.sqlite`：日线缓存。
- `txflow-live.json`：配置、交易记录与净值。
- `txflow-orders.json`：本策略止盈订单归属；不要删除。

可通过 `STARS_TXFLOW_DB`、`STARS_TXFLOW_LIVE`、`STARS_TXFLOW_ORDERS` 指定文件路径；实际运行服务的用户需有对应目录的写权限。

实盘默认关闭。在装有钱包扩展的浏览器打开 `/txflow` →「实盘设置」→「连接钱包并生成 Agent」→ 核对地址 →「签名授权 Agent」。主钱包只签名授权，Agent 密钥自动保存到服务器，授权成功后主账户和密钥路径会自动填写并保存。先生成计划核对，再由操作者启用实盘。所有 TxFlow 交易请求（包括授权）共用后端队列，间隔至少 300ms。

新增持久化目录 `/var/lib/stars/txflow-agents/`（默认位于 `STARS_TXFLOW_LIVE` 同目录），保存 Agent 密钥及授权回执。由服务运行用户创建，目录权限 `700`、文件权限 `600`，须备份且不能删除正在使用的密钥。无需在 systemd 中配置私钥。

授权接口校验同源浏览器请求。现有 Nginx 的 `location` 代理配置需要保留访问域名与端口：

```nginx
proxy_set_header Host $http_host;
```

若修改 Nginx 配置，执行 `sudo nginx -t && sudo systemctl reload nginx`。通过已有受保护的管理入口和 HTTPS 域名访问。没有检测到钱包时，请换用装有 MetaMask 等扩展的浏览器。


## 账户收益口径与币安信号刷新（2026-10-10）

账户卡片展示记录起点资金、当前交易所净值、两者差额和变化率：

- 资金增减 = 当前净值 − 起点净值；资金变化率 = 资金增减 / 起点净值。
- 净值变化包含手续费、资金费和出入金。有后续出入金时，资金变化不能直接当作策略净收益。交易所返回的成交/流水可能截断，不再用它反推起始资金或宣称“真实总盈亏”。
- 起点存入账户状态的 `capital_baseline`，绑定账户；重启、清理成交记录不改变它。新账户首次真实调仓前先保存净值；保存失败时不开始执行。
- 旧状态优先保留原 `start_equity`，缺失时使用最早留存的有效净值。两者都没有时，从首次成功读取开始记录，无法补造历史启动资金。旧 `start_equity` 没有可信时间戳，因此页面显示时间未知。

TxFlow 的默认 82 币映射来自已验证映射的币名和单位换算字段，保存在 `config/binance-mapping.json`，编译进二进制；部署不再依赖未跟踪的研究目录或可执行文件层级。默认可变 CSV 存在 `STARS_TXFLOW_LIVE` 同目录的 `binance-daily/`，仍可用 `STARS_BINANCE_MAPPING` 和 `STARS_BINANCE_DAILY` 显式覆盖。覆盖路径必须存在且服务用户可读写；公开行情需要能访问 `https://api.binance.com`。

手动、自动调仓共用同一日线检查：至少 20 币具有截至昨日 UTC 的足够连续日线。跨日立即重新检查，后台失败后 15 分钟重试。一次刷新最多 180 秒，已完成的文件会导入；下一次优先抓取缺失/过期币种。限流或区域访问失败会显示原因，不切换信号源或绕过 20 币门槛。仅行情检查失败不会写入“交易执行已开始”的恢复标记。

这些修改需要重新构建并部署服务才会影响线上页面。本地回归测试使用临时账户状态与模拟数据，不代表生产网络或真实调仓已验收。


### TxFlow 保证金预算报错

报错会列出预计占用、预算、超额、最终目标仓位占用（其中包括保留仓位）和挂单额外预留。不能仅凭这条报错断定交易所余额不足。

旧计划把全部可部署保证金分配到目标仓位，但错峰、调仓比例带和最小下单额会保留一些偏离目标的仓位，可能导致“目标在预算内、实际保留后的组合略超预算”。没有挂单、实际杠杆不低于配置且输入权重总绝对值不超过 1 时，规划器现在尝试递减目标规模：最多 64 次、目标不低于原计划 50%，找到首个通过全部风险检查的计划即停止。实际缩减比例显示在计划中。正常预算内的计划不变。

缩减仅改变本次目标名义金额，保留原错峰档位、绝对/比例下单门槛、配置杠杆和原保证金上限。每个候选都重新计算未调仓仓位，并检查每腿至少 3 币、单币/两腿/净敞口。出现其他风险错误即停止；有挂单或实际杠杆偏低时不自动缩减来掩盖问题。若仍无法形成可执行计划，保留详细报错并拒绝执行。
