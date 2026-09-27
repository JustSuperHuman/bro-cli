const { chromium } = require('C:/Users/james/AppData/Local/PlaywrightMCPBackground/node_modules/playwright');
const path = require('node:path');
(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: 'C:/Users/james/AppData/Local/ms-playwright/chromium_headless_shell-1234/chrome-headless-shell-win64/chrome-headless-shell.exe' });
  try {
    const page = await browser.newPage({ viewport: { width: 780, height: 1200 } });
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.goto('file:///F:/bro-cli/output/video-pricing/preview.html');
    const frameLocator = page.frameLocator('iframe');
    await frameLocator.locator('.vc-bar').first().waitFor({ timeout: 30000 });
    const frame = page.frames().find(f => f.parentFrame());
    const results = [];
    for (const width of [780, 360, 320]) {
      await page.setViewportSize({ width, height: 1500 });
      await page.waitForTimeout(200);
      results.push(await frame.evaluate(() => {
        const root = document.querySelector('#video-cost-comparison');
        const svg = root.querySelector('svg');
        const bounds = svg.getBoundingClientRect();
        const texts = [...svg.querySelectorAll('text')].filter(e => e.getAttribute('opacity') !== '0').map(e => ({text:e.textContent, box:e.getBoundingClientRect()}));
        const outside = texts.filter(({box}) => box.left < bounds.left - 1 || box.right > bounds.right + 1);
        const collisions = [];
        for(let i=0;i<texts.length;i++) for(let j=i+1;j<texts.length;j++) {
          const a=texts[i],b=texts[j];
          if(a.box.left < b.box.right && a.box.right > b.box.left && a.box.top < b.box.bottom && a.box.bottom > b.box.top) collisions.push([a.text,b.text]);
        }
        return {width:root.clientWidth, viewport:innerWidth, documentWidth:document.documentElement.scrollWidth, overflowElements:[...document.querySelectorAll('*')].filter(e=>e.getBoundingClientRect().right>innerWidth+1).map(e=>({tag:e.tagName,cls:e.className?.baseVal??e.className,right:e.getBoundingClientRect().right})).slice(0,12), bars:root.querySelectorAll('.vc-bar').length, rows:root.querySelectorAll('.vc-row').length, horizontalOverflow:document.documentElement.scrollWidth > innerWidth, outside:outside.map(x=>x.text), collisions};
      }));
      await page.screenshot({ path: path.join(__dirname, `chart-${width}.png`), fullPage: true });
    }
    const toggle = frameLocator.locator('button[data-series="monthly"]');
    await toggle.click();
    await page.waitForTimeout(250);
    const hidden = await frameLocator.locator('.vc-series[data-series="monthly"] rect').evaluateAll(els => els.every(e => +e.getAttribute('width')===0));
    await toggle.click();
    await page.waitForTimeout(250);
    const restored = await frameLocator.locator('.vc-series[data-series="monthly"] rect').evaluateAll(els => els.every(e => +e.getAttribute('width')>0));
    console.log(JSON.stringify({errors, results, toggle:{hidden,restored}},null,2));
    if(errors.length || results.some(r=>r.outside.length||r.collisions.length||r.horizontalOverflow||r.rows!==11||r.bars!==33)||!hidden||!restored) process.exitCode=1;
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode=1; });
