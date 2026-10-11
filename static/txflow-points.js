/* TxFlow only. Points never feed trading decisions or financial returns. */
(() => {
  const panel = document.getElementById('tx-points');
  if (!panel) return;
  const get = id => document.getElementById(id);
  const money = v => Number.isFinite(v) ? '$' + v.toFixed(2) : '—';
  const date = ts => new Date(ts).toISOString().slice(0,10);
  const num = v => Number.isFinite(v) ? v.toLocaleString('zh-CN',{maximumFractionDigits:2}) : '未登记';
  let account = null, busy = false;
  function render(d) {
    if (account !== d.account) {
      get('tx-points-form').reset(); account = d.account;
      get('tx-points-week').value = date(d.week_start);
    }
    const entries = d.entries || [], activity = d.activity?.weeks || [];
    const weeks = [...new Set([d.week_start,...entries.map(e=>e.week_start),...activity.map(e=>e.week_start)])].sort((a,b)=>b-a);
    const total = entries.reduce((sum,e)=>sum+e.trader_points+e.referral_points,0);
    const confirmed = entries.filter(e=>e.final_allocation).reduce((sum,e)=>sum+e.trader_points+e.referral_points,0);
    get('tx-points-summary').textContent = `已登记 ${num(total)} 分 · 其中季末确认 ${num(confirmed)} 分 · ${entries.length} 个周期。积分不计入账户收益。`;
    get('tx-points-rows').innerHTML = weeks.slice(0,52).map(week=>{
      const e = entries.find(x=>x.week_start===week), a=activity.find(x=>x.week_start===week);
      return `<tr><td>${date(week)} → ${date(week+7*86400000)}</td><td>${num(e?.trader_points)}</td><td>${num(e?.referral_points)}</td>
        <td>${e ? (e.final_allocation ? '季末确认 · 手动登记' : '官方估计 · 手动登记') : '待登记'}</td><td>${a ? a.active_days+' 天' : '—'}</td>
        <td>${money(a?.volume)}</td><td>${money(a?.fees)}${a?.incomplete_rows ? ' *' : ''}</td><td>${money(a?.realized)}${a?.incomplete_rows ? ' *' : ''}</td></tr>`;
    }).join('');
  }
  async function refresh() {
    if (busy || document.hidden) return;
    busy=true;
    try {
      const r=await fetch('/api/txflow/points',{cache:'no-store'}), d=await r.json();
      if(!r.ok || !d.ok) throw Error(d.error || '积分记录读取失败');
      render(d); get('tx-points-read-status').textContent='';
    } catch(e) {
      // Never leave evidence for a possibly previous wallet looking current.
      account=null; get('tx-points-rows').innerHTML=''; get('tx-points-summary').textContent='积分记录暂不可用';
      get('tx-points-read-status').textContent=e.message;
    } finally { busy=false; }
  }
  get('tx-points-form').addEventListener('submit',async event=>{
    event.preventDefault();
    const status=get('tx-points-status'), button=get('tx-points-save');
    if(!account) { status.textContent='请先等到账户读取成功。'; return; }
    const week=Date.parse(get('tx-points-week').value+'T00:00:00Z');
    const trader=Number(get('tx-points-trader').value), referral=Number(get('tx-points-referral').value);
    if(!Number.isFinite(week)||new Date(week).getUTCDay()!==1||!Number.isFinite(trader)||!Number.isFinite(referral)||trader<0||referral<0) {
      status.textContent='请选择 UTC 周一，积分填写非负数。';return;
    }
    button.disabled=true;
    let saved=false;
    try {
      const r=await fetch('/api/txflow/points',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({
        account,week_start:week,trader_points:trader,referral_points:referral,final_allocation:get('tx-points-final').checked})});
      const d=await r.json();if(!r.ok||!d.ok) throw Error(d.error||'登记失败');
      saved=true;status.textContent='已保存。同一周再次登记会更新数值，不会累加。';await refresh();
    } catch(e) { status.textContent=saved?'记录已保存，刷新失败，请稍后刷新。':e.message; }
    finally { button.disabled=false; }
  });
  refresh(); setInterval(refresh,60000);
  document.addEventListener('visibilitychange',()=>{if(!document.hidden) refresh();});
})();
