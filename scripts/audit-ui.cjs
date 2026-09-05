// Run production UI functions with deterministic DOM/RPC substitutes.
// No private keys, live requests, transactions, or desktop build.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const test = require('node:test');
const assert = require('node:assert/strict');
const source = fs.readFileSync(path.join(__dirname, '../crates/minter-desktop/ui/app.js'), 'utf8');
function extract(name) {
  const start = source.search(new RegExp(`^(?:async )?function ${name}\\(`, 'm'));
  assert(start >= 0, name);
  const end = source.indexOf('\n}', start);
  assert(end > start, name);
  return source.slice(start, end + 2);
}
function environment() {
  let now = 1_000_000;
  const calls = [], timers = [], elements = new Map();
  function element(id) {
    if (!elements.has(id)) elements.set(id, {value: '', options: [{value: ''}], textContent: '', appendChild() {}, classList: {toggle() {}}, addEventListener() {}});
    return elements.get(id);
  }
  const c = vm.createContext({
    Set, Map, Number, String, Date: {now: () => now}, console,
    $: element, document: {querySelectorAll: () => [], createElement: () => ({})},
    addrKey: a => a.toLowerCase(),
    taskModalWalletCache: ['0xaa', '0xbb', '0xcc', '0xdd'].map(address => ({address})),
    taskModalChecked: new Set(), taskModalPreselect: null, taskModalSeeded: false,
    taskWlPhases: [{stageKey: 'SIGNED_PRESALE#1', addresses: ['0xbb']}],
    taskWlAddresses: null, taskWlFilterActive: false, taskWlPhaseLabel: '',
    lastLoadedPhases: {stages: [
      {index: 0, stageIndex: 1, stageType: 'SIGNED_PRESALE'},
      {index: 1, stageIndex: 2, stageType: 'PUBLIC_SALE'}
    ], walletEligibility: {0: {'0xaa': true, '0xbb': false, '0xcc': null}, 1: {}}},
    renderTaskModalWalletList() {}, updateWlBadge() {},
    gasMonitorBusy: false, lastUiStatus: {unlocked: true}, gasMonitorVersion: 0,
    gasMonitorAttemptAt: 0, gasUsdAttemptAt: 0, gasUsdUpdatedAt: 0,
    gasMonitorChain: 'robinhood', gasSnapshot: null, gasUsdPrice: null, gasMonitorTimer: null,
    renderHeaderGas() {}, refreshVisibleTaskGasCost() {},
    localStorage: {setItem() {}},
    setInterval(fn, ms) {timers.push({fn, ms}); return 1;},
    invokeSafe: async (method, args) => {calls.push({method, ...args}); return {chain: args.chain, usdPrice: args.includeUsd ? '2500' : null, updatedAtMs: now};},
  });
  for (const name of ['wlStageKeyOf', 'applyWlForSelectedPhase', 'taskWalletAllowed', 'selectedTaskWallets', 'fillPhaseSelect', 'refreshGasMonitor', 'ensureGasMonitor', 'setGasMonitorChain', 'nativeSymbolForChain']) vm.runInContext(extract(name), c);
  element('wizard-phase').value = '0';
  return {c, calls, timers, element, advance(ms) {now += ms;}};
}
test('fresh per-wallet phase answers override the saved checker, reject false and unknown', () => {
  const {c} = environment(); c.applyWlForSelectedPhase();
  assert.deepEqual([...c.taskModalChecked], ['0xaa']);
  assert.deepEqual(Array.from(c.selectedTaskWallets()), ['0xaa']);
  assert.equal(c.taskWalletAllowed('0xbb'), false);
  assert.equal(c.taskWalletAllowed('0xcc'), false);
  assert.equal(c.taskWalletAllowed('0xdd'), false);
});
test('public allows every wallet, switching back restores presale-only selection', () => {
  const {c, element} = environment();
  c.applyWlForSelectedPhase(); element('wizard-phase').value = '1'; c.applyWlForSelectedPhase();
  assert.equal(c.selectedTaskWallets().length, 4);
  element('wizard-phase').value = '0'; c.applyWlForSelectedPhase();
  assert.deepEqual(Array.from(c.selectedTaskWallets()), ['0xaa']);
});
test('programmatic selection followed by filtering selects exactly the recommended phase', () => {
  const {c} = environment(); c.fillPhaseSelect(c.lastLoadedPhases.stages, 0, null); c.applyWlForSelectedPhase();
  assert.deepEqual(Array.from(c.selectedTaskWallets()), ['0xaa']);
});
test('manually injected ineligible checked keys cannot get into a saved task', () => {
  const {c} = environment(); c.taskModalChecked = new Set(['0xaa', '0xbb', '0xcc']);
  assert.deepEqual(Array.from(c.selectedTaskWallets()), ['0xaa']);
});
test('one simulated hour: 240 snapshots, 12 prices; repeated UI refresh adds no requests', async () => {
  const {c, calls, advance, timers} = environment();
  for (let i = 0; i < 240; i++) {
    await c.refreshGasMonitor(); c.ensureGasMonitor(); c.ensureGasMonitor(); advance(15000);
  }
  assert.equal(calls.length, 240);
  assert.equal(calls.filter(x => x.includeUsd).length, 12);
  assert.equal(timers.length, 1);
});
test('missing price does not retry each gas tick; locked app makes no requests', async () => {
  const {c, calls, advance} = environment();
  c.invokeSafe = async (method, args) => {calls.push(args); return {};};
  for (let i = 0; i < 20; i++) {await c.refreshGasMonitor(); advance(15000);}
  assert.equal(calls.filter(x => x.includeUsd).length, 1);
  c.lastUiStatus.unlocked = false; await c.refreshGasMonitor(); assert.equal(calls.length, 20);
});
test('switching network discards a late response and the old currency price', async () => {
  const {c} = environment(); let resolve;
  c.gasUsdPrice = '2500'; c.invokeSafe = () => new Promise(r => resolve = r);
  const pending = c.refreshGasMonitor(); c.setGasMonitorChain('polygon');
  resolve({chain: 'robinhood', usdPrice: '2500'}); await pending;
  assert.equal(c.gasSnapshot, null); assert.equal(c.gasUsdPrice, null);
});
test('actual phase-load handler guards collection version and reapplies filtering', () => {
  const start = source.indexOf('$("btn-load-phases")?.addEventListener("click", async () => {');
  const handler = source.slice(start, source.indexOf('\n});', start));
  assert.match(handler, /version !== phaseLoadVersion/);
  assert.match(handler, /fillPhaseSelect[\s\S]*applyWlForSelectedPhase\(\)/);
  assert.match(handler, /taskModalWalletCache\.map/);
});

function installPhaseHandler(env) {
  const {c, element} = env;
  c.phaseLoadVersion = 0; c.wlDebounce = null; c.clearTimeout = () => {};
  c.loadedPhaseInput = 'old'; c.resetTaskCostQuote = () => {};
  c.getLang = () => 'en'; c.refreshTaskCostQuote = async () => null;
  let handler;
  element('btn-load-phases').addEventListener = (event, fn) => {handler = fn;};
  const start = source.indexOf('$("btn-load-phases")?.addEventListener("click", async () => {');
  vm.runInContext(source.slice(start, source.indexOf('\n});', start) + 4), c);
  return handler;
}
test('real load callback clears old phases immediately, then applies fresh wallet answers', async () => {
  const env = environment(), {c, element} = env;
  const result = {...c.lastLoadedPhases, chain: 'robinhood', slug: 'new', name: 'New', recommendedIndex: 0, successfulWallets: 3, walletCount: 4};
  let finish; c.invoke = () => new Promise(resolve => {finish = resolve;});
  const handler = installPhaseHandler(env); element('wizard-slug').value = 'new';
  const pending = handler(); assert.equal(c.lastLoadedPhases, null); assert.equal(c.loadedPhaseInput, null);
  finish(result); await pending;
  assert.equal(element('wizard-msg').textContent, 'Phases loaded');
  assert.equal(c.loadedPhaseInput, 'new'); assert.deepEqual(Array.from(c.selectedTaskWallets()), ['0xaa']);
});
test('real load callback ignores response belonging to an old collection', async () => {
  const env = environment(), {c, element} = env;
  let finish; c.invoke = () => new Promise(resolve => {finish = resolve;});
  const handler = installPhaseHandler(env); element('wizard-slug').value = 'old';
  const pending = handler(); element('wizard-slug').value = 'new'; c.phaseLoadVersion++;
  finish({slug: 'old'}); await pending;
  assert.equal(c.lastLoadedPhases, null); assert.equal(c.loadedPhaseInput, null);
});
test('Arc native currency is USDC; existing Ethereum networks remain ETH', () => {
  const {c} = environment();
  assert.equal(c.nativeSymbolForChain('arc_testnet'), 'USDC');
  assert.equal(c.nativeSymbolForChain('robinhood'), 'ETH');
});
