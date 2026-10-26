// Browser regression for the HL carry shadow page.
// Run from repo root with Playwright and Chrome available:
// NODE_PATH=<node_modules> node tools/test-hlcarry-browser.cjs
// All API calls are intercepted; no account or exchange writes are made.
const { chromium } = require('playwright');
const fs = require('node:fs');
const assert = require('node:assert/strict');

const payload = {
  equity: 998.91,
  annualized_pct: null,
  warnings: [],
  state: {
    start_equity: 1000,
    budget_usd: 1000,
    legs: [{
      coin: 'BTC', spot: '@142', notional: 1000, funding_ann: 10.95,
      basis_bp: 8.4, funding_accrued: 0, basis_pnl: 0, fee_paid: 1.09, leg_pnl: -1.09,
    }],
    history: [], log: [], rebalances: [], daily: [],
    params: {top_k: 5, min_funding_ann_pct: 3, min_spot_volume_usd: 1000000, max_leg_weight: .4, payback_days: 30},
  },
};

(async () => {
  const browser = await chromium.launch({headless: true, channel: process.env.BROWSER_CHANNEL || 'chrome'});
  try {
    for (const width of [1440, 390]) {
      const page = await browser.newPage({viewport: {width, height: 1000}});
      const pageErrors = [];
      const openRequests = [];
      page.on('pageerror', e => pageErrors.push(e.message));
      await page.route('**/*', async route => {
        const req = route.request();
        const path = new URL(req.url()).pathname;
        if (path === '/') return route.fulfill({contentType: 'text/html', body: fs.readFileSync('static/index.html', 'utf8')});
        if (path === '/app.js') return route.fulfill({contentType: 'text/javascript', body: fs.readFileSync('static/app.js', 'utf8')});
        if (path === '/api/hlcarry' && req.method() === 'GET') return route.fulfill({json: payload});
        if (path === '/api/hlcarry/open' && req.method() === 'POST') {
          openRequests.push(JSON.parse(req.postData()));
          return route.fulfill({json: {ok: true, legs: 0}});
        }
        return route.fulfill({json: {}});
      });

      await page.goto('http://stars.test/');
      await page.locator('[data-view="hlcarry"]').click();
      await page.waitForSelector('#hc-metrics .metric');
      assert.match(await page.locator('#hc-metrics').innerText(), /影子权益/);

      const input = page.locator('#hc-budget');
      const button = page.locator('#hc-open');
      await input.fill('2000');
      await button.click();
      assert.match(await button.textContent(), /确认重置/);
      await input.fill('2500');
      assert.doesNotMatch(await button.textContent(), /确认重置/, '修改预算应撤销旧确认');
      await button.click();
      await button.click();
      await page.waitForFunction(() => document.querySelector('#hc-budget-status').textContent.includes('暂时空仓'));
      assert.deepEqual(openRequests, [{budget_usd: 2500}], '确认后只提交一次，且使用最新预算');
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), '页面不应横向溢出');
      assert.deepEqual(pageErrors, [], '页面初始化与交互不能抛出浏览器错误');

      await page.close();
      console.log(`✓ HL carry ${width}px: browser startup, reset button and responsive layout`);
    }
  } finally {
    await browser.close();
  }
})().catch(e => { console.error(e); process.exitCode = 1; });
