"""ML 策略下单：把 predict.py 选出的前 8 多 / 后 8 空下单到 HL 子账户。

签名流程（按 HL L1 签名，与 Rust SDK 的 sign_l1_action 一致）：
  1. connection_id = keccak256(msgpack(action) + nonce(8 字节大端) + vault_flag + vault_addr)
  2. 用 API wallet 私钥做 EIP-712 签名（domain: Exchange/1/1337/零地址，Agent{source,connectionId}）
  3. POST /exchange，带 action + signature + nonce + vaultAddress

用法：
  python place.py --wallet <api-wallet-私钥> --vault <子账户B地址> --equity <子账户净值> [--dry-run]

说明：每 3 天调仓，前 8 多后 8 空等权，每个仓位 1/8 名义（2x 杠杆多空各 100%）。
      上线前务必用 --dry-run + 小额验证签名正确。
"""
from __future__ import annotations

from pathlib import Path
BASE = Path(__file__).resolve().parent
import json
import math
import urllib.request
import urllib.error
import time
import datetime
import sys
import uuid

# 签名依赖只在真实签名时加载；离线预测、dry-run 和订单计算不需要安装它们。
from predict import latest_positions
from strategy import load_config

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
    import msgpack
    from eth_account import Account
    from eth_account.messages import encode_typed_data
    from eth_utils import keccak

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
            resp = json.load(urllib.request.urlopen(req, timeout=30))
            # HL 下单失败通常返回 200 + {"status":"err","response":"..."} ✗ 必须显式检查 ✗
            # 否则会【静默失败】（进程正常退出 ✗ 上层误判"下单成功"）。
            if resp.get("status") == "err":
                raise RuntimeError(f"下单被交易所拒绝: {resp.get('response')}")
            return resp
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


def account_equity(vault):
    """统一账户总净值：现货 USDC 与永续视图取大者，任一必要响应异常即中止。"""
    perp = info_post({"type": "clearinghouseState", "user": vault})
    equity = float(perp.get("marginSummary", {}).get("accountValue", 0) or 0)
    if not math.isfinite(equity) or equity < 0:
        raise ValueError("永续账户净值无效，拒绝计算订单")
    # 统一账户抵押品主要在现货视图。查询失败时绝不能回退到较小的永续分账户净值，
    # 否则所有目标仓位会按错误净值缩小，产生非预期的大幅减仓。
    spot = info_post({"type": "spotClearinghouseState", "user": vault})
    balances = spot.get("balances")
    if not isinstance(balances, list):
        raise ValueError("现货账户响应缺少 balances，拒绝使用不完整净值")
    usdc_values = [b.get("total") for b in balances if b.get("coin") == "USDC"]
    if not usdc_values:
        raise ValueError("现货账户缺少 USDC 总余额，拒绝使用不完整净值")
    usdc = sum(float(value or 0) for value in usdc_values)
    if not math.isfinite(usdc) or usdc < 0:
        raise ValueError("现货 USDC 总余额无效，拒绝计算订单")
    return max(equity, usdc)


SLIPPAGE = 0.005        # 和 Rust 动量一致（Ioc 吃单的滑点容忍）
MAKER_OFFSET_BP = 1.0   # Alo 挂单相对 mid 退让 1bp，降低 Post-Only 立即被拒概率
MAKER_TIMEOUT_SEC = 12.0
MAKER_POLL_SEC = 1.5
MIN_NOTIONAL_USD = 10.0


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


_ACTION_PRIORITY = {
    "close": 0,
    "reduce": 1,
    "flip": 2,
    "increase": 3,
    "open": 4,
}


def default_drift_band() -> float:
    """从 strategy_config.json 读取默认同向持仓漂移死区（默认 0.25）。"""
    try:
        return float(load_config().get("drift_band", 0.25))
    except Exception:
        return 0.25


def build_orders_with_meta(
    target: dict,
    current: dict,
    idx: dict,
    decimals: dict,
    equity: float,
    mids=None,
    drift_band: float | None = None,
) -> tuple[list[dict], list[dict]]:
    """带符号目标权重 vs 当前持仓 → 先平后开的订单列表及执行元信息。

    规则：
      - target[coin] 是相对子账户净值的目标名义权重；目标外旧仓一律归零（全平）；
      - 同向续持仓位若 |delta_usd| / max(target_usd, 1e-9) < drift_band，跳过微调；
      - 新开仓（open）、全平仓（close）和反手（flip）绝不受 drift_band 拦截；
      - 订单按 close → reduce → flip → increase → open 排序（同级按 coin 字母序），
        且 close/reduce 严格标记 reduce_only (r=True)。
    """
    if not math.isfinite(equity) or equity <= 0:
        raise ValueError("子账户净值必须为正数")
    if not target or not all(math.isfinite(float(w)) for w in target.values()):
        raise ValueError("目标权重为空或含非有限值，拒绝下单")
    band = default_drift_band() if drift_band is None else float(drift_band)
    if not math.isfinite(band) or band < 0.0 or band >= 1.0:
        raise ValueError("drift_band 必须在 [0.0, 1.0) 内")

    items = []
    mids = mids if mids is not None else info_post({"type": "allMids"})
    # 关键: 遍历【目标 ∪ 当前持仓】，确保被移出目标旧仓也会平仓。
    for coin in sorted(set(target) | set(current)):
        asset_idx = idx.get(coin)
        if asset_idx is None:
            raise ValueError("目标或当前持仓缺少资产序号: %s" % coin)
        cur_size = float(current.get(coin, 0.0))
        dec = decimals.get(coin, 5)
        px = float(mids.get(coin, 0.0))
        if not math.isfinite(px) or px <= 0:
            raise ValueError("目标或当前持仓缺少有效价格: %s" % coin)
        # 目标数量 = 带符号权重 × 净值 / 价格；不在目标里 = 0（平仓）。
        target_size = float(target.get(coin, 0.0)) * equity / px
        delta = target_size - cur_size
        delta_usd = abs(delta) * px
        target_usd = abs(target_size) * px

        same_side = (cur_size != 0.0) and (target_size != 0.0) and ((cur_size > 0.0) == (target_size > 0.0))
        # 同向续持漂移死区：跳过微小再平衡，避免吃单磨损；新开/全平/反手始终放行。
        if same_side and band > 0.0:
            if delta_usd / max(target_usd, 1e-9) < band:
                continue

        if delta_usd < MIN_NOTIONAL_USD:  # 差额小于 $10 不调
            continue

        if target_size == 0.0:
            action = "close"
            reduce_only = True
        elif same_side and abs(target_size) < abs(cur_size):
            action = "reduce"
            reduce_only = True
        elif cur_size != 0.0 and not same_side:
            action = "flip"
            reduce_only = False
        elif same_side:
            action = "increase"
            reduce_only = False
        else:
            action = "open"
            reduce_only = False

        buy = delta > 0  # 买 = 加多/减空
        # Ioc 吃单价：买单上浮、卖单下浮在 slippage 内；再按 tick size 格式化
        aggressive = px * (1.0 + SLIPPAGE) if buy else px * (1.0 - SLIPPAGE)
        price_str = fmt_price(aggressive, dec, round_up=not buy)
        size_str = round_sz(abs(delta), dec)
        # 数量和价格格式化后必须再次校验交易所最小名义。不得为了凑 $10
        # 自动放大数量；小额调整跳过即可，不能发送必然可能被拒的订单。
        try:
            rounded_size = float(size_str)
            if rounded_size <= 0 or rounded_size * float(price_str) < MIN_NOTIONAL_USD:
                continue
        except ValueError:
            continue
        order = {
            "a": asset_idx,
            "b": buy,
            "p": price_str,
            "s": size_str,
            "r": reduce_only,
            "t": {"limit": {"tif": "Ioc"}},
        }
        meta = {
            "coin": coin,
            "action": action,
            "reduce_only": reduce_only,
            "mid": px,
            "cur_size": cur_size,
            "target_size": target_size,
            "delta_usd": delta_usd,
        }
        items.append((_ACTION_PRIORITY[action], coin, order, meta))

    items.sort(key=lambda item: (item[0], item[1]))
    orders = [item[2] for item in items]
    metas = [item[3] for item in items]
    return orders, metas


def build_orders(
    target: dict,
    current: dict,
    idx: dict,
    decimals: dict,
    equity: float,
    mids=None,
    drift_band: float | None = None,
):
    """带符号目标权重 vs 当前持仓 → 先平后开的 HL 下单列表。"""
    orders, _ = build_orders_with_meta(
        target, current, idx, decimals, equity, mids=mids, drift_band=drift_band,
    )
    return orders


def make_cloid() -> str:
    """生成 Hyperliquid 规范 cloid（0x + 32 位十六进制）。"""
    return "0x" + uuid.uuid4().hex


def build_alo_order(order: dict, mid: float, dec: int, cloid: str, offset_bp: float = MAKER_OFFSET_BP) -> dict:
    """基于基础订单生成 Alo (Post-Only) 挂单：买单略低于 mid 向下取整，卖单略高于 mid 向上取整。"""
    buy = bool(order["b"])
    off = offset_bp / 10000.0
    px = mid * (1.0 - off) if buy else mid * (1.0 + off)
    price_str = fmt_price(px, dec, round_up=not buy)
    return {
        "a": order["a"],
        "b": buy,
        "p": price_str,
        "s": order["s"],
        "r": bool(order.get("r", False)),
        "t": {"limit": {"tif": "Alo"}},
        "c": cloid,
    }


def build_ioc_order(order: dict, mid: float, dec: int, size_float: float | None = None) -> dict:
    """基于最新 mid 与剩余数量生成 IOC 吃单。"""
    buy = bool(order["b"])
    aggressive = mid * (1.0 + SLIPPAGE) if buy else mid * (1.0 - SLIPPAGE)
    price_str = fmt_price(aggressive, dec, round_up=not buy)
    size_str = order["s"] if size_float is None else round_sz(size_float, dec)
    return {
        "a": order["a"],
        "b": buy,
        "p": price_str,
        "s": size_str,
        "r": bool(order.get("r", False)),
        "t": {"limit": {"tif": "Ioc"}},
    }


def query_order_status(vault: str, cloid: str) -> dict:
    """按 cloid 查询订单真实状态，防止网络超时误判导致双倍下单。"""
    return info_post({"type": "orderStatus", "user": vault, "oid": cloid})


def _parse_order_status(resp: dict) -> tuple[str, int | None, float | None]:
    """解析 orderStatus 响应，返回 (state, oid, remaining_sz)。
    state: 'filled' | 'open' | 'absent_or_canceled' | 'unknown'
    """
    if not isinstance(resp, dict):
        return ("unknown", None, None)
    top = resp.get("status")
    if top == "unknownOid":
        return ("absent_or_canceled", None, None)
    if top != "order":
        return ("unknown", None, None)
    od = resp.get("order") or {}
    inner = od.get("status", "")
    info = od.get("order") or {}
    oid = info.get("oid")
    try:
        rem = float(info["sz"]) if "sz" in info and info["sz"] is not None else None
    except (TypeError, ValueError):
        rem = None
    if inner == "filled":
        return ("filled", oid, 0.0)
    if inner == "open":
        return ("open", oid, rem)
    if inner in ("canceled", "rejected", "marginCanceled", "minTradeNtlRejected", "reduceOnlyCanceled"):
        return ("absent_or_canceled", oid, rem)
    return ("unknown", oid, rem)


def cancel_orders(wallet_priv: str, vault: str, cancels: list[dict]):
    """批量撤单（cancels 元素格式：{'a': asset_idx, 'o': oid}）。"""
    if not cancels:
        return {"status": "ok", "response": {"type": "cancel", "data": {"statuses": []}}}
    action = {"type": "cancel", "cancels": cancels}
    return exchange_post(wallet_priv, action, vault)


def execute_orders_maker_first(
    wallet_priv: str,
    vault: str,
    orders: list[dict],
    mids: dict,
    decimals: dict,
    coin_of: dict,
    timeout_sec: float = MAKER_TIMEOUT_SEC,
    poll_sec: float = MAKER_POLL_SEC,
    sleep_fn=time.sleep,
    monotonic_fn=time.monotonic,
) -> dict:
    """Alo Maker 优先执行器：
    1. 为每笔单生成独立 cloid 并批量挂 Alo (Post-Only)；
    2. 网络异常时按 cloid 查 orderStatus 防重，仅确认未落地才允许回退 IOC；
    3. Post-Only 被拒的立即加入 IOC 回退队列；
    4. resting 挂单轮询等待最多 timeout_sec 秒；
    5. 超时未完全成交的先撤单再查剩余量，仅对剩余名义 >= $10 的部分补发 IOC。
    """
    if not orders:
        return {"mode": "maker_first", "maker_filled": 0, "ioc_fallback": 0, "errors": [], "ioc_resp": None}

    alo_orders = []
    meta_list = []
    for o in orders:
        coin = coin_of.get(o["a"], str(o["a"]))
        mid = float(mids.get(coin, 0.0))
        dec = int(decimals.get(coin, 5))
        cloid = make_cloid()
        alo_o = build_alo_order(o, mid, dec, cloid)
        alo_orders.append(alo_o)
        meta_list.append({
            "base": o,
            "alo": alo_o,
            "coin": coin,
            "mid": mid,
            "dec": dec,
            "cloid": cloid,
            "orig_sz": float(o["s"]),
        })

    maker_filled = 0
    resting = []      # list of (meta, oid)
    fallback = []     # list of (meta, remaining_sz)
    errors = []

    action = {"type": "order", "orders": alo_orders, "grouping": "na"}
    try:
        alo_resp = exchange_post(wallet_priv, action, vault)
        statuses = (((alo_resp or {}).get("response") or {}).get("data") or {}).get("statuses") or []
        if len(statuses) != len(meta_list):
            raise RuntimeError(f"Alo 响应条数不匹配: {len(statuses)} vs {len(meta_list)}")
        for meta, st in zip(meta_list, statuses):
            if isinstance(st, dict) and "filled" in st:
                maker_filled += 1
            elif isinstance(st, dict) and "resting" in st:
                oid = (st.get("resting") or {}).get("oid")
                resting.append((meta, oid))
            elif isinstance(st, dict) and "error" in st:
                # Post-Only 立即吃单被拒等明确错误 → 安全回退 IOC
                fallback.append((meta, meta["orig_sz"]))
            else:
                errors.append(f"{meta['coin']}: 未知 Alo 响应 {st}")
    except Exception as exc:
        # 网络超时或未知异常：逐笔按 cloid 查单防重，绝不盲目补单
        print(f"[place] Alo 批量下单异常({exc})，按 cloid 查单确认防重…", file=sys.stderr, flush=True)
        for meta in meta_list:
            try:
                st_resp = query_order_status(vault, meta["cloid"])
                state, oid, rem = _parse_order_status(st_resp)
                if state == "filled":
                    maker_filled += 1
                elif state == "open":
                    resting.append((meta, oid))
                elif state == "absent_or_canceled":
                    rem_sz = meta["orig_sz"] if rem is None else rem
                    if rem_sz > 0:
                        fallback.append((meta, rem_sz))
                else:
                    errors.append(f"{meta['coin']}: Alo 异常后状态未确认({st_resp})，已跳过防重")
            except Exception as qexc:
                errors.append(f"{meta['coin']}: Alo 异常且查单失败({qexc})，已跳过防重")

    # 轮询等待 resting 挂单成交
    if resting and timeout_sec > 0:
        deadline = monotonic_fn() + timeout_sec
        still_open = list(resting)
        while still_open and monotonic_fn() < deadline:
            sleep_fn(min(poll_sec, max(0.0, deadline - monotonic_fn())))
            next_open = []
            for meta, oid in still_open:
                try:
                    st_resp = query_order_status(vault, meta["cloid"])
                    state, q_oid, rem = _parse_order_status(st_resp)
                    if state == "filled":
                        maker_filled += 1
                    elif state == "open":
                        next_open.append((meta, q_oid if q_oid is not None else oid))
                    elif state == "absent_or_canceled":
                        rem_sz = meta["orig_sz"] if rem is None else rem
                        if rem_sz > 0:
                            fallback.append((meta, rem_sz))
                        else:
                            maker_filled += 1
                    else:
                        next_open.append((meta, oid))
                except Exception:
                    next_open.append((meta, oid))
            still_open = next_open
        resting = still_open

    # 超时仍未完全成交的：先撤单，再查一次最终剩余量，仅对剩余部分补 IOC
    if resting:
        cancels = [{"a": m["base"]["a"], "o": oid} for m, oid in resting if oid is not None]
        if cancels:
            try:
                cancel_orders(wallet_priv, vault, cancels)
            except Exception as cexc:
                print(f"[place] 批量撤单异常: {cexc}", file=sys.stderr, flush=True)
        for meta, oid in resting:
            try:
                st_resp = query_order_status(vault, meta["cloid"])
                state, _, rem = _parse_order_status(st_resp)
                if state == "filled":
                    maker_filled += 1
                elif state == "absent_or_canceled":
                    rem_sz = meta["orig_sz"] if rem is None else rem
                    if rem_sz > 0:
                        fallback.append((meta, rem_sz))
                    else:
                        maker_filled += 1
                else:
                    errors.append(f"{meta['coin']}: 撤单后仍未确认取消({state})，跳过补单防重")
            except Exception as qexc:
                errors.append(f"{meta['coin']}: 撤单后查单失败({qexc})，跳过补单防重")

    # 对回退列表补发 IOC（过滤掉名义 < $10 或四舍五入后 <= 0 的残余）
    ioc_orders = []
    if fallback:
        try:
            latest_mids = info_post({"type": "allMids"})
        except Exception:
            latest_mids = mids
        for meta, rem_sz in fallback:
            px = float((latest_mids or {}).get(meta["coin"], meta["mid"]) or meta["mid"])
            if rem_sz * px < MIN_NOTIONAL_USD:
                continue
            ioc_o = build_ioc_order(meta["base"], px, meta["dec"], size_float=rem_sz)
            try:
                if float(ioc_o["s"]) <= 0:
                    continue
            except ValueError:
                continue
            ioc_orders.append(ioc_o)

    ioc_resp = None
    if ioc_orders:
        ioc_action = {"type": "order", "orders": ioc_orders, "grouping": "na"}
        ioc_resp = exchange_post(wallet_priv, ioc_action, vault)

    return {
        "mode": "maker_first",
        "maker_filled": maker_filled,
        "ioc_fallback": len(ioc_orders),
        "errors": errors,
        "ioc_resp": ioc_resp,
    }


def main():
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--wallet", default="", help="API wallet 私钥（dry-run 只算 orders 时可省略）")
    ap.add_argument("--vault", required=True, help="ML 子账户 B 地址（vaultAddress）")
    ap.add_argument("--equity", type=float, default=0.0, help="子账户净值（不传则自动读）")
    ap.add_argument("--dry-run", action="store_true", help="只打印下单指令，不真下单")
    ap.add_argument("--ioc-only", action="store_true", help="跳过 Alo Maker，直接发 IOC 吃单")
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
            from eth_account import Account
            print(f"[place] API wallet 地址: {Account.from_key(wallet).address}", file=sys.stderr, flush=True)
            print(f"[place] vault(子账户) 地址: {vault}", file=sys.stderr, flush=True)
        except Exception as e:
            print(f"[place] 推导 API wallet 地址失败: {e}", file=sys.stderr, flush=True)

    current = current_positions(vault)
    # 持仓缓冲（exit_rank=13）必须基于真实旧仓方向和当前截面排名，不能使用无持仓预览。
    target = latest_positions(current_positions=current)
    if not getattr(args, 'json_out', False):
        print(f"目标持仓（{target['date']}）：做多 {target['long']} 做空 {target['short']}")

    if args.equity <= 0:
        equity = account_equity(vault)
    else:
        equity = args.equity
    if not getattr(args, 'json_out', False):
        print(f"子账户净值 ${equity:,.2f}")

    idx, decimals = meta_index()
    target_map = target.get("weights")
    if not isinstance(target_map, dict) or not target_map:
        raise ValueError("预测结果缺少带符号 weights，拒绝下单")

    drift_band = target.get("drift_band")
    mids = info_post({"type": "allMids"})
    orders, order_metas = build_orders_with_meta(
        target_map, current, idx, decimals, equity, mids=mids, drift_band=drift_band,
    )

    # 把 asset 序号换回币名，并保留 close-first 顺序与 reduce_only/mid/action 字段给 Rust /api/ml/execute
    coin_of = {v: k for k, v in idx.items()}
    order_views = []
    for o, meta in zip(orders, order_metas):
        order_views.append({
            "coin": coin_of.get(o["a"], str(o["a"])),
            "buy": o["b"],
            "price": o["p"],
            "mid": meta["mid"],
            "size": o["s"],
            "reduce_only": bool(o.get("r", False)),
            "action": meta["action"],
            "cur_size": meta["cur_size"],
            "target_size": meta["target_size"],
            "delta_usd": round(meta["delta_usd"], 4),
        })

    def _exec():
        if args.ioc_only:
            action = {"type": "order", "orders": orders, "grouping": "na"}
            return exchange_post(wallet, action, vault)
        return execute_orders_maker_first(wallet, vault, orders, mids, decimals, coin_of)

    if getattr(args, 'json_out', False):
        print(json.dumps({"target": target, "orders": order_views}, ensure_ascii=False))
        if not (args.dry_run or not orders):
            resp = _exec()
            print(json.dumps({"result": resp}, ensure_ascii=False))
        return

    print(f"\n需下单 {len(orders)} 笔（{'IOC' if args.ioc_only else 'Alo Maker 优先'}）：")
    for o in orders:
        side = "买" if o["b"] else "卖"
        print(f"  {side} asset#{o['a']} sz={o['s']} px={o['p']}")

    if args.dry_run or not orders:
        print("\n(dry-run：未真正下单)")
        return

    resp = _exec()
    print(f"\n下单响应: {json.dumps(resp, ensure_ascii=False)}")


if __name__ == "__main__":
    main()
