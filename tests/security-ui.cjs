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
  await vm.runInContext('toggleMonitoring()', context);
  assert.equal(calls.at(-1).name, 'start_monitoring');
  assert.equal(calls.at(-1).args.external, false);
  nodes.get('externalTests').checked = true;
  await vm.runInContext('toggleMonitoring()', context);
  assert.equal(calls.at(-1).args.external, true);
  vm.runInContext('latest = { running: true };', context);
  await vm.runInContext('toggleMonitoring()', context);
  assert.equal(calls.at(-1).name, 'stop_monitoring');
  failStart = true;
  await vm.runInContext('toggleMonitoring()', context);
  assert.equal(nodes.get('startBtn').disabled, false);
  assert.equal(nodes.get('externalTests').disabled, false);
  assert(!calls.some(c => c.name.includes('phone')));
  console.log('PASS: DOM bindings, internal-only default, external opt-in, stop, error recovery, no phone IPC');
})().catch(e => { console.error(e); process.exitCode = 1; });
