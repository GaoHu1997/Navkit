// UI regression with explicit preview IPC mocks; no native database writes.
import assert from 'node:assert/strict';
import { chromium } from 'playwright-core';
import { preview } from 'vite';

// Run the production build first; this serves the existing output only.
const server=await preview({configFile:false,root:process.cwd(),preview:{host:'127.0.0.1',port:0},logLevel:'error'});
let browser;
try {
 console.log('Production preview started');
 const address=server.httpServer.address();
 browser=await chromium.launch({executablePath:process.env.BROWSER_EXE||'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',headless:true});
 const page=await browser.newPage({viewport:{width:1400,height:900}});
 const errors=[];
 page.on('pageerror',error=>errors.push(error.message));
 await page.addInitScript(()=>{
  let mock;
  Object.defineProperty(window,'__PREVIEW_MOCK__',{configurable:true,get:()=>mock,set:value=>{
   mock=value;mock['plugin:event|listen']=()=>1;mock['plugin:event|unlisten']=()=>null;
  }});
 });
 const base='http://127.0.0.1:'+address.port;
 await page.route('**/preview.html',async route=>{
  const response=await page.request.get(base+'/index.html');
  const html=(await response.text()).replace('<head>','<head><script src="/preview-bootstrap.js"></script>');
  await route.fulfill({status:200,contentType:'text/html',body:html});
 });
 await page.goto(base+'/preview.html');
 console.log('Production preview page loaded');
 await page.waitForFunction(()=>!!window.__APP__);
 const sql="CREATE VIEW v_readable AS SELECT a.id,a.display_name,COUNT(*) AS total FROM users a LEFT JOIN orders b ON a.id=b.user_id WHERE a.display_name <> 'from  where' GROUP BY a.id,a.display_name ORDER BY total;";
 await page.evaluate(sql=>{
  const app=window.__APP__;
  const tab=app.makeTab('viewdesign','v_readable','c1','navkit','v_readable','view',sql);
  app.tabs.push(tab);app.activeTabId=tab.id;app.renderTabs();
 },sql);
 const editor=()=>page.evaluate(()=>window.__APP__.activeView().getSql());
 const button=page.locator('.tab-toolbar button[data-command="beautify-sql"]');
 assert.equal(await button.count(),1);assert.equal(await button.getAttribute('aria-label'),'美化 SQL');assert(await button.isEnabled());
 await button.click();
 await page.waitForFunction(()=>window.__APP__.activeView().getSql().includes('\n'));
 const formatted=await editor();
 assert.equal(formatted.replace(/\s/g,''),sql.replace(/\s/g,''));
 assert(formatted.includes("'from  where'"));
 assert.equal(await page.evaluate(()=>window.__APP__.activeTab.dirty),true);
 assert.equal(await page.evaluate(()=>window.__APP__.activeView().captureDraft().baseline),sql);
 console.log('PASS native toolbar button formats the view draft and preserves baseline/literal');
 await page.keyboard.press('Control+z');
 await page.waitForFunction(sql=>window.__APP__.activeView().getSql()===sql,sql);
 console.log('PASS Ctrl+Z restores the complete original view definition');
 await page.keyboard.press('F6');
 await page.waitForFunction(()=>window.__APP__.activeView().getSql().includes('\n'));
 assert.equal(await editor(),formatted);
 console.log('PASS F6 uses the design editor rather than a query tab');
 await page.keyboard.press('Control+z');
 await page.waitForFunction(sql=>window.__APP__.activeView().getSql()===sql,sql);
 await page.keyboard.press('Control+l');
 await page.waitForFunction(()=>window.__APP__.activeView().getSql().includes('\n'));
 assert.equal(await editor(),formatted);
 console.log('PASS Ctrl+L formats through the design editor callback');
 await button.click();assert.equal(await editor(),formatted);
 assert.deepEqual(errors,[]);
 await page.screenshot({path:'artifacts/sql-design-beautify.png'});
 console.log('PASS repeat click stays stable, no page errors');
} catch(error) {
 console.error(error);process.exitCode=1;
} finally {
 await browser?.close();await new Promise(resolve=>server.httpServer.close(resolve));
}
