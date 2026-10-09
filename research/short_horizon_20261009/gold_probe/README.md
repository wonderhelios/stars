# 黄金日内历史报价来源探测

仅探测数据可取得性，未登记或运行新的黄金策略绩效。

Dukascopy官方历史导出说明包含bid/ask；现行批量S3下载说明采用Requester Pays。本次没有使用AWS凭据、付费下载或登录账户。

公开旧格式月度XAUUSD小时BID文件请求第一次超时，正常重试一次返回HTTP429。已停止该来源请求，不更换网络身份或绕过限流。probe_source.json记录URL与返回结果。当前同名.bi5文件若存在只是失败响应，不能当作报价数据解码。

公开官网说明：https://www.dukascopy.com/api/data/get/historical-data-export

现行导出格式与计费说明：https://www.dukascopy.com/wiki/en/development/data-export/

后续若恢复合法可用的历史报价，需先核对XAUUSD价格缩放、时间基准、bid/ask对应、交易时段和点差，再登记日内宏观响应/时段策略；不能拿黄金方向案例直接推断短线策略收益。
