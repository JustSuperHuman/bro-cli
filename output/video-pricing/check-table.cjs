const { chromium } = require('C:/Users/james/AppData/Local/PlaywrightMCPBackground/node_modules/playwright');
const fs = require('node:fs');
const path = require('node:path');
(async () => {
  const source=fs.readFileSync(path.join(__dirname,'video-cost-table.html'),'utf8');
  if(source.includes('\\"')||source.includes('\\n')) throw Error('Escaped markup in source');
  const browser=await chromium.launch({headless:true,executablePath:'C:/Users/james/AppData/Local/ms-playwright/chromium_headless_shell-1234/chrome-headless-shell-win64/chrome-headless-shell.exe'});
  try {
    const page=await browser.newPage({viewport:{width:1280,height:1200}});
    const errors=[];
    page.on('pageerror',error=>errors.push(error.message));
    await page.goto('file:///F:/bro-cli/output/video-pricing/table-preview.html');
    const table=page.frameLocator('iframe');
    await table.locator('tbody tr').first().waitFor();
    const expected={
      'Veo 3.1 Lite':['router'],
      'MiniMax H3 Max':['router'],
      'Wan 3.0':['monthly-100','annual-100','annual-75'],
      'Veo 3.1 Fast':['router'],
      'Kling 3.0':['monthly-100','monthly-75','annual-100','annual-75'],
      'MiniMax H3':['monthly-100','monthly-75','annual-100','annual-75'],
      'Grok Imagine 1.5':['router'],
      'Seedance 2.0':['router'],
      'FLUX.3 Video':['router'],
      'Seedance 2.5':['router'],
      'Veo 3.1':['annual-100']
    };
    const results=[];
    for(const [model,columns] of Object.entries(expected)) {
      const actual=await table.locator(`tr[data-model="${model}"] td[data-cheapest="true"]`).evaluateAll(cells=>cells.map(cell=>cell.dataset.column));
      if(JSON.stringify(actual)!==JSON.stringify(columns)) throw Error('Wrong highlighted columns: '+model);
      results.push({model,highlighted:actual});
    }
    if(await table.locator('tbody td').count()!==77) throw Error('Missing price cells');
    if(await table.locator('#vt-scenario').count()!==0) throw Error('Old scenario filter remains');
    const paint=await table.locator('tbody').evaluate(body=>{
        const clear=e=>getComputedStyle(e).backgroundColor==='rgba(0, 0, 0, 0)';
        const rowBackgrounds=[...body.querySelectorAll('tr')].every(clear);
        const winners=[...body.querySelectorAll('[data-cheapest="true"] .vt-price-content')].every(e=>!clear(e));
        const others=[...body.querySelectorAll('[data-cheapest="false"] .vt-price-content')].every(clear);
        return {rowBackgrounds,winners,others};
      });
    if(!paint.rowBackgrounds||!paint.winners||!paint.others)throw Error('Highlight escaped cheaper cells');
    const lite50=await table.locator('tr[data-model="Veo 3.1 Lite"] td[data-column="monthly-50"] .vt-amount').textContent();
    const wanRouter=await table.locator('tr[data-model="Wan 3.0"] td[data-column="router"] .vt-amount').textContent();
    if(lite50!=='$0.148'||wanRouter!=='$0.106') throw Error('Incorrect rounding');
    for(const model of ['Kling 3.0','MiniMax H3']) {
      const amount=await table.locator(`tr[data-model="${model}"] td[data-column="monthly-100"] .vt-amount`).textContent();
      if(amount!=='✓ $0.098') throw Error('User-reported price still not highlighted: '+model);
    }
    const sizes=[];
    for(const width of [1280,360]) {
      await page.setViewportSize({width,height:1400});
      const frame=page.frames().find(f=>f.parentFrame());
      const size=await frame.evaluate(()=>({viewport:innerWidth,document:document.documentElement.scrollWidth,rows:document.querySelectorAll('tbody tr').length,rowBackground:getComputedStyle(document.querySelector('tbody tr')).backgroundColor,winnerBackground:getComputedStyle(document.querySelector('td[data-cheapest="true"] .vt-price-content')).backgroundColor}));
      if(size.document>size.viewport+1) throw Error('Page overflows');
      sizes.push(size);
      await page.screenshot({path:path.join(__dirname,`table-${width}.png`),fullPage:true});
    }
    await page.emulateMedia({colorScheme:'dark'});
    await page.setViewportSize({width:1280,height:1200});
    await page.screenshot({path:path.join(__dirname,'table-dark.png'),fullPage:true});
    if(errors.length)throw Error(errors.join('\n'));
    console.log(JSON.stringify({errors,results,sizes},null,2));
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
