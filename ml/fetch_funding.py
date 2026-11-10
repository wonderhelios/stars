"""增量拉取 Hyperliquid 资金费率和 premium 到 ml/data/hl_funding/。

请求失败会打印原因并有限重试；最终失败使进程非零退出，daily 不得继续使用旧数据。
"""
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor, as_completed
import datetime
import json
import os
import time
import urllib.error
import urllib.request

BASE = Path(__file__).resolve().parent
INFO_URL = "https://api.hyperliquid.xyz/info"
UTC = datetime.timezone.utc
MAX_RETRIES = 3
MAX_WORKERS = 4


def market_data_files(directory):
    """只读真实 JSON；排除 macOS AppleDouble（._*）和其他隐藏文件。"""
    return sorted(
        path for path in Path(directory).glob("*.json")
        if path.is_file() and not path.name.startswith(".")
    )


def hl_info(body, retries=MAX_RETRIES, timeout=30):
    """调用 HL info；每次请求最多 retries 次，429 遵守 Retry-After。"""
    coin = body.get("coin", "?")
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
            try:
                wait = float(retry_after) if retry_after else min(30.0, 5.0 * (2 ** attempt))
            except ValueError:
                wait = min(30.0, 5.0 * (2 ** attempt))
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


def pull(coin, out_dir, end_ms):
    path = out_dir / (coin + ".json")
    existing = {}
    if path.exists():
        try:
            with open(path, encoding="utf-8") as f:
                existing = json.load(f)
        except Exception as e:
            print("[funding] %s 旧文件不可读，将全量重拉：%s" % (coin, e), flush=True)
            existing = {}
    last_day = max(existing.keys()) if existing else None
    if last_day:
        # 从最后一天的前一天开始，重叠一天防漏。
        t = datetime.datetime.strptime(last_day, "%Y-%m-%d").replace(tzinfo=UTC)
        cur = int(t.timestamp() * 1000) - 86400000
    else:
        cur = int(datetime.datetime(2023, 5, 1, tzinfo=UTC).timestamp() * 1000)
    records = []
    guard = 0
    while cur < end_ms:
        if guard >= 300:
            raise RuntimeError("%s funding 分页超过 300 次，中止以防死循环" % coin)
        batch = hl_info({"type": "fundingHistory", "coin": coin, "startTime": cur})
        if not batch:
            break
        records.extend(batch)
        next_cur = int(batch[-1]["time"]) + 1
        if next_cur <= cur:
            raise RuntimeError("%s funding 分页时间没有前进，中止以防死循环" % coin)
        cur = next_cur
        if len(batch) < 2:
            break
        guard += 1
    daily = {}
    for record in records:
        day = datetime.datetime.fromtimestamp(int(record["time"]) / 1000, UTC).strftime("%Y-%m-%d")
        values = daily.setdefault(day, {"fr": [], "prem": []})
        try:
            values["fr"].append(float(record["fundingRate"]))
        except Exception:
            pass
        try:
            values["prem"].append(float(record["premium"]))
        except Exception:
            pass
    new = {
        day: {
            "fr": sum(v["fr"]) / len(v["fr"]) if v["fr"] else None,
            "prem": sum(v["prem"]) / len(v["prem"]) if v["prem"] else None,
        }
        for day, v in daily.items()
    }
    return coin, {**existing, **new}, len(new)


def main():
    out_dir = BASE / "data" / "hl_funding"
    os.makedirs(out_dir, exist_ok=True)
    coins = [path.stem for path in market_data_files(BASE / "data" / "hl2")]
    end_ms = int(datetime.datetime.now(UTC).timestamp() * 1000)
    print("[funding] 开始更新 %d 币，%d 并发，单请求最多重试 %d 次" % (
        len(coins), MAX_WORKERS, MAX_RETRIES), flush=True)
    with ThreadPoolExecutor(max_workers=MAX_WORKERS) as executor:
        futures = {executor.submit(pull, coin, out_dir, end_ms): coin for coin in coins}
        done = 0
        for future in as_completed(futures):
            coin, merged, count = future.result()  # 任一币最终失败则非零退出，daily 中止。
            path = out_dir / (coin + ".json")
            temporary = out_dir / (coin + ".json.tmp")
            with open(temporary, "w", encoding="utf-8") as f:
                json.dump(merged, f)
            temporary.replace(path)
            done += 1
            print("[funding] [%d/%d] %s %d 天（新 %d）" % (
                done, len(coins), coin, len(merged), count), flush=True)
    print("[funding] 完成", flush=True)


if __name__ == "__main__":
    main()
