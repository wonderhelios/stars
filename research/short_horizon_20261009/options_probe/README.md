# 历史期权数据可行性核对（尚无策略收益）

2026-10-09，通过正常公开访问 `https://history.deribit.com/api/v2/public/` 取得2024年已到期期权的定义和真实成交。没有账户登录、私有接口或订单操作。

- `get_instrument` 实测可取得 BTC-27SEP24-60000-C 的strike、创建时间、到期时间和合约单位。创建时间可用来防止交易尚未挂牌的合约；返回费率仍须核实是否代表历史收费。
- `get_last_trades_by_instrument_and_time` 实测取得2024-09-26成交，包含price、mark_price、iv、index_price、direction、amount、trade_id、combo等字段。成交和mark均不是当时完整买卖报价。
- 63000行权价C/P在固定六小时窗口分别22/36笔，普通成交21/35笔，其余含组合/大宗。两腿没有可匹配的同方向、同数量、同时间完整跨式组合。原60000-C的五笔探测全部属于组合成交，不能拆成可以独立成交的单腿价格。
- 所有原始响应与六小时覆盖计数保留；`has_more=false`仅说明这两份指定窗口已完整，不代表整个历史市场完整。

官方边界：[get_instruments](https://docs.deribit.com/api-reference/market-data/public-get_instruments) 的 `expired=true` 只是近期到期列表，不是历史全链；更早名字可用get_instrument查询。[成交接口](https://docs.deribit.com/api-reference/market-data/public-get_last_trades_by_instrument_and_time) 和[期权数据说明](https://docs.deribit.com/articles/options-data-collection-best-practices)分别说明成交字段与买卖盘口/希腊值来源。私有historical订单文档与公共历史市场数据是两回事，不要混用。

下一步可先用事件前的全币种公开期权成交发现当时实际交易的合约，再核对创建/到期时间和各腿流动性；这是“历史已成交合约池”，不能标为完整挂牌期权链。优先测试全额付费、最大损失有限的事件多头期权结构，避免用DVOL差代替权利金盈亏。必须先登记事件、选约和成本规则，再读取绩效。

仍缺历史bid/ask与挂单队列；任何成交价代理只能用于保守筛选/否决，不能直接证明可实现高Sharpe。手续费、结算币种、兑换费用、逐日盯市、单腿无法成交的处理、资金占用和跨期结算都必须入账。当前没有发现或验证期权策略，也没有期权年化收益数字。
