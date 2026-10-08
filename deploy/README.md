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
