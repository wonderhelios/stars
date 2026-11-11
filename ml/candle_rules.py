"""Hyperliquid 日线的闭合边界规则；纯函数，不访问网络。"""

DAY_MS = 86_400_000


def daily_request_end(now_ms):
    """返回 candleSnapshot 的包含式 endTime：上一根已闭合日线的末尾。"""
    day_start = int(now_ms) // DAY_MS * DAY_MS
    return day_start - 1


def merge_closed_daily_bars(existing, incoming, now_ms):
    """按开盘时间合并日线，并同时剔除旧数据及新响应中的未闭合 bar。"""
    now_ms = int(now_ms)
    day_start = now_ms // DAY_MS * DAY_MS
    by_t = {}
    for bar in list(existing) + list(incoming):
        # t 防开盘边界包含当天；T 防历史快照尚未真正收盘。
        if int(bar["t"]) < day_start and int(bar["T"]) < now_ms:
            by_t[int(bar["t"])] = bar
    return sorted(by_t.values(), key=lambda bar: int(bar["t"]))
