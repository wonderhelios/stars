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
import urllib.request
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


def info_post(body):
    req = urllib.request.Request(INFO_URL, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    return json.load(urllib.request.urlopen(req, timeout=30))


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
    return {
        "r": signed.r.to_bytes(32, "big").hex(),
        "s": signed.s.to_bytes(32, "big").hex(),
        "v": signed.v,  # eth_account 的 v 已是 27/28，无需再加
    }


def exchange_post(wallet_priv: str, action: dict, vault_address: str | None):
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
    return json.load(urllib.request.urlopen(req, timeout=30))


def current_positions(vault_address: str):
    """查询子账户当前持仓（coin → signed size）。"""
    st = info_post({"type": "clearinghouseState", "user": vault_address})
    pos = {}
    for p in st.get("assetPositions", []):
        p = p["position"]
        pos[p["coin"]] = float(p["szi"])
    return pos


def build_orders(target: dict, current: dict, idx: dict, decimals: dict, equity: float):
    """目标持仓（{coin: signed_notional}）vs 当前持仓 → 下单列表。

    前 6 多后 6 空，每个仓位名义 = equity / 6（多空各 100% = 2x 杠杆）。
    """
    orders = []
    # 目标每个仓位的名义（USD）
    per_leg = equity / 6.0
    for coin, target_notional in target.items():
        asset_idx = idx[coin]
        cur_size = current.get(coin, 0.0)
        dec = decimals.get(coin, 5)
        # 当前价格（取中间价，简化用 markPx）
        mids = info_post({"type": "allMids"})
        px = float(mids.get(coin, 0.0))
        if px <= 0:
            continue
        # 目标数量（币）= 名义 / 价格，多头为正、空头为负
        target_size = (per_leg / px) * (1.0 if target_notional > 0 else -1.0)
        delta = target_size - cur_size
        if abs(delta) * px < 10:  # 小于 $10 不调
            continue
        order = {
            "a": asset_idx,
            "b": delta > 0,           # 买 = 加多/减空
            "p": str(px),
            "s": round_sz(abs(delta), dec),
            "r": False,
            "t": {"limit": {"tif": "Ioc"}},
        }
        # cloid 只在有值时加（Rust 里 skip_serializing_if = Option::is_none）
        orders.append(order)
    return orders


def main():
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--wallet", required=True, help="API wallet 私钥（authorized agent，不是主钱包）")
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
    if not wallet.startswith("0x") or len(wallet) != 66:
        print("错误：--wallet 应是 0x 开头的 64 位 hex 私钥", file=sys.stderr)
        sys.exit(1)

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
