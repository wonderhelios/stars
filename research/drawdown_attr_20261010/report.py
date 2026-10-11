import json,hashlib
from pathlib import Path
import numpy as np,pandas as pd
import sys
sys.path.insert(0,'/tmp/drawdown_attr_plot_deps')
import plotly.graph_objects as go
from plotly.subplots import make_subplots
import study as s
p=s.OUT;S=json.load(open(p/'summary.json'));A=json.load(open(p/'audit.json'));D=pd.read_csv(p/'combined_daily.csv');B=pd.read_csv(p/'beta_summary.csv');BD=pd.read_csv(p/'beta_daily.csv');C=pd.read_csv(p/'concentration.csv');T=pd.read_csv(p/'metrics.csv');M=pd.read_csv(p/'monthly.csv');CO=pd.read_csv(p/'coin_attribution.csv');F=pd.read_csv(p/'factor_attribution.csv')
m=((D.date>'2024-03-05')&(D.date<='2024-10-15')).to_numpy();loss=S['window_loss'];names={'momentum':'动量÷波动','lowvol':'低波动','volume':'量冲击','combined':'三因子组合'}
def pct(x):return f'{x*100:.2f}%'
def pp(x):return f'{x*100:+.2f}'
def num(x):return f'{x:.2f}'
def table(headers,rows):return '| '+' | '.join(headers)+' |\n| '+' | '.join(['---']*len(headers))+' |\n'+'\n'.join('| '+' | '.join(map(str,r))+' |' for r in rows)+'\n'
def metric(label,period='drawdown'):return T[(T.strategy==label)&(T.period==period)].iloc[0]
def con(label,field):return C[(C.strategy==label)&(C.period=='drawdown')&(C.field==field)].iloc[0]
# Hierarchical additive allocation: 1x/no-fee base split into BTC and factor residuals;
# leverage and cost are two-player Shapley changes from this base. No repeated beta dollars.
base=pd.read_csv(p/'combined_1x_nofee_daily.csv');ba=np.load(p/'combined_1x_nofee.npz');ii=D.i.to_numpy();start=base.loc[m,'before'].iloc[0]
rr=s.O/np.roll(s.O,1,axis=0)-1;rr[0]=np.nan;btc=rr[:,list(s.names).index('BTC')];rd=pd.DataFrame(rr);pair_b=pd.DataFrame(np.broadcast_to(btc[:,None],rr.shape)).where(rd.notna())
betas=rd.rolling(60,min_periods=20).cov(pd.Series(btc)).div(pair_b.rolling(60,min_periods=20).var()).shift(1).to_numpy()[ii]
component=np.divide(s.sleeves[ii-2],s.norms[ii-2,None,None],out=np.zeros((len(ii),3,s.k)),where=s.norms[ii-2,None,None]>0)
fbr=np.nansum(component*betas[:,None,:],axis=2)*btc[ii,None];abr=np.nansum(ba['exposure']*betas,axis=1)*btc[ii];fp=ba['factor_pnl'][m].sum(0)/start
fb=(fbr[m]*base.before.to_numpy()[m,None]).sum(0)/start;ab=(abr[m]*base.before.to_numpy()[m]).sum()/start
execution=base.loc[m,'execution_residual'].sum()/start-(ab-fb.sum())
H=[dict(component='杠杆 1x→2.7x（含风险交互）',loss=S['shapley']['leverage']),dict(component='成本（含杠杆交互分摊）',loss=S['shapley']['cost'])]
for f,v in zip(s.factors,fp-fb):H.append(dict(component=names[f]+'：1x 无费、扣除 BTC 模型分量后的基底',loss=float(-v)))
H.extend([dict(component='BTC beta：仅 1x 无费基底',loss=float(-ab)),dict(component='执行残差：仅 1x 无费基底',loss=float(-execution))]);H=sorted(H,key=lambda x:-x['loss'])
assert abs(sum(x['loss'] for x in H)-loss)<1e-10
for x in H:x['share']=x['loss']/loss
pd.DataFrame(H).to_csv(p/'hierarchical_attribution.csv',index=False)
# Standalone diagnostic figure; all curves reset at fixed baseline peak, never reselect their peaks.
fig=make_subplots(rows=2,cols=2,subplot_titles=['Wealth from fixed peak (net fees)','Source P&L / initial peak capital','Actual beginning-position BTC beta (60d)','Universe and effective held names'])
dates=D.loc[m,'date'].tolist();x=['2024-03-05']+dates
for label,color in [('combined','#222222'),('momentum','#e76f51'),('lowvol','#457b9d'),('volume','#9c6ade'),('combined_1x','#2a9d8f'),('no_renormalization','#e9a23b')]:
 r=pd.read_csv(p/f'{label}_daily.csv').loc[m,'r'];fig.add_trace(go.Scatter(x=x,y=np.r_[1,np.cumprod(1+r)].tolist(),name=label,line=dict(color=color)),row=1,col=1)
for field,color in [('momentum','#e76f51'),('lowvol','#457b9d'),('volume','#9c6ade'),('cost_contribution','#777777')]:fig.add_trace(go.Bar(x=M.month.tolist(),y=(M[field]*100).tolist(),name=field+' P&L pp',marker_color=color),row=1,col=2)
fig.add_trace(go.Scatter(x=dates,y=BD.loc[m,'beta'].tolist(),name='60d BTC beta',line=dict(color='#457b9d')),row=2,col=1)
fig.add_trace(go.Scatter(x=dates,y=D.loc[m,'earning_universe'].tolist(),name='Eligible universe'),row=2,col=2)
fig.add_trace(go.Scatter(x=dates,y=D.loc[m,'begin_neff'].tolist(),name='Effective held names'),row=2,col=2)
fig.update_layout(height=900,width=1400,template='plotly_white',barmode='relative',title='Drawdown attribution: 2024-03-05 peak to 2024-10-15 trough',legend=dict(orientation='h',y=-.13))
fig.write_html(p/'attribution_overview.html',include_plotlyjs=True,full_html=True)
rows=[]
for f in [*s.factors,'combined']:
 for period,title in [('pre','之前'),('drawdown','回撤期'),('post','之后')]:
  q=metric(f,period);rows.append([names[f],title,int(q.days),pct(q.total),pct(q.annual_arithmetic),pct(q.cagr),pct(q.mdd),num(q.sharpe)])
single_table=table(['策略（各自 2.7x）','时段','日数','区间净收益','算术年化','CAGR','段内最大回撤','Sharpe'],rows)
calendar_table=table(['策略','2024 全年净收益','全年最大回撤'],[[names[f],pct(metric(f,'calendar_2024').total),pct(metric(f,'calendar_2024').mdd)] for f in [*s.factors,'combined']])
source_table=table(['来源','占峰值本金的 P&L（百分点）','占 80.90% 损失'],[[names.get(r.component,{'execution_residual':'执行残差','cost':'成本'}.get(r.component)),pp(r.contribution),pct(r.loss_share)] for r in F.itertuples()])
beta_table=table(['时段','持仓 beta 均值（含空仓日）','仅有仓日均值','beta 为正的有仓日','净收益 OLS beta'],[[{'pre':'之前','drawdown':'回撤期','post':'之后'}[r.period],f'{r.beta_mean:+.3f}',f'{r.beta_active_mean:+.3f}',pct(r.positive_fraction),f'{r.net_ols_beta:+.3f}'] for r in B[(B.lookback==60)&(B.minobs==20)&B.period.isin(['pre','drawdown','post'])].itertuples()])
sens_table=table(['滚动日数 / 最少共同观测','回撤期平均净 beta','beta P&L / 峰值本金（百分点）'],[[f'{r.lookback} / {r.minobs}',f'{r.beta_mean:+.3f}',pp(r.beta_contribution)] for r in B[B.period=='drawdown'].itertuples()])
crows=[]
for f in [*s.factors,'combined']:
 kval=con(f,'earning_kk');held=con(f,'begin_held');neff=con(f,'begin_neff');crows.append([names[f],f'{kval["mean"]:.2f} / {kval["min"]:.0f}–{kval["max"]:.0f}',f'{held["mean"]:.2f} / {held["min"]:.0f}–{held["max"]:.0f}',f'{neff["mean"]:.2f} / {neff["median"]:.2f} / {neff["min"]:.2f}'])
con_table=table(['策略','kk 均值 / 范围（每因子每侧）','实际持币数均值 / 范围','N_eff 均值 / 中位数 / 最小'],crows)
monthly_table=table(['月份（边界月截断）','当月净收益','合格宇宙均值','N_eff 均值','动量 / 峰值本金 pp','低波动 pp','量冲击 pp','成本 pp'],[[r.month,pct(r.total),num(r.universe_mean),num(r.neff_mean),pp(r.momentum),pp(r.lowvol),pp(r.volume),pp(r.cost_contribution)] for r in M.itertuples()])
coin_table=table(['币种','价格 P&L pp','成本 pp','合计 pp','占总损失'],[[r.coin,pp(r.price_contribution),pp(r.cost_contribution),pp(r.net_contribution),pct(r.loss_share)] for r in CO.head(10).itertuples()])
scenario_table=table(['对照账本','同窗净收益','同窗最大回撤'],[[label,pct(metric(key).total),pct(metric(key).mdd)] for key,label in [('combined','2.7x、有成本'),('combined_1x','1x、有成本'),('combined_nofee','2.7x、无成本'),('combined_1x_nofee','1x、无成本'),('no_renormalization','不重新归一化、有成本'),('equal_weight_sides','多空各侧净名单等权、仍 2.7x')]])
h_table=table(['约定归因，按损失份额排序','损失百分点（正数=损失）','占总损失'],[[r['component'],f'{r["loss"]*100:+.3f}',f'{r["share"]*100:+.2f}%'] for r in H]+[['宇宙集中','不可独立识别','N/A；不是 0%']])
text=f'''# −80.90% 回撤归因：亏损来自哪里，以及放大在哪里

研究日期：2026-10-11。范围严格限定为给定日频引擎与本地数据；不重新检验任何 IC。用户给定的动量 IC 为正、低波动 IC 长期为负、回撤期三个 IC 稳定，全部作为前提。

**结论：这次回撤的主要损失来源是动量尾部持仓与量冲击尾部持仓；抵消后再加回总名义与 2.7x 杠杆的放大由反事实账本证实；后半段小宇宙中还出现了集中空头同步受损的极端事件。低波动不是这段回撤的主要现金损失来源；“回撤期转成净多 BTC beta，遇下跌而崩溃”不符合本账本。底层 1x、零成本策略仍亏 35.86%，因此也不能把事故全部推给杠杆或手续费。**

最直接的峰值本金账：动量 **−36.30pp（总损失的 44.87%）**，量冲击 **−29.68pp（36.68%）**，成本 **−13.26pp（16.39%）**，低波动 **−1.67pp（2.06%）**，执行残差 +0.0021pp，精确合计 −80.8983pp。这里的因子份额是组合内部持仓来源账，不能与下面杠杆/beta 的另一种拆法叠加。

[打开独立交互图：净值、逐月损失来源、beta、宇宙与集中度](attribution_overview.html)（文件包含绘图库，无需联网）

## 0. 口径与复现门槛

- 原引擎：`../trailing_tp_20261008/baseline.py`，AST 仅提取初始化及函数，**不执行原脚本底部写文件的循环**。在本目录生成带观察器的函数，只把 gross 参数化、替换目标因子集合，保留原成交逻辑。
- 原始输入：`/tmp/hl-daily-v2/` 的 178 个 JSON；使用原 `request.json` 的时间过滤和原 `meta.json` 的数量精度。实际收益日为 2023-07-04 至 2026-10-07，1192 天。
- 日期按 UTC 日线记账。初始资金 100 万；open 时钟；前一日收盘信号、次日开盘调仓；费率每单位成交额 0.0007；2% 调仓带、数量取整、10 美元最低单、cap5 均不变。所有反事实从同一全样本起点独立跑，然后固定切同一窗口，**没有各自挑峰谷**。资金约束的动态 cap 所有账本都未低于 5。
- 基线重建逐日收益最大差 **0**、逐日换手最大差 **0**、日期完全一致；全样本 Sharpe 1.1292998945，最大回撤 −80.89830930%；无持仓缺失报价。
- **损失窗口是峰值 2024-03-05 调仓后净值 → 2024-10-15 调仓后净值**，即收益日 **2024-03-06～2024-10-15，共 224 日**。峰值本金 $2,387,265.54，谷值 $456,008.08，损失 $1,931,257.46。
- 三时段：之前 2023-07-04～2024-03-05（246 日）；回撤期 224 日；之后 2024-10-16～2026-10-07（722 日）。峰值当天归入“之前”。把 3 月 5 日的收益也计入会得到 225 个收益、**区间收益 −74.96%**，不能称为峰谷损失 −80.90%。225 日口径仍保存在数据文件中供对照。
- “年化”同时列算术年化 `365×mean(r)` 和 CAGR；Sharpe 为 `sqrt(365)×mean(r)/std(r,ddof=1)`，无风险利率设零。段内最大回撤把段初净值 1 计入高水位。算术年化可低于 −100%，不是本金损失比例。

## A. 单因子 P&L 与组合结构

### A1. 各自独立成交、各自 2.7x 的三个时段

{single_table}

“2024 亏多少”如果指自然年而不是本次回撤，对应如下，不能混用：

{calendar_table}

**动量单独亏 84.20%，量冲击单独亏 76.06%，低波动单独亏 56.68%；这不是只有某一个坏因子才会亏的窗口。** 动量正 IC 不保证本实现的两端等权组合赚钱：IC 衡量整个合格截面的排序关系，本引擎只取两端、按 gross 放大、以收盘信号在次日开盘成交，最终赚亏还受被选尾部的收益幅度、同向冲击和复利路径影响。这是指标与交易账本的区别，不是对 IC 前提的重新检验，也不把回归残差命名为“纯 alpha”。

### A2. “组合亏损是否大于三个单因子亏损之和”

字面答案：**没有**。三个单因子净收益之和是 **−216.94%**，组合 −80.90%。但三本各投入 1 元的账本共用了 3 元，直接相加百分比不能判断组合是否创造额外亏损，更不能由“不大于”推出“必定只有一个因子负责”。

合理的同资本比较：

- 峰值时资金各分三分之一、三本独立滚动至谷值：**−72.31%**，组合多亏 **8.59pp**。
- 每天把独立单因子净收益等权合成（每日重置三份资金的理论组合）：**−65.70%**，组合多亏 **15.20pp**。该对照是收益合成，未模拟跨子账户资金划拨。
- **直接删除原目标的最后一次 L1 归一化**，仍用同一执行引擎和费用：**−64.02%**，组合多亏 **16.88pp**。该反事实精确测的是“抵消后是否补回名义仓位”的总影响，包括随之改变的成交额、成本、净值路径；不是独立的额外费用。

原结构：每个因子先有三分之一 gross 的多空袖套 `v_f`，合并 `u=Σv_f`，然后 `w=u/g`，`g=Σ|u|`。回撤期 g 均值 **0.7221**；补回名义的倍数 `1/g` 均值 **1.4349**、最大 **2.25**。不补回时实际平均 gross **1.95x**；补回后约 **2.704x**（费用后净值作分母略高于 2.7）。**抵消本身减少净暴露，随后把剩余方向重新加大才是风险放大动作。**

删除一个因子再重新归一化的诊断：去动量 −74.15%，去低波动 −76.05%，去量冲击 −78.83%。都仍是灾难级损失；这些结果同时改变剩余因子的仓位份额，不能当作各因子的可加总边际贡献，也不是样本外策略建议。

### A3. 在实际组合里，哪些来源真正亏了钱

对每个成交日保留 `2.7×v_f/g` 对应的虚拟数量；在下一次开盘按真实价格变化计算各来源的美元 P&L。合计与实际数量之间的取整、调仓带等误差，单列执行残差；实际费用单列。每项除以同一个 **3 月 5 日峰值本金**，所以可以精确加总：

{source_table}

这是**先抵消之前的来源归属约定**，虚拟袖套可以相互对冲，不是三个可独立提款的子账户。低波动在 4、6、7 月的正贡献抵消其余月份损失，故在真实组合的资金路径中只净亏 1.67pp；不能拿其独立账本 −56.68% 替代该贡献。手续费没有强行归到具体因子。

{monthly_table}

## B. BTC beta：方向、实际贡献与局限

逐币使用 **过去 60 个 open→open 日收益、至少 20 个共同有效样本**估计带截距 beta；估计截止到收益区间起点的开盘，之后乘真实期初持仓净值权重：`B_t=Σ w_actual,t−1,j × beta_j,t−1`。beta 使用协方差与 BTC 方差的相同有效日期；当前收益未进入估计。日价格收益分成 `B_t×r_BTC,t + residual_t`，再按实际期初资金逐日换算为峰值本金贡献。历史不足的资产不凭空填 beta，归入未建模残差；窗口 gross 覆盖率平均 **99.894%**，最低 **91.667%**。

{beta_table}

回撤期实际持仓 beta 平均 **−0.270**；只有 **26.34%** 的日子为正，范围 **−1.553～+1.077**。因此不能说从前后偏空变成了持续净多 beta。金额中性也不等于 beta 中性：组合净金额权重均值接近零，而净 beta 明显为负。

独立的区间收益回归给出净 beta **−0.393**，Newey–West 7 滞后标准误的近似 95% 区间 **[−0.629, −0.158]**，R² **6.71%**；毛价格收益 beta −0.392。回归常数项和残差吸收大量其余收益，R² 不是回撤贡献份额。此前所述全样本 beta≈−0.09 不作为这一新口径的数值目标：本次与该引擎严格对齐的全样本 open→open 净收益 OLS beta 是 −0.386，差异可能来自价格时钟、样本或收益口径；未追查旧数字的来源。

**60 日模型的窗口 beta P&L 是 +0.815pp（略有保护），其余价格残差为 −68.454pp，再扣成本 −13.259pp，合计 −80.898pp。** beta 对损失的份额约 **−1.01%**；正收益不是损失来源。

{sens_table}

beta 的精确小数依赖历史长度；这些口径给出 **+0.40～+6.15pp**，支持“BTC 单因子不是主要净损失解释”，不支持“beta 完全无风险”。这也不排除山寨币共同因子、meme 风格、非线性上涨挤压、下跌相关性突变等未建模风险。

按 BTC 当天方向切分（均以同一峰值本金衡量）：BTC 下跌的 113 天，价格 P&L **+16.79pp**、beta 项 **+21.39pp**；BTC 上涨的 111 天，价格 P&L **−84.43pp**、beta 项 **−20.57pp**。所以损失更集中在上涨日的相对价格冲击，不能用“BTC 中途急跌”概括整个窗口。BTC 跌幅至少 5% 的 8 天仍有价格损失 −6.16pp，但模型 beta 项为 +3.79pp，这部分损失落在残差中。

同窗 BTC open→open 累计 **−3.33%**，算术年化 **+8.68%**。原“约 +11% 年化”与本口径不同；期内算术均值为正和复利亏损可以同时发生，也不能用“平均 beta × BTC 年化”代替实际逐日损益。

## C. 宇宙与集中：有严重尾部事故，但没有一枚币独自解释 −81%

与当日收益对应的信号宇宙，即持仓建立前一日的合格名单：均值 **21.25**，中位数 **17**，P10/P90 **12/38**，范围 **10～46**。因此“13～18 个币”适合描述后半段，**不是整个 224 日窗口的全部范围**。3 月均值约 40，7～9 月约 12～13；月度数据见上表。**仅 3 月 6～31 日已经损失峰值本金 43.87pp，占整段损失 54.23%，当时宇宙均值 40.19、N_eff 均值 18.44。故宇宙后来缩小不是整段回撤的充分解释，更不能把早期过半损失归到“仅十几个币”的状态。**

`N_eff=1/Σ(|w|/gross)²`，使用实际**期初、产生当日 P&L 的持仓**；gross 相消前后、持仓数量和 N_eff 不混用。每个因子的 kk 是每侧入选数量；cap5 不是“全组合只持 5 个币”，也不是单币 5% 的权重限制。

{con_table}

组合单币 gross 份额平均最大值 **16.09%**，极端时 **37.77%**；最大单币名义暴露达到净值的 **103.12%**；最大三币 gross 份额达到 **83.33%**。即使账面上美元多空中性，仍可能由少数币对赌决定权益。

### C1. 单币累计贡献

{coin_table}

WIF 一枚币含成本贡献 **−18.47pp，占总损失 22.83%**；前五名合计 **61.85%**，前十名 **88.99%**。这是实际交易路径的会计集中度，不是“删掉这些币就必然避免同额亏损”的反事实；盈利币和其他亏损币还会相互抵消。

### C2. 最坏的一天与最大单币事件

**2024-07-16（记账日，价格区间为 7 月 15 日开盘→7 月 16 日开盘）：组合净亏 23.75%，占峰值本金 −10.38pp，占整段损失 12.83%。** 当时只有 5 个净持币，合格宇宙 10、每因子每侧 kk=2：

| 实际持仓 | 期初净值权重 | 币价涨跌 | 对当天净值的价格贡献 |
| --- | --- | --- | --- |
| WLD 空头（动量来源） | −45.08% | +25.77% | −11.62pp |
| WIF 空头（量冲击来源） | −45.08% | +24.80% | −11.18pp |
| kPEPE 空头（动量来源） | −45.08% | +23.11% | −10.42pp |
| TIA 多头（多因子抵消后） | +45.08% | +8.53% | +3.85pp |
| BTC 多头（低波动、量冲击来源） | +90.17% | +6.51% | +5.87pp |

三枚空头合计价格贡献 −33.22pp，两个多头只抵消 +9.72pp；价格净亏 −23.50pp，再扣成本 0.26pp。该日净 BTC beta **−0.527**，beta 模型仅解释 **−3.43pp**，剩余价格损失 **−20.07pp**。这直接展示了**小宇宙 + 相关空头集中 + 残余方向放大**的风险。

另一明显尾部事件：3 月 10 日 WLD 上涨 **55.22%**，空头权重约 −12.29%，单币当天贡献 −6.79pp；按峰值本金计 **−5.27pp**，是窗口内最大单币单日峰值本金损失。7 月 16 日 WLD 的 −11.62pp 则是最大单币单日“当日净值”损失。分母不同，不能混称同一个“最大”。原始开盘价格与临近收盘价格已核对，7 月 16 日这些跳涨没有表现为单一开盘坏点；没有据此声称已完成行情源外部审计。

最差 10 个日收益对应的美元净亏合计为峰值本金 **−50.38pp**；但仍不是“一次单币黑天鹅打穿”。仅凭日线无法判断这些事件是否事前不可预见，故本报告把它们称作尾部冲击，不给“黑天鹅”作因果认定。

保持净持仓名单与方向、改成多空各侧等权且 gross 仍 2.7x 的诊断账本为 **−78.56%**，仅比原组合少亏 **2.34pp**。这说明重复因子投票造成的个别权重倾斜并非全部问题；名单本身、同向币风险和少数名额仍存在。**该对照不能把“宇宙集中”单独识别成一个损失百分比**，因为等权同时改了因子来源份额，且没有创造额外可交易币。

## D. 杠杆：把底层亏损放大了多少

{scenario_table}

最直接的有成本对照：**1x 仍亏 42.95%，2.7x 亏 80.90%；加杠杆多亏 37.95pp，占原损失 46.91%**。不是用 80.90%÷2.7 得到 29.96%：每日杠杆、每日复利、波动损耗和手续费共同决定终值。将原日收益直接除以 2.7 的固定收益路径诊断是 −43.00%，接近独立重跑的 −42.95%，差异来自成交数量、调仓带、费用与净值路径。

为避免把成本与杠杆的交互重复分配，再使用四个账本做二变量 Shapley：记 L(g,c) 为固定窗口终值损失，分别比较 g=1/2.7、c=0/0.0007；杠杆在“先加成本/后加成本”两个顺序中的增量取平均，成本同理。结果精确加总：

| 部分 | 损失百分点 | 占 −80.90% |
| --- | --- | --- |
| 原组合结构在 1x、零成本下的底层损失 | 35.86 | 44.33% |
| 杠杆增量，含分摊的交互 | 37.89 | 46.84% |
| 成本增量，含分摊的交互 | 7.14 | 8.83% |
| 合计 | 80.90 | 100.00% |

这里的杠杆项也包含放大集中度、beta、残差和波动路径的效果；不能再加上 A3 的因子损失份额或“取消归一化改善 16.88pp”。

## E. 成本：三种数字各回答什么

224 日平均单边绝对成交额/调仓前净值 **2.02163**，累计换手 **452.84485 倍**，按平均日换手×0.0007×365 的年化成本 **51.6526%**。这些是原引擎定义，不除以 2。

| 口径 | 数字 | 含义 |
| --- | --- | --- |
| 各日费用/各日期初净值的简单和 | 31.38% | 分母每天变化，不能直接称为 −81% 中的 31.38pp |
| 实际美元费用/固定峰值本金 | 13.26pp | 与真实美元价格 P&L 可加总；占实际损失 16.39% |
| 固定原每日价格收益、扣费与不扣费的终值差 | 7.13pp | 净 −80.90% vs 去费 −73.77%，仍用原每日仓位收益路径 |
| 同引擎从头重跑 2.7x、费率归零的终值改善 | 7.21pp | 净 −80.90% vs 无费 −73.69%；约占损失 8.91% |
| 与杠杆两顺序均分交互的成本 Shapley | 7.14pp | 与 D 的底层/杠杆损失可加总；占损失 8.83% |

现金费用贡献 13.26pp 大于免手续费终值改善 7.21pp，并不矛盾：原来交掉的费用若保留，会在后续亏损路径中继续承受风险；独立重跑还改变以净值定规模的数量。不能把 51.65% 年化直接乘八个月后塞进峰谷损失中。

复利账另作核对：净 log 收益 −1.65539，固定实际仓位收益的毛 log 收益 −1.33815，成本 log 拖累 −0.31724。日净简单收益之和 −1.43999，与净 log 收益差 0.21541；这是路径与非线性效应，**不是另一项可再相加的现金亏损**。

## ★ F. 最终数字排序：给出明确约定，不伪造五项独立原因

**最稳健的实际现金来源排序：动量 44.87% > 量冲击 36.68% > 成本 16.39% > 低波动 2.06%；执行残差抵消约 0.003%。** 这回答“实际是哪些来源亏掉了钱”。

用户希望把因子、beta、杠杆、成本、宇宙集中放在同一个比例排序。但因子定义仓位、宇宙决定可选名单、beta 描述这些仓位的共同风险、杠杆乘上这些风险；它们天然交叠。直接填成五项加起来 100% 会重复计算。

如果需要一个**含杠杆、成本、beta、因子且严格可加总**的排序，以下采用明确的层级约定：先取 **1x、无费、原归一化结构**为基底；把基底的因子价格 P&L 扣除逐日 60 日 BTC 模型分量，剩余称“因子来源残差”；单列基底 BTC 项；最后加入 D 的杠杆/成本 Shapley。放大的 beta、尾部与集中风险已包含在“杠杆”项中，故不再另计。因子残差包含未建模共同风险和路径影响，绝不是纯 alpha。

{h_table}

该层级排序是一种**透明的分账约定，不是唯一真实因果比例**；它与 A3 的因子份额回答不同问题。宇宙集中缺少唯一可识别的反事实，必须写 N/A，不能凭感觉分配 e%，也不能写成 0%。可观测的集中证据是前五币贡献 61.85%、最低 N_eff 3.60、单币名义最大超过 1 倍净值、7 月 16 日三个空头同步受损；可执行的组合结构对照是不重新归一化改善 16.88pp。

**最终判断：底层选出的多空尾部在这段窗口已经明显亏损，主要现金损失来自动量和量冲击来源；抵消后补回 gross、少数币集中及 2.7x 杠杆进一步放大，成本持续侵蚀。稳定 IC 没有阻止这些仓位发生真实的大幅亏损。低波动长期负 IC 值得独立处理，但删掉它只能把本窗口 −80.90% 改成 −76.05%，救不了这次事故。**

## G. 局限与可审计材料

1. 这是给定基线的离线日频账本归因，不是逐笔实盘审计。引擎不含资金费、借贷/融资、真实冲击成本、盘口容量、盘中强平和保证金风险；固定 7bp 只是原费用假设。尤其单日 −23.75% 时，盘中风险可能更差，日线无法量化。没有拿这些未知项补齐任何比例。
2. 使用当前数据文件与原 meta 精度，保留原宇宙过滤；可能存在存活者/上市资产覆盖、历史数据修订、量价数据质量、当前精度回用历史等偏差。报告通过原账本一致性和本地价格交叉核对，不代表已用独立行情源证实所有极端价格。
3. 因子来源归属以原始袖套为约定；净额抵消后实际持仓无法唯一还原“哪一个因子拥有哪一份仓位”。执行误差明示，未把残差藏进某个因子。无成本/低杠杆/等权/删因子都是固定已知窗口的事后诊断，没有样本外显著性或可部署结论。
4. beta 为线性、单市场代理的估计；短历史资产有缺口，时间变化和共同山寨风险不能都被 BTC 解释。滚动长度敏感性不是统计置信区间；收益回归的 HAC 区间也受样本量与模型设定限制。没有证明“残差必然是可重复 alpha”。
5. 三时段长短不一，之前有 157 日无期初持仓（其中可能发生开仓费用），155 日净收益为零；年化和 Sharpe 不是等长度实验。短区间 CAGR 只用于完整披露，不能外推。
6. 没有重新验证已知 IC，也没有单独检验“为什么全截面排序稳定但被选两端收益恶化”的尾部收益分布因果机制；本报告识别的是实际亏损来源与可检验的组合放大效应，不能超出数据声称找到了唯一经济原因。
7. 没有修改 `src/`、原 `baseline.py` 或原研究输出；没有 commit/push/部署，没有访问实盘服务。

复现（从 `/Users/wonder/Code/stars` 运行；制图额外依赖安装于 `/tmp/drawdown_attr_plot_deps`，未修改指定 Python 环境）：

```sh
/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 -m pip install --target /tmp/drawdown_attr_plot_deps --no-deps plotly narwhals packaging
/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/drawdown_attr_20261010/study.py
/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/drawdown_attr_20261010/audit.py
/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/drawdown_attr_20261010/report.py
```

文件索引：

- [study.py](study.py)、[extracted_observed_run.py](extracted_observed_run.py)：提取与完整计算；[input_manifest.json](input_manifest.json)：所有行情输入、原引擎与元数据 SHA-256。
- [metrics.csv](metrics.csv)：单因子、组合、所有对照的各时段净收益/年化/回撤/Sharpe；各 `*_daily.csv` 与 `*.npz` 保存日账本、实际持仓、逐币价格/成本、虚拟因子价格 P&L。
- [factor_attribution.csv](factor_attribution.csv)、[hierarchical_attribution.csv](hierarchical_attribution.csv)、[monthly.csv](monthly.csv)：精确可加总分解。
- [beta_summary.csv](beta_summary.csv)、[beta_daily.csv](beta_daily.csv)、[beta_conditions.csv](beta_conditions.csv)：beta 敏感性、逐日残差与冲击分组。
- [concentration.csv](concentration.csv)、[coin_attribution.csv](coin_attribution.csv)、[coin_day_events.csv](coin_day_events.csv)、[shock_2024_07_16.csv](shock_2024_07_16.csv)：宇宙、持仓、币种及极端日证据。`begin_*`/`earning_*` 是本报告持仓风险口径；无此前缀字段是当天调仓后快照，避免误用一天错位。
- [verification.json](verification.json)、[audit.json](audit.json)：逐日原基线一致、净值递推、真实持仓回报、因子/费用守恒、beta 手工逐币 OLS 复核。全部通过；最大 beta 核对差小于 6×10⁻¹⁵，其他收益分账误差小于 6×10⁻¹⁵。
'''
(p/'REPORT.md').write_text(text)
(p/'runtime.json').write_text(json.dumps(dict(python=sys.version,numpy=np.__version__,pandas=pd.__version__,plotly=__import__('plotly').__version__),indent=2))
(p/'output_manifest.json').write_text(json.dumps({q.name:hashlib.sha256(q.read_bytes()).hexdigest() for q in p.iterdir() if q.is_file() and q.name not in ['output_manifest.json','run.log']},indent=2))
print(pd.DataFrame(H).to_string(index=False));print('REPORT bytes',len(text.encode()))
