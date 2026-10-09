# 事先可知的代币解锁：来源核对，尚未回测

APT 2022-10-17官方说明：主网上线2022-10-12；社区/基金会部分预期按月释放；投资者/贡献者13–18个月每月3/48，19个月起每月1/48。文中没有精确链上释放时刻，不能把所有自然月12日00UTC当已验证事实。

ARB官方流通量文档称首次2024-03-16、之后按月；脚注又按365天的秒数除12计算。官方ArbitrumVestingWallet合约确实用2628000秒阶梯；部署脚本TOKEN_DEPLOYMENT_TIMESTAMP=1678968508，加365天得到2024-03-15 12:08:28 UTC，与文档日历有一天差别（跨闰年）。这只能证明源码语义和文档不完全一致，尚未核实对应实际部署钱包的start、duration及锁定金额，不能把脚本常数冒充实际全体投资者释放时刻。基金会部分按秒线性释放，也不能当每月离散冲击。

OP官方2023-03-13澄清：公开释放表是示意，不是确定的实际发行时点。因此暂不把它纳入精确24–48小时事件交易。

GitHub匿名tree API请求403，停止该API；未换身份重试。公开网页及公开raw合约/脚本可读，本次只读源码，没有运行部署程序、访问钱包凭据或发送链上交易。

下一证据门槛：找到公开实际部署地址及构造参数，核对事前可见的发布时间，并区分可领取、实际领取和转入交易所；未达到前不测试或宣称解锁策略收益。

来源：
- https://aptosnetwork.com/currents/aptos-tokenomics-overview
- https://docs.arbitrum.foundation/token-supply
- https://raw.githubusercontent.com/ArbitrumFoundation/governance/main/src/ArbitrumVestingWallet.sol
- https://raw.githubusercontent.com/ArbitrumFoundation/governance/main/scripts/vestedWalletsDeployer.ts
- https://raw.githubusercontent.com/ArbitrumFoundation/governance/main/scripts/vestedWalletsDeploymentVerifier.ts
- https://gov.optimism.io/t/clarification-on-op-token-supply/5589
