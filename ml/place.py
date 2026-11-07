"""ML 策略下单：把 predict.py 选出的前 6 多 / 后 6 空下单到 HL 子账户。

签名流程（按 HL L1 签名，与 Rust SDK 的 sign_l1_action 一致）：
  1. connection_id = keccak256(msgpack(action) + nonce(8 字节大端) + vault_flag + vault_addr)
  2. 用 API wallet 私钥做 EIP-712 签名（domain: Exchange/1/1337/零地址，Agent{source,connectionId}）
  3. POST /exchange，带 action + signature + nonce + vaultAddress

用法：
  python place.py --wallet <api-wallet-私钥> --vault <子账户B地址> --equity <子账户净值> [--dry-run]

说明：每 3 天调仓，前 6 多后 6 空等权，每个仓位 1/6 名义（2x 杠杆多空各 100%）。
      上线前务必用 --dry-run + 小额验证签名正确。
"""
from pathlib import Path
BASE = Path(__file__).resolve().parent
import json
import math
import urllib.request
import urllib.error
import time
import datetime
import sys

import msgpack
from eth_account import Account
from eth_account.messages import encode_typed_data
from eth_utils import keccak

from predict import latest_positions

INFO_URL = "https://api.hyperliquid.xyz/info"
EXCHANGE_URL = "https://api.hyperliquid.xyz/exchange"


def info_post(body, retries=3):
    """HL info 接口（官方限流 ~1200 次/分钟/IP ✗ 429 时指数退避重试）。"""
    req = urllib.request.Request(INFO_URL, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    last = None
    for attempt in range(retries):
        try:
            return json.load(urllib.request.urlopen(req, timeout=30))
        except urllib.error.HTTPError as e:
            last = e
            if e.code == 429:
                wait = min(30, 5 * (2 ** attempt))  # 5, 10, 20 秒
                print(f"[place] info 限流 429 ✗ 等 {wait}s 重试（{attempt+1}/{retries}）",
                      file=sys.stderr, flush=True)
                time.sleep(wait)
                continue
            raise
        except Exception as e:
            last = e
            time.sleep(2)
    raise last


def meta_index():
    """coin 名 → asset 序号，以及每个币的 szDecimals。"""
    meta = info_post({"type": "metaAndAssetCtxs"})
    universe = meta[0]["universe"]
    idx = {}
    decimals = {}
    for i, u in enumerate(universe):
        idx[u["name"]] = i
        decimals[u["name"]] = int(u.get("szDecimals", 5))
    return idx, decimals


def round_sz(sz: float, decimals: int) -> str:
    """按 szDecimals 截断数量（与交易所一致）。"""
    return f"{sz:.{decimals}f}"


def sign_l1_action(wallet_priv: str, action: dict, nonce: int, vault_address: str | None):
    packed = msgpack.packb(action, use_bin_type=True)
    data = bytearray(packed)
    data += nonce.to_bytes(8, "big")
    if vault_address:
        data += b"\x01" + bytes.fromhex(vault_address[2:])
    else:
        data += b"\x00"
    connection_id = keccak(bytes(data))

    domain = {
        "name": "Exchange",
        "version": "1",
        "chainId": 1337,
        "verifyingContract": "0x0000000000000000000000000000000000000000",
    }
    types = {
        "Agent": [
            {"name": "source", "type": "string"},
            {"name": "connectionId", "type": "bytes32"},
        ]
    }
    # connectionId 是 bytes32 ✗ 必须传 0x 开头的 66 字符 hex（传 64 字符纯 hex 会被当成 64 字节 ✗ 超长报错）
    message = {"source": "a", "connectionId": "0x" + connection_id.hex()}
    signable = encode_typed_data(domain, types, message)
    signed = Account.sign_message(signable, wallet_priv)
    # r/s 必须 "0x" 开头的 66 字符 hex（HL 按 B256 解析 ✗ 裸 hex 会恢复出错误地址）
    return {
        "r": "0x" + signed.r.to_bytes(32, "big").hex(),
        "s": "0x" + signed.s.to_bytes(32, "big").hex(),
        "v": signed.v,  # eth_account 的 v 已是 27/28，无需再加
    }


def exchange_post(wallet_priv: str, action: dict, vault_address: str | None, retries=3):
    """下单接口（429 时重新签名 + 新 nonce 重试 ✗ nonce 必须单调递增）。"""
    last = None
    for attempt in range(retries):
        nonce = int(time.time() * 1000)
        signature = sign_l1_action(wallet_priv, action, nonce, vault_address)
        payload = {
            "action": action,
            "nonce": nonce,
            "signature": signature,
        }
        if vault_address:
            payload["vaultAddress"] = vault_address
        req = urllib.request.Request(
            EXCHANGE_URL,
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json"},
        )
        try:
            return json.load(urllib.request.urlopen(req, timeout=30))
        except urllib.error.HTTPError as e:
            last = e
            if e.code == 429:
                wait = min(30, 5 * (2 ** attempt))
                print(f"[place] 下单限流 429 ✗ 等 {wait}s 重试（{attempt+1}/{retries}）",
                      file=sys.stderr, flush=True)
                time.sleep(wait)
                continue
            raise
    raise last


def current_positions(vault_address: str):
    """查询子账户当前持仓（coin → signed size）。"""
    st = info_post({"type": "clearinghouseState", "user": vault_address})
    pos = {}
    for p in st.get("assetPositions", []):
        p = p["position"]
        pos[p["coin"]] = float(p["szi"])
    return pos


SLIPPAGE = 0.005  # 和 Rust 动量一致（Ioc 吃单的滑点容忍）


def order_price(px: float, sz_decimals: int, round_up: bool) -> float:
    """价格格式化：最多 5 个有效数字，且小数位 ≤ 6 - szDecimals（和 Rust 的 order_price 一致）。
    HL 要求价格是 tick size 的倍数 ✗ 直接用 mark price 会被拒（Price must be divisible by tick size）。
    """
    if not math.isfinite(px) or px <= 0:
        return 0.0
    decimal_places = min(4 - int(math.floor(math.log10(px))), 6 - sz_decimals)
    factor = 10.0 ** decimal_places
    scaled = px * factor
    return math.ceil(scaled) / factor if round_up else math.floor(scaled) / factor


def fmt_price(px: float, sz_decimals: int, round_up: bool) -> str:
    """格式化价格字符串（避免浮点误差 ✗ 直接按 decimal_places 输出）。"""
    p = order_price(px, sz_decimals, round_up)
    if p <= 0:
        return "0"
    decimal_places = min(4 - int(math.floor(math.log10(p))), 6 - sz_decimals)
    decimal_places = max(decimal_places, 0)
    return f"{p:.{decimal_places}f}"


def build_orders(target: dict, current: dict, idx: dict, decimals: dict, equity: float):
    """目标持仓（{coin: signed_notional}）vs 当前持仓 → 下单列表。

    前 6 多后 6 空，每个仓位名义 = equity / 6（多空各 100% = 2x 杠杆）。
    """
    orders = []
    # 目标每个仓位的名义（USD）
    per_leg = equity / 6.0
    mids = info_post({"type": "allMids"})  # 只调一次（之前每个币调一次 ✗ 慢且限流）
    for coin, target_notional in target.items():
        asset_idx = idx[coin]
        cur_size = current.get(coin, 0.0)
        dec = decimals.get(coin, 5)
        px = float(mids.get(coin, 0.0))
        if px <= 0:
            continue
        # 目标数量（币）= 名义 / 价格，多头为正、空头为负
        target_size = (per_leg / px) * (1.0 if target_notional > 0 else -1.0)
        delta = target_size - cur_size
        if abs(delta) * px < 10:  # 小于 $10 不调
            continue
        buy = delta > 0  # 买 = 加多/减空
        # Ioc 吃单价：买单上浮、卖单下浮在 slippage 内；再按 tick size 格式化
        aggressive = px * (1.0 + SLIPPAGE) if buy else px * (1.0 - SLIPPAGE)
        price_str = fmt_price(aggressive, dec, round_up=not buy)
        size_str = round_sz(abs(delta), dec)
        # 数量为 0 的跳过（价格太高时可能四舍五入到 0）
        try:
            if float(size_str) <= 0:
                continue
        except ValueError:
            continue
        order = {
            "a": asset_idx,
            "b": buy,
            "p": price_str,
            "s": size_str,
            "r": False,
            "t": {"limit": {"tif": "Ioc"}},
        }
        # cloid 只在有值时加（Rust 里 skip_serializing_if = Option::is_none）
        orders.append(order)
    return orders


def main():
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--wallet", default="", help="API wallet 私钥（dry-run 只算 orders 时可省略）")
    ap.add_argument("--vault", required=True, help="ML 子账户 B 地址（vaultAddress）")
    ap.add_argument("--equity", type=float, default=0.0, help="子账户净值（不传则自动读）")
    ap.add_argument("--dry-run", action="store_true", help="只打印下单指令，不真下单")
    ap.add_argument("--json", dest="json_out", action="store_true", help="输出 JSON（给前端/监控解析）")
    args = ap.parse_args()

    wallet = args.wallet.strip()
    vault = args.vault.strip()
    # 容忍 "HL:0x..." 前缀（HL 网页复制出来常带 ✗ 纯地址才能查）
    if vault.upper().startswith("HL:"):
        vault = vault[3:].strip()
    if wallet and (not wallet.startswith("0x") or len(wallet) != 66):
        print(f"错误：--wallet 应是 0x 开头的 64 位 hex 私钥（实际长度 {len(wallet)}）", file=sys.stderr)
        sys.exit(1)
    # 诊断：打印实际用的 API wallet 地址 + vault（排查 "does not exist"）
    if wallet:
        try:
            print(f"[place] API wallet 地址: {Account.from_key(wallet).address}", file=sys.stderr, flush=True)
            print(f"[place] vault(子账户) 地址: {vault}", file=sys.stderr, flush=True)
        except Exception as e:
            print(f"[place] 推导 API wallet 地址失败: {e}", file=sys.stderr, flush=True)

    target = latest_positions()
    if not getattr(args, 'json_out', False):
        print(f"目标持仓（{target['date']}）：做多 {target['long']} 做空 {target['short']}")

    if args.equity <= 0:
        perp = info_post({"type": "clearinghouseState", "user": vault})
        equity = float(perp.get("marginSummary", {}).get("accountValue", 0) or 0)
        # 统一账户（unified）的抵押品在现货账户里 ✗ 取 max（否则现货有 $3000 也读成 0）
        try:
            spot = info_post({"type": "spotClearinghouseState", "user": vault})
            usdc = sum(
                float(b.get("total", 0) or 0)
                for b in spot.get("balances", [])
                if b.get("coin") == "USDC"
            )
            if usdc > equity:
                equity = usdc
        except Exception:
            pass
    else:
        equity = args.equity
    if not getattr(args, 'json_out', False):
        print(f"子账户净值 ${equity:,.2f}")

    idx, decimals = meta_index()
    # 目标：多头正名义、空头负名义
    target_map = {}
    for c in target["long"]:
        target_map[c] = equity / 6.0
    for c in target["short"]:
        target_map[c] = -equity / 6.0

    current = current_positions(vault)
    orders = build_orders(target_map, current, idx, decimals, equity)

    # 把 asset 序号换回币名（给前端/监控看）
    coin_of = {v: k for k, v in idx.items()}
    order_views = []
    for o in orders:
        order_views.append({
            "coin": coin_of.get(o["a"], str(o["a"])),
            "buy": o["b"],
            "price": o["p"],
            "size": o["s"],
        })

    if getattr(args, 'json_out', False):
        print(json.dumps({"target": target, "orders": order_views}, ensure_ascii=False))
        if not (args.dry_run or not orders):
            action = {"type": "order", "orders": orders, "grouping": "na"}
            resp = exchange_post(wallet, action, vault)
            print(json.dumps({"result": resp}, ensure_ascii=False))
        return

    print(f"\n需下单 {len(orders)} 笔：")
    for o in orders:
        side = "买" if o["b"] else "卖"
        print(f"  {side} asset#{o['a']} sz={o['s']} px={o['p']}")

    if args.dry_run or not orders:
        print("\n(dry-run：未真正下单)")
        return

    action = {"type": "order", "orders": orders, "grouping": "na"}
    resp = exchange_post(wallet, action, vault)
    print(f"\n下单响应: {json.dumps(resp)}")


if __name__ == "__main__":
    main()
