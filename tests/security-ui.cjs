// Runs the actual UI handlers with a mocked Tauri bridge; no network or system changes.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const root = path.join(__dirname, '..');
const html = fs.readFileSync(path.join(root, 'src/index.html'), 'utf8');
const nodes = new Map([...html.matchAll(/id="([^"]+)"/g)].map(m => [m[1], {
  checked: false, disabled: false, textContent: '', hidden: false,
  addEventListener() {},
}]));
const calls = [];
let failStart = false;
const context = vm.createContext({
  document: { getElementById(id) { assert(nodes.has(id), `Missing DOM element ${id}`); return nodes.get(id); } },
  window: {
    addEventListener() {},
    __TAURI__: {
      core: { invoke: async (name, args) => {
        calls.push({ name, args });
        if (name === 'get_snapshot') return new Promise(() => {});
        if (name === 'start_monitoring' && failStart) throw Error('Simulated failure');
        return { running: false };
      } },
      event: { listen() {} },
    },
  },
  setTimeout() { return 1; }, clearTimeout() {},
});
vm.runInContext(fs.readFileSync(path.join(root, 'src/main.js'), 'utf8'), context);
vm.runInContext('render = (s) => { latest = s; }; refresh = async () => {};', context);
(async () => {
  assert(!/<input[^>]*id="externalTests"[^>]*\bchecked/.test(html));
  assert(!/<input[^>]*id="wifiScan"[^>]*\bchecked/.test(html));
  await vm.runInContext('toggleMonitoring()', context);
  assert.equal(calls.at(-1).name, 'start_monitoring');
  assert.equal(calls.at(-1).args.external, false);
  assert.equal(calls.at(-1).args.wifiScan, false);
  // 兩個選項各自獨立：勾外部測試不會順便開 Wi-Fi 掃描
  nodes.get('externalTests').checked = true;
  await vm.runInContext('toggleMonitoring()', context);
  assert.equal(calls.at(-1).args.external, true);
  assert.equal(calls.at(-1).args.wifiScan, false);
  nodes.get('wifiScan').checked = true;
  await vm.runInContext('toggleMonitoring()', context);
  assert.equal(calls.at(-1).args.wifiScan, true);
  vm.runInContext('latest = { running: true };', context);
  await vm.runInContext('toggleMonitoring()', context);
  assert.equal(calls.at(-1).name, 'stop_monitoring');
  failStart = true;
  await vm.runInContext('toggleMonitoring()', context);
  assert.equal(nodes.get('startBtn').disabled, false);
  assert.equal(nodes.get('externalTests').disabled, false);
  assert.equal(nodes.get('wifiScan').disabled, false);
  assert(!calls.some(c => c.name.includes('phone')));

  // 檢測進行中，兩個選項都要鎖住
  context.setOptionsDisabled(true);
  assert.equal(nodes.get('externalTests').disabled, true);
  assert.equal(nodes.get('wifiScan').disabled, true);

  // 附近無線環境卡片：沒啟用整塊隱藏；有資料才顯示，而且用 textContent 填入
  context.renderWifiEnv({ wifi_env: null });
  assert.equal(nodes.get('wifiEnvCard').hidden, true);
  context.renderWifiEnv({ wifi_env: {
    status_text: '擁擠', color: 'poor', detail: '<b>細節</b>', note: null, suggestion: '請資訊人員評估',
  } });
  assert.equal(nodes.get('wifiEnvCard').hidden, false);
  assert.equal(nodes.get('wifiEnvStatus').textContent, '擁擠');
  assert.equal(nodes.get('wifiEnvStatus').className, 'extra-value poor');
  assert.equal(nodes.get('wifiEnvDetail').textContent, '<b>細節</b>');
  assert.equal(nodes.get('wifiEnvNote').hidden, true);
  assert.equal(nodes.get('wifiEnvSuggestion').hidden, false);
  assert.equal(nodes.get('wifiEnvSuggestion').textContent, '建議：請資訊人員評估');
  assert(!/innerHTML/.test(fs.readFileSync(path.join(root, 'src/main.js'), 'utf8')));

  console.log('PASS: DOM bindings, internal-only default, external/wifi-scan opt-in, stop, error recovery, wifi env card, no phone IPC');
})().catch(e => { console.error(e); process.exitCode = 1; });
