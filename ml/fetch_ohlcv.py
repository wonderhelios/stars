"""拉 Hyperliquid 日线 OHLCV → ml/data/hl2/。

增量合并已闭合日线。任何活跃币请求失败或缺少上一闭合日都会非零退出；
不得用旧缓存冒充更新成功，否则 daily 必须中止，不能继续生成调仓信号。
"""
from pathlib import Path
BASE = Path(__file__).resolve().parent
import json, urllib.request, urllib.error, time, datetime, os
from concurrent.futures import ThreadPoolExecutor, as_completed
from candle_rules import (
    DAY_MS, daily_request_end, merge_closed_daily_bars,
    require_latest_closed_daily_bar,
)

INFO_URL = "https://api.hyperliquid.xyz/info"
MAX_WORKERS = 4
MAX_RETRIES = 5


def post(url, body):
    req = urllib.request.Request(url, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    errors = []
    for attempt in range(MAX_RETRIES):
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                result = json.load(r)
            if result is None:
                raise RuntimeError("交易所返回 null")
            return result
        except Exception as e:
            errors.append("%s: %s" % (type(e).__name__, e))
            if attempt + 1 < MAX_RETRIES:
                wait = min(120.0, 5.0 * (2 ** attempt)) if isinstance(
                    e, urllib.error.HTTPError) and e.code == 429 else min(30.0, 2.0 ** attempt)
                if isinstance(e, urllib.error.HTTPError) and e.headers.get("Retry-After"):
                    try:
                        wait = max(wait, float(e.headers["Retry-After"]))
                    except ValueError:
                        pass
                print("[ohlcv] %s 请求失败，%.1f 秒后重试（%d/%d）" % (
                    body.get("req", {}).get("coin", body.get("type", "?")),
                    wait, attempt + 1, MAX_RETRIES), flush=True)
                time.sleep(wait)
    coin = body.get("req", {}).get("coin", body.get("type", "?"))
    raise RuntimeError("%s 请求连续失败 %d 次：%s" % (
        coin, MAX_RETRIES, "；".join(errors)))


def fetch_one(coin, out_dir, now_ms, full_start, require_fresh=True):
    path = Path(out_dir) / (coin + ".json")
    existing = []
    if path.exists():
        try:
            with open(path, encoding="utf-8") as f:
                existing = json.load(f)
        except Exception as e:
            raise RuntimeError("%s 本地日线不可读：%s" % (coin, e))
    # 旧文件可能已被未闭合日线污染；计算增量起点前先剔除。
    existing = merge_closed_daily_bars(existing, [], now_ms)
    last_t = max((int(x["t"]) for x in existing), default=0)
    end = daily_request_end(now_ms)
    # 保留 2 天重叠，防止漏掉或沿用错误的历史快照。
    request_start = max(full_start, last_t - 2 * DAY_MS) if existing else full_start
    if request_start < end:
        bars = post(INFO_URL, {"type": "candleSnapshot", "req": {
            "coin": coin, "interval": "1d", "startTime": request_start, "endTime": end}})
        if not bars and require_fresh:
            raise RuntimeError("%s 交易所返回空日线；拒绝沿用旧缓存" % coin)
        existing = merge_closed_daily_bars(existing, bars, now_ms)
    if require_fresh:
        require_latest_closed_daily_bar(existing, now_ms, coin)
    temporary = path.with_suffix(".json.tmp")
    with open(temporary, "w") as f:
        json.dump(existing, f)
    temporary.replace(path)
    return coin, len(existing)


def main():
    meta = post(INFO_URL, {"type": "metaAndAssetCtxs"})
    # meta 仍包含已停交易币；只有当前有价格和成交额的币才强制 freshness。
    universe = []
    for definition, context in zip(meta[0]["universe"], meta[1]):
        if ":" in definition["name"]:
            continue
        try:
            active = (float(context.get("markPx", 0)) > 0
                      and float(context.get("dayNtlVlm", 0)) > 0)
        except (TypeError, ValueError):
            active = False
        universe.append((definition["name"], active))
    active_count = sum(active for _, active in universe)
    print("宇宙 %d 币（活跃 %d），并发 %d" % (
        len(universe), active_count, MAX_WORKERS), flush=True)

    now_ms = int(datetime.datetime.now(datetime.UTC).timestamp() * 1000)
    full_start = int(datetime.datetime(2023, 2, 26, tzinfo=datetime.UTC).timestamp() * 1000)
    out_dir = BASE / "data" / "hl2"
    os.makedirs(out_dir, exist_ok=True)

    failures = []
    updated = 0
    with ThreadPoolExecutor(max_workers=MAX_WORKERS) as executor:
        futures = {executor.submit(fetch_one, coin, out_dir, now_ms, full_start, active): coin
                   for coin, active in universe}
        for done, future in enumerate(as_completed(futures), 1):
            coin = futures[future]
            try:
                future.result()
                updated += 1
            except Exception as e:
                failures.append((coin, str(e)))
                print("[ohlcv] %s 失败：%s" % (coin, e), flush=True)
            if done % 20 == 0:
                print("  %d/%d（校验通过 %d，失败 %d）" % (
                    done, len(universe), updated, len(failures)), flush=True)
    if failures:
        print("[ohlcv] %d 币失败：%s" % (
            len(failures), ", ".join(coin for coin, _ in failures)), flush=True)
        raise SystemExit(1)
    print("完成：%d 个活跃币全部具有上一闭合日线；%d 个历史币保留数据" % (
        active_count, len(universe) - active_count), flush=True)


if __name__ == "__main__":
    main()
