"""ML 每天一次的完整流程（cron 调用）：
  1. 增量拉数据（OHLCV + funding + OI）
  2. 重算 151 特征
  3. 判断是否调仓日（从 2026-05-04 起每 3 天）→ 是则 predict + place 下单

环境变量（可不设，有默认值）：
  STARS_ML_KEY_FILE   API 钱包私钥文件（默认 /etc/hyper-fly/api-wallet.key）
  STARS_ML_VAULT      ML 子账户地址（必须设 ✗ 或用 --vault 参数）

用法：
  python daily.py                  # 正式跑（调仓日会下单）
  python daily.py --dry-run        # 只算不下单（调仓日只打印计划）
  python daily.py --force          # 不管是不是调仓日都跑一次 place（测试用）
"""
from pathlib import Path
import subprocess
import sys
import datetime
import os
import argparse

BASE = Path(__file__).resolve().parent
PY = sys.executable

# 调仓周期：从 2026-05-04 起每 3 天（和回测/alphapilot 口径一致）
REBALANCE_START = datetime.date(2026, 5, 4)
REBALANCE_EVERY = 3


def log(msg):
    print(f"[daily {datetime.datetime.now(datetime.UTC).strftime('%Y-%m-%d %H:%M:%S')} UTC] {msg}", flush=True)


def run_script(script, *args, timeout=1800):
    """跑一个 ml/ 下的脚本，失败返回 False。"""
    log(f"执行 {script} {' '.join(args)}")
    try:
        r = subprocess.run(
            [PY, str(BASE / script), *args],
            capture_output=True, text=True, timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        log(f"{script} 超时（{timeout}s）")
        return False
    if r.returncode != 0:
        log(f"{script} 失败（退出码 {r.returncode}）: {(r.stderr or '')[-800:]}")
        return False
    out = (r.stdout or "").strip()
    if out:
        log(f"{script} 输出: {out[-400:]}")
    return True


def is_rebalance_day(today=None):
    today = today or datetime.datetime.now(datetime.UTC).date()
    return (today - REBALANCE_START).days % REBALANCE_EVERY == 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true", help="调仓日只打印计划，不下单")
    ap.add_argument("--force", action="store_true", help="非调仓日也跑一次 place（测试用）")
    ap.add_argument("--vault", default=os.environ.get("STARS_ML_VAULT", ""), help="ML 子账户地址")
    args = ap.parse_args()

    # 1. 更新数据
    ok = True
    for s in ["fetch_ohlcv.py", "fetch_funding.py", "fetch_oi.py"]:
        ok = run_script(s) and ok
    if not ok:
        log("数据拉取失败，中止（避免用旧数据调仓）")
        sys.exit(1)

    # 2. 重算特征
    if not run_script("build_features.py"):
        log("特征重算失败，中止")
        sys.exit(1)

    # 3. 是否调仓日
    today = datetime.datetime.now(datetime.UTC).date()
    if not (is_rebalance_day(today) or args.force):
        log(f"{today} 不是调仓日（每 {REBALANCE_EVERY} 天 ✗ 起始 {REBALANCE_START}），数据已更新，不下单")
        return

    # 4. 调仓日：取私钥 + 下单
    key_file = os.environ.get("STARS_ML_KEY_FILE", "/etc/hyper-fly/api-wallet.key")
    try:
        wallet = Path(key_file).read_text().strip()
    except Exception as e:
        log(f"读 API 钱包私钥失败（{key_file}）: {e}")
        sys.exit(1)
    vault = args.vault.strip()
    if not vault:
        log("未配置 ML 子账户地址（STARS_ML_VAULT 或 --vault）")
        sys.exit(1)

    extra = ["--wallet", wallet, "--vault", vault]
    if args.dry_run:
        extra.append("--dry-run")
    if not run_script("place.py", *extra):
        log("下单失败")
        sys.exit(1)
    log("调仓完成" + ("（dry-run 未下单）" if args.dry_run else ""))


if __name__ == "__main__":
    main()
