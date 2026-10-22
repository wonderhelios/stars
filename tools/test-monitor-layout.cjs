// Browser regression: npm package playwright + installed Chrome required.
// Run from repo root: node tools/test-monitor-layout.cjs
// All requests are fulfilled locally. No real accounts or writes are used.
const { chromium } = require('playwright');
const fs = require('node:fs');
const assert = require('node:assert/strict');
const config = { capital: 2000, leverage: 3, margin_buffer: .9, lookback: 14,
  top_frac: .2, min_vol_usd: 5000000, target_positions: 8, replay_days: 90,
  momentum_weight: 1, dd_weight: 1, upfrac_weight: 1, slippage: .0003 };
const now = Date.now();
const history = [{ts:now-86400000,equity:2000},{ts:now-3600000,equity:2100}];
const live = {account:'test',config,equity:2100,capital_baseline:{account:'test',equity:2000,ts:now-86400000},
  capital_flows:[],history,positions:[],records:[],tp_orders:[],exec_daily:[],
  win_rate_days:50,win_rate_trades:51,win_rate_months:null,win_samples_days:2,win_samples_months:0,win_samples_trades:69};
const shadow = {running:true,config,equity:2100,capital:2000,pnl:100,pnl_pct:5,days_elapsed:90,
  gross_notional:6000,history,positions:[],trades:[]};
(async () => {
  const browser = await chromium.launch({headless:true,channel:process.env.BROWSER_CHANNEL || 'chrome'});
  try {
    const page = await browser.newPage();
    const errors=[];
    page.on('pageerror',e=>errors.push(e.message));
    await page.route('**/*',async route => {
      const path = new URL(route.request().url()).pathname;
      if(route.request().method()!=='GET') { errors.push('Unexpected write'); return route.abort(); }
      if(path==='/')return route.fulfill({contentType:'text/html',body:fs.readFileSync('static/index.html','utf8')});
      if(path==='/app.js')return route.fulfill({contentType:'text/javascript',body:fs.readFileSync('static/app.js','utf8')});
      return route.fulfill({json:path==='/api/live'?live:path==='/api/paper2'?shadow:path==='/api/status'?{cached_coins:10,universe_liquid:10}:{}});
    });
    await page.goto('http://stars.test/');
    await page.waitForSelector('#mo-metrics > .metric');
    for(const width of [1920,1440,1280,1024,768,390,320]) {
      await page.setViewportSize({width,height:1000});
      for(const [view,id] of [['monitor','mo-metrics'],['shadow','sh-metrics']]) {
        await page.locator(`[data-view="${view}"]`).click();
        await page.waitForSelector(`#${id} > .metric`);
        assert.equal(await page.locator(`#${id} > .metric`).count(),3);
        assert.equal(await page.locator(`#${id}`).evaluate(e=>e.parentElement.classList.contains('performance-panel') && !!e.parentElement.querySelector('.equity-card')),true);
        assert.equal(await page.locator(`#view-${view} .account-details`).count(),0);
        assert.equal(await page.locator(`#${id} .account-return`).count(),1);
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
        const details=page.locator('#'+id+'-details');
        const analysis=view==='monitor'?'analysis':'shadow-analysis';
        await page.locator(`[data-open-view="${analysis}"]`).click();
        assert.equal(await page.locator(`#view-${analysis}`).evaluate(e=>e.classList.contains('active')),true);
        assert.equal(await page.locator(view==='monitor'?'#mo-exec-daily':'#sh-trades').isVisible(),true);
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
        const buttons=details.locator('.account-detail-tabs button');
        for(let i=0;i<await buttons.count();i++) {
          await buttons.nth(i).click();
          assert.equal(await details.locator(':scope > .metric-group:not([hidden])').count(),1);
          const clipped=await details.locator('.metric-group:not([hidden]) .v').evaluateAll(es=>es.some(e=>e.scrollWidth>e.clientWidth+1));
          assert.equal(clipped,false);
        }
        // Polling rerenders must preserve the analysis page and selected category.
        const selected=await details.getAttribute('data-active');
        await page.evaluate(view=>view==='monitor'?renderMonitor(lastLiveData):renderShadow(lastShadowData),view);
        assert.equal(await page.locator(`#view-${analysis}`).evaluate(e=>e.classList.contains('active')),true);
        assert.equal(await details.getAttribute('data-active'),selected);
        await page.locator(`[data-open-view="${view}"]`).click();
        assert.equal(await page.locator('#'+id).isVisible(),true);
        await page.evaluate(()=>scrollTo(0,0));
        if(process.env.SCREENSHOT_DIR && [1440,390].includes(width)) {
          fs.mkdirSync(process.env.SCREENSHOT_DIR,{recursive:true});
          await page.screenshot({path:`${process.env.SCREENSHOT_DIR}/${view}-${width}.png`});
        }
        console.log(`✓ ${view} ${width}px: 3 primary metrics, detail tabs and refresh state verified`);
      }
    }
    assert.deepEqual(errors,[]);
  } finally { await browser.close(); }
})().catch(e=>{console.error(e);process.exitCode=1;});
