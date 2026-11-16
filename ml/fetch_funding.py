"""增量拉取原始小时资金费，并按平台 (t, t+1d] 口径生成日聚合。

原始缓存：ml/data/hl_funding_hourly/{COIN}.json.gz
日聚合：  ml/data/hl_funding/{COIN}.json
单线程、有限重试、断点续抓；失败必须非零退出，daily 不得沿用旧数据下单。
"""
from pathlib import Path
import datetime
import gzip
import json
import os
import time
import urllib.error
import urllib.request

BASE = Path(__file__).resolve().parent
INFO_URL = "https://api.hyperliquid.xyz/info"
UTC = datetime.timezone.utc
MAX_RETRIES = 3
REQUEST_INTERVAL_SECONDS = 0.35
START_MS = int(datetime.datetime(2023, 5, 1, tzinfo=UTC).timestamp() * 1000)


def market_data_files(directory):
    return sorted(path for path in Path(directory).glob("*.json")
                  if path.is_file() and not path.name.startswith("."))


def hl_info(body, retries=MAX_RETRIES, timeout=40):
    coin = body.get("coin", "?")
    error = "未知错误"
    for attempt in range(retries):
        req = urllib.request.Request(
            INFO_URL, data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        try:
            return json.load(urllib.request.urlopen(req, timeout=timeout))
        except urllib.error.HTTPError as e:
            try:
                detail = e.read(300).decode("utf-8", errors="replace")
            except Exception:
                detail = ""
            error = "HTTP %s %s" % (e.code, detail.strip())
            retry_after = e.headers.get("Retry-After") if e.headers else None
            backoff = min(120.0, 30.0 * (2 ** attempt)) if e.code == 429 else min(30.0, 2.0 ** attempt)
            try:
                wait = max(float(retry_after), backoff) if retry_after else backoff
            except ValueError:
                wait = backoff
        except Exception as e:
            error = "%s: %s" % (type(e).__name__, e)
            wait = min(30.0, 2.0 ** attempt)
        print("[funding] %s 请求失败（%d/%d）：%s" % (
            coin, attempt + 1, retries, error), flush=True)
        if attempt + 1 < retries:
            print("[funding] %s %.1f 秒后重试" % (coin, wait), flush=True)
            time.sleep(wait)
    raise RuntimeError("%s funding 请求连续失败 %d 次，中止；最后错误：%s" % (
        coin, retries, error))


def platform_day(time_ms):
    """UTC 00:00 的小时记录归入前一资金费日，即 (D-1 00:00, D 00:00]。"""
    value = int(time_ms)
    if value % 86400000 == 0:
        value -= 1
    return datetime.datetime.fromtimestamp(value / 1000, UTC).strftime("%Y-%m-%d")


def aggregate_daily(records):
    values = {}
    for record in records:
        day = platform_day(record["time"])
        item = values.setdefault(day, {"fr_sum": 0.0, "fr_n": 0, "prem_sum": 0.0, "prem_n": 0})
        try:
            item["fr_sum"] += float(record["fundingRate"])
            item["fr_n"] += 1
        except (KeyError, TypeError, ValueError):
            pass
        try:
            item["prem_sum"] += float(record["premium"])
            item["prem_n"] += 1
        except (KeyError, TypeError, ValueError):
            pass
    return {
        day: {
            "fr": item["fr_sum"] / item["fr_n"] if item["fr_n"] else None,
            "prem": item["prem_sum"] / item["prem_n"] if item["prem_n"] else None,
            "hours": item["fr_n"],
        }
        for day, item in sorted(values.items())
    }


def pull(coin, out_dir, end_ms):
    out_dir = Path(out_dir)
    raw_dir = out_dir.parent / "hl_funding_hourly"
    raw_dir.mkdir(parents=True, exist_ok=True)
    raw_path = raw_dir / (coin + ".json.gz")
    daily_path = out_dir / (coin + ".json")
    records = []
    if raw_path.exists():
        try:
            with gzip.open(raw_path, "rt", encoding="utf-8") as f:
                records = json.load(f)
        except Exception as e:
            print("[funding] %s 原始缓存不可读，将全量重拉：%s" % (coin, e), flush=True)
            records = []
    records.sort(key=lambda row: int(row["time"]))
    latest_hour_ms = (int(end_ms) // 3600000) * 3600000
    if records and int(records[-1]["time"]) >= latest_hour_ms - 60000 and daily_path.exists():
        return coin, None, 0, True
    cursor = max(int(records[-1]["time"]) + 1, START_MS) if records else START_MS
    new_records = 0
    guard = 0
    while cursor < end_ms:
        if guard >= 300:
            raise RuntimeError("%s funding 分页超过 300 次，中止以防死循环" % coin)
        batch = hl_info({"type": "fundingHistory", "coin": coin,
                         "startTime": cursor, "endTime": int(end_ms)})
        if not batch:
            break
        records.extend(batch)
        new_records += len(batch)
        following = int(batch[-1]["time"]) + 1
        if following <= cursor:
            raise RuntimeError("%s funding 分页时间没有前进，中止" % coin)
        cursor = following
        guard += 1
        if len(batch) < 500:
            break
        time.sleep(REQUEST_INTERVAL_SECONDS)
    if new_records or not raw_path.exists():
        temporary = raw_path.with_suffix(raw_path.suffix + ".tmp")
        with gzip.open(temporary, "wt", encoding="utf-8") as f:
            json.dump(records, f, separators=(",", ":"))
        temporary.replace(raw_path)
    return coin, aggregate_daily(records), new_records, False


def update_all():
    out_dir = BASE / "data" / "hl_funding"
    os.makedirs(out_dir, exist_ok=True)
    coins = [path.stem for path in market_data_files(BASE / "data" / "hl2")]
    end_ms = int(datetime.datetime.now(UTC).timestamp() * 1000)
    print("[funding] 检查 %d 币资金费缓存（单线程间隔 %.2f 秒，已含本小时记录将跳过）" % (
        len(coins), REQUEST_INTERVAL_SECONDS), flush=True)
    skipped = 0
    fetched = 0
    total_new = 0
    for done, coin in enumerate(coins, 1):
        _, daily, count, was_skipped = pull(coin, out_dir, end_ms)
        if was_skipped:
            skipped += 1
        else:
            fetched += 1
            total_new += count
            path = out_dir / (coin + ".json")
            temporary = out_dir / (coin + ".json.tmp")
            with open(temporary, "w", encoding="utf-8") as f:
                json.dump(daily, f, separators=(",", ":"))
            temporary.replace(path)
            if count > 0 or done % 50 == 0:
                print("[funding] [%d/%d] %s %d 天（新 %d 条小时记录）" % (
                    done, len(coins), coin, len(daily), count), flush=True)
            if done < len(coins):
                time.sleep(REQUEST_INTERVAL_SECONDS)
    print("[funding] 完成：请求 %d 币，跳过已最新 %d 币，新增 %d 条小时记录" % (
        fetched, skipped, total_new), flush=True)


def main():
    import argparse
    import fcntl

    parser = argparse.ArgumentParser()
    parser.add_argument("--wait", action="store_true",
                        help="等待已有下载结束；daily 调仓前使用，避免沿用旧数据")
    args = parser.parse_args()
    lock_path = BASE / "data" / ".fetch_funding.lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with open(lock_path, "w") as lock_file:
        try:
            fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX | (0 if args.wait else fcntl.LOCK_NB))
        except BlockingIOError:
            print("[funding] 已有更新进程运行，小时任务本次跳过", flush=True)
            return
        if args.wait:
            print("[funding] 已获得更新锁，开始调仓前最终增量校验", flush=True)
        update_all()


if __name__ == "__main__":
    main()
