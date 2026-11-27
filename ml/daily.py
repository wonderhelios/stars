"""ML 每天一次的完整流程（cron 调用）：
  0. UTC 日级幂等锁检查（防止同日重复 cron 如 00:15 UTC + 16:15 UTC 重复下单）
  1. 增量拉数据（OHLCV + funding + OI）
  2. 重算 151 特征
  3. 判断是否调仓日（从 2026-05-04 起每 3 天）→ 是则校验/季度扩展训练模型 + 调 Rust 下单
  4. 执行完成后原子写入 UTC 日级锁文件

环境变量（可不设，有默认值）：
  STARS_ML_KEY_FILE   API 钱包私钥文件（默认 /etc/hyper-fly/api-wallet.key）
  STARS_ML_VAULT      ML 子账户地址（必须设，或用 --vault 参数）

用法：
  python daily.py                  # 正式跑（同日重复触发自动跳过；调仓日下单）
  python daily.py --dry-run        # 只算不下单（不写锁、不受锁阻挡）
  python daily.py --force          # 强制覆盖同日幂等锁和非调仓日限制（测试/补跑用）
"""
from pathlib import Path
import argparse
import datetime
import fcntl
import json
import os
import subprocess
import sys

BASE = Path(__file__).resolve().parent
PY = sys.executable
DEFAULT_LOCK_PATH = BASE / "data" / "last_daily_run.json"
DEFAULT_FLOCK_PATH = BASE / "data" / ".daily_run.lock"

# 调仓周期：从 2026-05-04 起每 3 天（和回测/alphapilot 口径一致）
REBALANCE_START = datetime.date(2026, 5, 4)
REBALANCE_EVERY = 3


def log(msg):
    print(f"[daily {datetime.datetime.now(datetime.UTC).strftime('%Y-%m-%d %H:%M:%S')} UTC] {msg}", flush=True)


def run_script(script, *args, timeout=1800):
    """跑一个 ml/ 下的脚本，子进程 stdout/stderr 直接实时透传。"""
    log(f"执行 {script} {' '.join(args)}")
    try:
        # 不再 capture_output：长下载的错误和进度必须立即可见，便于 cron 日志排查。
        r = subprocess.run([PY, str(BASE / script), *args], timeout=timeout)
    except subprocess.TimeoutExpired:
        log(f"{script} 超时（{timeout}s）")
        return False
    if r.returncode != 0:
        log(f"{script} 失败（退出码 {r.returncode}），中止；详细错误见上方子进程日志")
        return False
    log(f"{script} 完成")
    return True


def read_daily_lock(lock_path=DEFAULT_LOCK_PATH):
    """读取持久化 UTC 日级执行记录；不存在或损坏时返回 None。"""
    path = Path(lock_path)
    if not path.exists():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else None
    except Exception as e:
        log(f"读取幂等锁文件 {path} 失败，忽略旧内容: {e}")
        return None


def write_daily_lock(utc_date, status="ok", note="", lock_path=DEFAULT_LOCK_PATH, ran_at_utc=None):
    """原子写入 UTC 日级执行锁文件。"""
    path = Path(lock_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    record = {
        "last_utc_date": utc_date.isoformat() if isinstance(utc_date, datetime.date) else str(utc_date),
        "ran_at_utc": ran_at_utc or datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "status": str(status),
        "note": str(note),
    }
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)
    return record


def is_already_ran_today(today=None, lock_path=DEFAULT_LOCK_PATH):
    """检查同一 UTC 日期是否已经成功完成过执行（防止 00:15/16:15 重复 cron）。"""
    today = today or datetime.datetime.now(datetime.UTC).date()
    record = read_daily_lock(lock_path)
    if not record:
        return False, None
    if record.get("last_utc_date") == today.isoformat() and record.get("status") == "ok":
        return True, record
    return False, record


def is_rebalance_day(today=None):
    today = today or datetime.datetime.now(datetime.UTC).date()
    return (today - REBALANCE_START).days % REBALANCE_EVERY == 0


def target_train_end_for(today=None, config=None):
    """计算当前 UTC 日期对应的季度扩展训练截止日期（不晚于昨日闭合日线）。"""
    from strategy import latest_quarter_end, load_config
    today = today or datetime.datetime.now(datetime.UTC).date()
    config = config or load_config()
    cfg_end = datetime.date.fromisoformat(str(config["train_end"]))
    if config.get("retrain_frequency") == "frozen":
        return cfg_end
    q_end = latest_quarter_end(today)
    yesterday = today - datetime.timedelta(days=1)
    return min(max(q_end, cfg_end), yesterday)


def model_is_current(today=None):
    """校验模型格式、4 周期集成组件，以及季度扩展重训是否已覆盖最近季度末。"""
    try:
        from strategy import latest_quarter_end, load_config, load_model, validate_model
        today = today or datetime.datetime.now(datetime.UTC).date()
        config = load_config()
        model = load_model()
        validate_model(model, config)
        if config.get("retrain_frequency") == "quarterly":
            model_end = datetime.date.fromisoformat(str(model["train_end"]))
            req_end = latest_quarter_end(today)
            if model_end < req_end:
                log(f"季度模型过期: train_end={model_end} < 最新季度末 {req_end}")
                return False
        return True
    except Exception as e:
        log("当前 ML 集成模型不可用: %s" % e)
        return False


def ensure_current_model(today=None):
    today = today or datetime.datetime.now(datetime.UTC).date()
    if model_is_current(today):
        return
    train_end = target_train_end_for(today)
    log(f"模型缺失、损坏或达到季度重训边界，训练扩展窗口至 {train_end.isoformat()}")
    if not run_script("train_model.py", "--train-end", train_end.isoformat(), timeout=3600):
        raise RuntimeError("集成模型训练失败，中止；不会回退旧模型或继续下单")
    if not model_is_current(today):
        raise RuntimeError("集成模型训练后校验失败，中止")


def execute_ml_rebalance(token):
    """调用本机 Rust /api/ml/execute 接口执行调仓。"""
    import urllib.request as _ur
    req = _ur.Request(
        "http://127.0.0.1:3000/api/ml/execute",
        data=b"",
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
        method="POST",
    )
    with _ur.urlopen(req, timeout=600) as r:
        return json.loads(r.read())


def _read_api_token():
    token = os.environ.get("STARS_TOKEN", "").strip()
    if token:
        return token
    for tp in ("/etc/hyper-fly/api-token", "/opt/stars/api-token"):
        try:
            token = Path(tp).read_text(encoding="utf-8").strip()
            if token:
                return token
        except Exception:
            continue
    return ""


def run_daily(args, today=None, lock_path=None):
    today = today or datetime.datetime.now(datetime.UTC).date()
    lock_path = Path(lock_path or args.lock_file)

    # 0. UTC 日级幂等锁：同日若已成功执行，除非 --force 或 --dry-run，否则直接 0 退出
    if not args.dry_run and not args.force:
        already, prev = is_already_ran_today(today=today, lock_path=lock_path)
        if already:
            log(
                f"UTC 日期 {today.isoformat()} 已完成执行（ran_at={prev.get('ran_at_utc')}, "
                f"note={prev.get('note', '')}），跳过重复触发（如需强制重跑请传 --force）"
            )
            return 0

    # 1. 更新数据
    ok = True
    for script in ["fetch_ohlcv.py", "fetch_oi.py"]:
        ok = run_script(script) and ok
    # 小时任务负责持续归档；调仓前等待其释放锁，并做最后一次增量校验。
    ok = run_script("fetch_funding.py", "--wait") and ok
    if not ok:
        log("数据拉取失败，中止（避免用旧数据调仓）")
        return 1

    # 2. 重算特征
    if not run_script("build_features.py"):
        log("特征重算失败，中止")
        return 1

    # 3. 是否调仓日
    if not (is_rebalance_day(today) or args.force):
        note = f"{today} 非调仓日，仅更新数据"
        log(f"{today} 不是调仓日（每 {REBALANCE_EVERY} 天，起始 {REBALANCE_START}），数据已更新，不下单")
        if not args.dry_run:
            write_daily_lock(today, status="ok", note=note, lock_path=lock_path)
        return 0

    # 4. 调仓日前校验 / 季度扩展重训 4 周期集成
    try:
        ensure_current_model(today)
    except Exception as e:
        log(str(e))
        return 1

    # 5. 下单（走 Rust /api/ml/execute，由 Rust 签名与 Maker 优先执行）
    vault = args.vault.strip()
    if not vault:
        log("未配置 ML 子账户地址（STARS_ML_VAULT 或 --vault）")
        return 1

    if args.dry_run:
        ok = run_script("place.py", "--vault", vault, "--dry-run", "--json")
        return 0 if ok else 1

    token = _read_api_token()
    if not token:
        log("未设置 STARS_TOKEN（环境变量或 /etc/hyper-fly/api-token 文件），无法自动下单，请手动在前端点「ML 执行调仓」")
        return 1

    try:
        resp = execute_ml_rebalance(token)
    except Exception as e:
        log(f"调用 Rust 下单接口失败: {e}")
        return 1
    if not resp.get("ok"):
        log(f"ML 调仓失败: {resp.get('error') or json.dumps(resp)}")
        return 1

    note = resp.get("note") or f"{len(resp.get('results', []))} 项订单"
    write_daily_lock(today, status="ok", note=f"rebalanced: {note}", lock_path=lock_path)
    log(f"调仓完成: {note}")
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true", help="调仓日只打印计划，不下单，不写入幂等锁")
    ap.add_argument("--force", action="store_true", help="覆盖同日幂等锁与非调仓日限制（测试/手动补跑用）")
    ap.add_argument("--vault", default=os.environ.get("STARS_ML_VAULT", ""), help="ML 子账户地址")
    ap.add_argument("--lock-file", default=str(DEFAULT_LOCK_PATH), help="UTC 日级幂等锁文件路径")
    args = ap.parse_args(argv)

    # 进程互斥锁：防止两个 cron 同时并发进入
    flock_path = Path(args.lock_file).parent / ".daily_run.lock"
    flock_path.parent.mkdir(parents=True, exist_ok=True)
    with open(flock_path, "w") as lf:
        try:
            fcntl.flock(lf.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            log("已有另一个 daily.py 实例正在运行，本次跳过")
            return
        code = run_daily(args)
    if code != 0:
        sys.exit(code)


if __name__ == "__main__":
    main()
