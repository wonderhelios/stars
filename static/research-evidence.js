/* Fixed research hypotheses and evidence. No trading controls or live authorization. */
(() => {
  const el = id => document.getElementById(id);
  const esc = escapeHtml;
  const pct = v => v == null || !Number.isFinite(Number(v)) ? '—' : `${Number(v)>=0?'+':''}${Number(v).toFixed(2)}%`;
  const tone = v => v == null ? '' : v >= 0 ? 'pos' : 'neg';
  const avg = v => v.length ? v.reduce((a,b)=>a+b,0)/v.length : null;
  let report = null, busy = false, lastLoad = 0, handoffBusy = false;
  const flyUrl = `https://${location.hostname}:9527/#strategies`;
  el('hyper-fly-link').href = flyUrl;

  function renderSignals() {
    const lines=[];
    for (const [venue, label] of Object.entries(VENUE)) {
      const rows=researchRows(state.paper[venue]?.data);
      for (const [kind,name] of [['','全部形态'],...Object.entries(KINDS)]) {
        const cohort=rows.filter(r=>!kind || r.kind===kind);
        const complete=cohort.filter(r=>Number(r.entry_price)>0 && Number(r.t24_price)>0);
        const values=complete.map(r=>(1-Number(r.t24_price)/Number(r.entry_price))*100-COST).sort((a,b)=>b-a);
        const n=values.length;
        const median=n ? (values[(n-1)>>1]+values[n>>1])/2 : null;
        const profits=values.filter(v=>v>0), profitTotal=profits.reduce((a,b)=>a+b,0);
        const dates=new Map();
        for (const r of complete) {const day=new Date(Number(r.triggered_at)).toISOString().slice(0,10);dates.set(day,(dates.get(day)||0)+1);}
        const coins=new Set(complete.map(r=>r.inst_id || r.coin || r.symbol));
        lines.push(`<tr><td>${esc(label)}<br><span class="muted">${esc(name)}</span></td><td>${n} / ${cohort.length}</td><td>${dates.size}日 / ${coins.size}币</td><td>${pct(avg(values))} / ${pct(median)}</td><td>${pct(values[0])} / ${pct(values[n-1])}</td><td class="${tone(avg(values.slice(1)))}">${pct(avg(values.slice(1)))}</td><td class="${tone(avg(values.slice(3)))}">${pct(avg(values.slice(3)))}</td><td>${profitTotal>0?(profits.slice(0,3).reduce((a,b)=>a+b,0)/profitTotal*100).toFixed(1)+'%':'—'}</td><td>${n?(Math.max(...dates.values())/n*100).toFixed(1)+'%':'—'}</td></tr>`);
      }
    }
    el('signal-evidence-body').innerHTML=lines.join('');
  }
  function summary(e) {
    return `<span class="${tone(e.mean)}">${pct(e.mean)}</span><br><span class="muted">n=${e.n} · ${e.days}日 · ${e.coins}币 · 中位${pct(e.median)}</span>`;
  }
  function detail(s) {
    const e=s.forward;
    const regimes=e.regimes.map(r=>`${esc(r.regime)}：${pct(r.mean)} / n=${r.n}`).join('<br>') || '尚无未来样本';
    const dates=e.dates.map(r=>`${new Date(r.day*86400000).toISOString().slice(0,10)}：${pct(r.mean)} / n=${r.n}`).join('<br>') || '尚无未来样本';
    return `<details class="evidence-detail"><summary>收益贡献 / 行情 / 日期</summary><p>冻结于 ${esc(dateTime(s.frozen_at))}。入场条件及成本固定，新样本按入场时间判定。退出按目标小时的真实快照重算（target-hour-v2），不是精确持有时长回放。</p><p>最好 ${pct(e.best)}；最差 ${pct(e.worst)}<br>去掉最好一笔 ${pct(e.without_best)}<br>去掉最好三笔 ${pct(e.without_top3)}<br>前三笔占正收益 ${e.top3_profit_share==null?'—':e.top3_profit_share.toFixed(1)+'%'}<br>最大单日样本占比 ${e.largest_day_share==null?'—':e.largest_day_share.toFixed(1)+'%'}</p><p>按UTC日期整组重采样的95%均值区间：${e.day_bootstrap_95?e.day_bootstrap_95.map(pct).join(" ～ "):"至少5个日期后显示"}。此区间未校正多重筛选，日期之间也可能相关，不代表已经证实alpha。</p><p>${regimes}</p><p>${dates}</p><p>全窗口未到期 ${s.pending_exit||0}；历史缺目标小时 ${s.historical_missing_exit ?? "—"}；新增缺目标小时 ${s.forward_missing_exit ?? "—"}（合计 ${s.missing_exit}）；实际观察时长 ${s.actual_hold_min_hours==null?"—":s.actual_hold_min_hours.toFixed(2)+"～"+s.actual_hold_max_hours.toFixed(2)+"h"}；绝对价格变动超过50% ${s.extreme_moves}（仍保留在收益中）。缺失退出可能造成幸存偏差。</p></details>`;
  }
  function renderReport() {
    if (!report) return;
    const venue=el('parallel-venue').value, hold=Number(el('parallel-hold').value);
    const family=el('parallel-family').value;
    const rows=report.studies.filter(s=>(!venue || s.venue===venue) && s.hold_hours===hold && (!family || s.id.includes(`-${family}-`)));
    renderLineCards();
    el('parallel-body').innerHTML=rows.map(s=>`<tr><td>${esc(s.venue)}<br><strong>${esc(s.name)}</strong></td><td>${esc(s.direction)} · ${s.hold_hours}h<br><span class="muted">${esc(s.rule)}</span></td><td>${summary(s.historical)}</td><td>${summary(s.forward)}</td><td class="${tone(s.forward.btc_excess)}">${pct(s.forward.btc_excess)}<br><span class="muted">匹配${s.forward.btc_matched}</span></td><td class="${tone(s.forward.market_excess)}">${pct(s.forward.market_excess)}<br><span class="muted">匹配${s.forward.market_matched}</span></td><td>${detail(s)}</td></tr>`).join('') || '<tr><td colspan="7">该平台暂无完整小时数据</td></tr>';
    const c=(report.collection_cycle!=="all"?report.execution_coverage?.current_cycle:report.execution_coverage) || {};
    const last=c.last_at ? `${dateTime(c.last_at)}（${Math.max(0,Math.floor((Date.now()-c.last_at)/60000))}分钟前）` : '尚无分钟帧';
    const coverage=report.coverage.map(v=>`${v.venue}：${v.observations}条小时快照 / ${v.markets}个合约，最新 ${v.latest?dateTime(v.latest):'无数据'}`).join('；');
    el('evidence-health').textContent=`${esc(report.version)} · 研究周期 ${esc(report.collection_cycle)} · 起点 ${report.cycle_started_at?dateTime(report.cycle_started_at):"全部历史"} · HL ${report.collection_cycle==="all"?"最近24h":"本周期"}执行采集：${c.frames||0}帧，全DEX完整${c.complete_frames||0}帧，超过90秒断档${c.gaps||0}段，最新${last}。${c.error||''} ${coverage}。${report.errors.join('；')} 数据仅含完整小时，缓存最多2分钟。价格研究按目标小时快照退出，名义4h/24h对应的实际时长可能有约1小时偏差；详情展示实际时长范围，不能代替分钟执行回放。BTC和市场超额按同方向、同持仓区间比较，非扣除β后的alpha；市场基准需至少10个原生币种，HIP-3暂缺可比基准。`;
    renderFees();
  }
  function renderLineCards() {
    const families=[['positive-reversal','正费率反转做空'],['negative-reversal','负费率反转做多'],['momentum-long','纯动量做多'],['momentum-short','纯动量做空'],['funding-only','纯正费率做空'],['confirmed-reversal','跨平台确认反转'],['opposite-control','反向对照']];
    const hold=Number(el('parallel-hold').value);
    el('study-line-cards').innerHTML=families.map(([id,name])=>{
      const lines=(report?.studies||[]).filter(s=>s.id.includes(`-${id}-`)&&s.hold_hours===hold);
      const n=lines.reduce((a,s)=>a+s.forward.n,0);
      const promising=lines.filter(s=>s.forward.n>=30 && s.forward.days>=5 && s.forward.mean>0 && s.forward.median>0 && s.forward.without_top3>0 && s.forward.market_matched>=s.forward.n*0.8 && s.forward.market_excess>0 && s.forward.day_bootstrap_95?.[0]>0 && (s.forward_missing_exit ?? s.missing_exit)===0);
      const stage=!lines.length?'数据未就绪':promising.length?'值得执行回放':n?'未来观察中':'积累新样本';
      return `<button type="button" class="study-card ${el('parallel-family').value===id?'selected':''}" data-family="${id}"><strong>${name}</strong><span class="study-stage ${promising.length?'wait':''}">${stage}</span><small>${lines.length}平台 · ${hold}h · 新观察${n}笔（平台间可能重复）</small><small>${promising.length?'初步证据为正，仍未通过实盘验证':'未通过完整执行验证，不能直接实盘'}</small></button>`;
    }).join('')+`<button type="button" class="study-card" data-family="fees"><strong>跨平台费率差</strong><span class="study-stage">持续性研究</span><small>${report?.fee_studies?.length||0}组原生币种对照</small><small>缺少双腿成交与实际结算，不能实盘交接</small></button>`;
  }
  async function loadHandoff() {
    if (handoffBusy) return;
    handoffBusy=true;
    const controller=new AbortController(), timer=setTimeout(()=>controller.abort(),10000);
    try {
      const response=await fetch('/api/research/handoff',{cache:'no-store',signal:controller.signal});
      const d=await response.json();if(!response.ok)throw new Error(d.error||`HTTP ${response.status}`);
      if(!Array.isArray(d.strategies))throw new Error('请更新Hyper Fly看板，当前策略状态接口不兼容');
      if(d.compatible_rules===false){
        el('handoff-summary').textContent=d.compatibility_error||'两项目规则版本不一致，请更新Hyper Fly。';
        el('handoff-cards').innerHTML='';return;
      }
      const list=d.strategies;
      const ready=list.filter(s=>s.validated).length;
      el('handoff-summary').textContent=`${ready?'验证通过 '+ready+' 个候选，可去 Hyper Fly 申请切换。':'暂无通过完整验证的策略。'} 当前生效：${d.active_name||'—'}${d.activation?.mode==='manual_trial'?'（人工试运行，未通过完整验证）':''} · ${d.paused?'实盘已暂停新开仓':'实盘允许新开仓'}。策略库${list.length}个候选。`;
      el('handoff-cards').innerHTML=list.map(s=>`<article class="study-card"><strong>${esc(s.name)}</strong><span class="study-stage ${s.validated?'ready':'wait'}">${s.validated?'验证通过 · 可申请切换':s.historical_passed?'待独立模拟验证':'回放证据不足'}</span><small>${esc(s.dex_scope)} · ${s.hold_hours}h · 止损${s.stop_pct}% · ${s.max_positions}仓</small><small>${esc(s.blocker||'在Hyper Fly检查空仓与订单状态后启用；不会自动恢复开仓')}</small><a href="${flyUrl}" target="_blank" rel="noopener">${s.validated?'去 Hyper Fly 启用':'去 Hyper Fly 查看 / 模拟'}</a></article>`).join('');
    } catch(e) {
      el('handoff-summary').textContent=`交接状态暂不可用：${e.name==='AbortError'?'读取超时':e.message}。需要同时更新并启动 Hyper Fly 看板，当前不能判断可否实盘。`;
      el('handoff-cards').innerHTML='';
    } finally {clearTimeout(timer);handoffBusy=false;}
  }
  function renderFees() {
    if (!report) return;
    const q=el('fee-evidence-search').value.trim().toUpperCase();
    const rows=report.fee_studies.filter(r=>`${r.coin} ${r.pair}`.toUpperCase().includes(q));
    el('fee-evidence-body').innerHTML=rows.slice(0,40).map(r=>`<tr><td>${esc(r.coin)}<br><span class="muted">${esc(r.pair)}</span></td><td>空 ${esc(r.short)}<br>多 ${esc(r.long)}</td><td>${r.n} / ${r.days}日</td><td class="${tone(r.mean_daily_spread)}">${pct(r.mean_daily_spread)}</td><td>${r.positive_pct.toFixed(1)}%</td><td>${r.longest_positive_hours.toFixed(1)}h</td><td>${pct(r.basis_range)}</td><td>${r.gaps}</td></tr>`).join('') || '<tr><td colspan="8">暂无匹配历史数据</td></tr>';
    el('fee-evidence-status').textContent=`匹配${rows.length}组，按平台和币种排序，显示前40组；搜索可查看其余。最长跨度仅表示相邻小时快照同为正，不证明中间始终为正。基差范围为百分点，不是组合盈亏。`;
  }
  async function load(force=false) {
    renderSignals();
    if (!researchCycle.id || busy || (!force && Date.now()-lastLoad<120000)) return;
    const requestedCycle=researchCycle.id;
    busy=true; el('evidence-refresh').disabled=true;
    const controller=new AbortController(), timer=setTimeout(()=>controller.abort(),60000);
    try {
      const cycle=researchCycle.id;
      const response=await fetch('/api/research/evidence?cycle='+encodeURIComponent(cycle),{cache:'no-store',signal:controller.signal});
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const next=await response.json();
      if (!Array.isArray(next.studies) || !Array.isArray(next.fee_studies)) throw new Error('证据格式不正确');
      if(cycle!==researchCycle.id)return;
      report=next;lastLoad=Date.now();renderReport();
    } catch(e) {el('evidence-health').textContent=`证据读取失败：${e.name==='AbortError'?'请求超时':e.message}。${report?'下方保留上次结果；当前未更新。':'尚无证据，不能据此判断策略。'}`;}
    finally {clearTimeout(timer);busy=false;el('evidence-refresh').disabled=false;if(researchCycle.id && requestedCycle!==researchCycle.id)load(true);}
  }
  window.researchEvidence={renderSignals,loadHandoff,resetCycle:()=>{
    report=null;lastLoad=0;renderSignals();renderLineCards();
    el('evidence-health').textContent='正在读取所选周期的证据，旧周期结果已隐藏…';
    el('parallel-body').innerHTML='<tr><td colspan="7">正在读取所选周期，成熟样本不足时不会展示旧收益。</td></tr>';
    el('fee-evidence-body').innerHTML='';el('fee-evidence-status').textContent='正在读取所选周期费率快照…';
    load(true);
  }};
  el("parallel-family").addEventListener("change",renderReport);
  el("study-line-cards").addEventListener("click",event=>{
    const card=event.target.closest("[data-family]");if(!card)return;
    if(card.dataset.family==="fees"){el("fee-history-wrapper").open=true;el("fee-history-wrapper").scrollIntoView({behavior:"smooth",block:"start"});}
    else {el("parallel-family").value=card.dataset.family;renderReport();}
  });
  el('parallel-venue').addEventListener('change',renderReport);
  el('parallel-hold').addEventListener('change',renderReport);
  el('fee-evidence-search').addEventListener('input',renderFees);
  el('evidence-refresh').addEventListener('click',()=>{load(true);loadHandoff();});
  document.querySelectorAll('.nav-item').forEach(b=>b.addEventListener('click',()=>{if(state.view!=='market'){load();loadHandoff();}}));
  setInterval(()=>{if(state.view!=='market'){load();loadHandoff();}},30000);
  renderLineCards();
  if(state.view!=='market'){load();loadHandoff();}
})();
