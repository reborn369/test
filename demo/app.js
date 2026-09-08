const icons = {
  overview: '<path d="M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z"/>',
  tasks: '<path d="M6 3h12v18l-6-3-6 3z"/>',
  code: '<path d="m8 9-3 3 3 3m8-6 3 3-3 3m-2-9-4 12"/>',
  wallet: '<path d="M4 6h14a2 2 0 0 1 2 2v10H6a2 2 0 0 1-2-2zm0 3h16m-4 4h2"/>',
  disperse: '<path d="M12 4v6m0 0-3-3m3 3 3-3M5 14v5h14v-5M8 14l-3 3m11-3 3 3"/>',
  sweep: '<path d="M5 18h14M8 18l2-11h4l2 11M7 13h10"/>',
  network: '<circle cx="12" cy="12" r="3"/><path d="M12 3v6m0 6v6M3 12h6m6 0h6M5.6 5.6l4.2 4.2m4.4 4.4 4.2 4.2m0-12.8-4.2 4.2m-4.4 4.4-4.2 4.2"/>',
  proxy: '<path d="M4 7h16M7 4v6m10-6v6M6 14h12v6H6z"/>',
  check: '<path d="M5 12.5 9.5 17 19 7"/>',
  layers: '<path d="m12 3 9 5-9 5-9-5zm-9 10 9 5 9-5m-18 5 9 5 9-5"/>',
  history: '<path d="M4 12a8 8 0 1 0 2.3-5.7L4 8.5M4 4v4.5h4.5M12 7v5l3 2"/>',
  settings: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1A1.7 1.7 0 0 0 9 4.6a1.7 1.7 0 0 0 1-1.6v-.2h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z"/>',
  chevron: '<path d="m9 10 3 3 3-3"/>', search: '<circle cx="11" cy="11" r="7"/><path d="m16 16 4 4"/>', bell: '<path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9m-8 12h2"/>',
  menu: '<path d="M4 7h16M4 12h16M4 17h16"/>', tick: '<path d="m6 12 4 4 8-8"/>', plus: '<path d="M12 5v14M5 12h14"/>',
  play: '<path d="m8 5 11 7-11 7z"/>', filter: '<path d="M4 6h16M7 12h10m-7 6h4"/>', more: '<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
  bolt: '<path d="m13 2-8 12h7l-1 8 8-12h-7z"/>', close: '<path d="m6 6 12 12M18 6 6 18"/>', info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5m0-8h.01"/>',
  copy: '<rect x="8" y="8" width="11" height="11" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>', external: '<path d="M14 5h5v5m0-5-9 9"/><path d="M19 13v6H5V5h6"/>',
  warning: '<path d="M12 3 2.8 20h18.4zm0 6v5m0 3h.01"/>', success: '<circle cx="12" cy="12" r="9"/><path d="m8 12 3 3 5-6"/>', upload: '<path d="M12 16V4m0 0L8 8m4-4 4 4M5 15v5h14v-5"/>',
};

const svg = (name, className = "") => `<svg class="${className}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${icons[name] || icons.overview}</svg>`;
document.querySelectorAll('[data-icon]').forEach((node) => node.insertAdjacentHTML('afterbegin', svg(node.dataset.icon)));

const pageMeta = {
  overview: ['Workspace', 'Overview'], tasks: ['Minting', 'Mint tasks'], raw: ['Minting', 'Raw mint'], wallets: ['Wallets & funds', 'Wallets'],
  disperse: ['Wallets & funds', 'Disperse'], sweep: ['Wallets & funds', 'Sweep'], rpcs: ['Network & access', 'RPC health'], proxies: ['Network & access', 'Proxies'],
  wl: ['Network & access', 'WL check'], multicall: ['Tools', 'Multicall'], history: ['Tools', 'History'], settings: ['System', 'Settings'],
};

const wallets = [
  ['Alpha 01','0x71F4…2A9C','1.842','$5,618'], ['Alpha 02','0x98C2…7D11','0.614','$1,874'], ['Alpha 03','0x42D8…A512','0.208','$635'],
  ['Alpha 04','0xB91A…10FE','2.041','$6,225'], ['Alpha 05','0x12F3…9C40','0.093','$284'], ['Alpha 06','0xD77E…F283','0.731','$2,230'],
  ['Reserve 01','0x8AA1…111D','4.205','$12,827'], ['Reserve 02','0xCC23…880A','1.106','$3,374'],
];
const tasks = [
  ['Ethereal Fragments','Ethereum','0xA4e2…9B11','24','Live','58%'], ['Based Punks','Base','0x918c…F009','12','Ready','—'], ['Robinhood Early','Robinhood','0x7B2a…332E','8','Scheduled','18:40'],
  ['Memory Blocks','Ethereum','0x1D02…BA40','16','Paused','42%'], ['Test Contract','Base','0xC241…0F19','4','Draft','—'],
];

function panel(title, body, extra = '', icon = '') {
  return `<section class="panel"><header class="panel-header"><div class="panel-title">${icon ? svg(icon) : ''}<h3>${title}</h3></div>${extra}</header>${body}</section>`;
}
function intro(title, description, actions = '') {
  return `<div class="page-intro"><div><h2>${title}</h2><p>${description}</p></div>${actions ? `<div class="actions">${actions}</div>` : ''}</div>`;
}
const button = (label, icon = '', classes = '', action = '') => `<button class="button ${classes}" ${action ? `data-action="${action}"` : ''}>${icon ? svg(icon) : ''}${label}</button>`;
const status = (text, type = '') => `<span class="status ${type}">${text}</span>`;

function overviewPage() {
  const executionRows = [
    ['Alpha 01','Confirmed','Block 22,041,802','0x93d…fe2'], ['Alpha 02','Confirmed','Block 22,041,805','0x118…a91'], ['Alpha 03','Pending','Awaiting receipt','0x6c2…d70'], ['Alpha 04','Submitting','Nonce 184','—'],
  ].map(([w,s,d,tx]) => `<tr><td><span class="mono">${w}</span></td><td>${status(s, s === 'Confirmed' ? 'success' : s === 'Pending' ? 'pending' : 'live')}</td><td>${d}</td><td class="mono">${tx}</td></tr>`).join('');
  return `<div class="page">${intro('Good evening, Operator','One calm surface for every active mint, wallet and network signal.', button('New mint task','plus','primary','new-task'))}
    <section class="metric-grid">
      <div class="metric"><span class="metric-label">Active tasks ${svg('bolt')}</span><strong class="metric-value">03</strong><span class="metric-meta">1 executing · 2 armed</span></div>
      <div class="metric"><span class="metric-label">Wallet balance ${svg('wallet')}</span><strong class="metric-value">12.84 ETH</strong><span class="metric-meta">Across 24 burner wallets <i class="trend">+2.1%</i></span></div>
      <div class="metric"><span class="metric-label">Confirmed today ${svg('success')}</span><strong class="metric-value">46</strong><span class="metric-meta">95.8% execution success</span></div>
      <div class="metric"><span class="metric-label">Current gas ${svg('network')}</span><strong class="metric-value">12.4</strong><span class="metric-meta">Gwei · low congestion</span></div>
    </section>
    <div class="dashboard-grid"><div class="stack">
      ${panel('Live execution', `<div class="execution-summary"><div><div class="execution-name"><span class="collection-avatar">EF</span><div><h4>Ethereal Fragments · Public mint</h4><p>Ethereum · 0xA4e2…9B11 · 0.024 ETH</p></div></div><div class="progress-track"><span></span></div></div><div class="execution-count"><strong>14 / 24</strong><span>receipts confirmed</span></div></div><div class="table-wrap"><table class="data-table"><thead><tr><th>Wallet</th><th>Status</th><th>Detail</th><th>Transaction</th></tr></thead><tbody>${executionRows}</tbody></table></div>`, '<span class="live-label">Live</span>')}
      ${panel('Execution queue', `<div class="table-wrap"><table class="data-table"><thead><tr><th>Task</th><th>Network</th><th>Wallets</th><th>Trigger</th><th>Status</th></tr></thead><tbody><tr><td>Based Punks</td><td>Base</td><td>12</td><td>Manual</td><td>${status('Ready','success')}</td></tr><tr><td>Robinhood Early</td><td>Robinhood</td><td>8</td><td>18:40 UTC</td><td>${status('Scheduled','pending')}</td></tr><tr><td>Memory Blocks</td><td>Ethereum</td><td>16</td><td>Paused</td><td>${status('Paused')}</td></tr></tbody></table></div>`, button('View tasks','','ghost small','go-tasks'))}
    </div><div class="stack">
      ${panel('Network pulse', `<div class="panel-body network-list"><div class="network-row"><span><i class="network-orb"></i><span><strong>Ethereum</strong><small>3 / 3 endpoints healthy</small></span></span><span class="latency">84 ms</span></div><div class="network-row"><span><i class="network-orb base"></i><span><strong>Base</strong><small>2 / 2 endpoints healthy</small></span></span><span class="latency">61 ms</span></div><div class="network-row"><span><i class="network-orb robinhood"></i><span><strong>Robinhood</strong><small>2 / 2 endpoints healthy</small></span></span><span class="latency">93 ms</span></div></div>`, '<span class="status success">Operational</span>')}
      ${panel('Recent activity', `<div class="panel-body activity-list"><div class="activity"><span class="activity-icon">${svg('success')}</span><div><p>12 receipts confirmed for Ethereal Fragments</p><small>Task · Ethereum</small></div><time>18:22</time></div><div class="activity"><span class="activity-icon">${svg('wallet')}</span><div><p>0.48 ETH dispersed across 8 wallets</p><small>Funds · Base</small></div><time>17:58</time></div><div class="activity"><span class="activity-icon">${svg('network')}</span><div><p>Configured fallback retried after primary latency spike</p><small>Network · Ethereum</small></div><time>17:41</time></div></div>`, button('All history','','ghost small','go-history'))}
    </div></div></div>`;
}

function tasksPage() {
  const rows = tasks.map(([name,network,contract,count,state,detail]) => `<tr data-task-state="${state.toLowerCase()}"><td><div class="wallet-cell"><span class="collection-avatar">${name.split(' ').map(x=>x[0]).join('').slice(0,2)}</span><span><strong>${name}</strong><small>${contract}</small></span></div></td><td>${network}</td><td>${count}</td><td>${state === 'Live' ? status(state,'live') : state === 'Ready' ? status(state,'success') : state === 'Scheduled' ? status(state,'pending') : status(state)}</td><td class="mono">${detail}</td><td><button class="row-button" data-action="task-details" aria-label="Task details">${svg('more')}</button></td></tr>`).join('');
  return `<div class="page">${intro('Mint tasks','Prepare, simulate and control mint executions without losing operational context.', `${button('Import task','upload','ghost','toast-import')}${button('New task','plus','primary','new-task')}`)}
    <section class="panel"><div class="toolbar"><div class="toolbar-group"><div class="search-field">${svg('search')}<input id="task-search" aria-label="Search task or contract" placeholder="Search task or contract" /></div><div class="segmented" data-filter="tasks"><button class="active" data-value="all">All</button><button data-value="live">Live</button><button data-value="ready">Ready</button><button data-value="scheduled">Scheduled</button></div></div>${button('Filters','filter','ghost small','toast-filter')}</div><div class="table-wrap"><table class="data-table"><thead><tr><th>Task</th><th>Network</th><th>Wallets</th><th>Status</th><th>Progress / trigger</th><th></th></tr></thead><tbody id="task-table">${rows}</tbody></table></div><div class="table-scroll-hint">Swipe horizontally to inspect every column →</div></section></div>`;
}

function walletsPage() {
  const rows = wallets.map(([name,address,balance,usd], i) => `<tr><td><input class="checkbox wallet-check" type="checkbox" aria-label="Select ${name}" /></td><td><button class="row-button wallet-open" data-wallet="${i}" style="padding:0"><div class="wallet-cell"><span class="wallet-ident">${String(i+1).padStart(2,'0')}</span><span><strong>${name}</strong><small>${address}</small></span></div></button></td><td class="mono">${balance} ETH</td><td>${usd}</td><td>${i === 4 ? status('Low balance','pending') : status('Ready','success')}</td><td><button class="row-button wallet-open" data-wallet="${i}" aria-label="Wallet details">${svg('more')}</button></td></tr>`).join('');
  return `<div class="page">${intro('Wallets','Burner wallets remain local, encrypted and grouped around the work you need to do.', `${button('Import','upload','ghost','toast-import')}${button('Create wallets','plus','primary','create-wallets')}`)}
    <section class="panel"><div class="stat-strip"><div class="mini-stat"><span>Total balance</span><strong>12.84 ETH</strong></div><div class="mini-stat"><span>Ready wallets</span><strong>23 / 24</strong></div><div class="mini-stat"><span>Portfolio estimate</span><strong>$39,162</strong></div></div><div class="toolbar"><div class="toolbar-group"><div class="search-field">${svg('search')}<input id="wallet-search" aria-label="Search wallet name or address" placeholder="Search name or address" /></div><div class="segmented"><button class="active" data-wallet-filter="all">All</button><button data-wallet-filter="ready">Ready</button><button data-wallet-filter="low">Low balance</button></div></div><span id="wallet-selection" class="status">0 selected</span></div><div class="table-wrap"><table class="data-table"><thead><tr><th><input id="wallet-all" class="checkbox" type="checkbox" aria-label="Select all" /></th><th>Wallet</th><th>Balance</th><th>USD estimate</th><th>Status</th><th></th></tr></thead><tbody id="wallet-table">${rows}</tbody></table></div><div class="table-scroll-hint">Swipe horizontally to inspect every column →</div></section></div>`;
}

function rawPage() {
  return `<div class="page">${intro('Raw mint','Build an explicit contract call, inspect the decoded payload and simulate before going live.', status('Advanced mode','pending'))}<div class="form-layout"><section class="panel">
    <div class="form-section"><div class="section-heading"><h3>Contract call</h3><p>Choose a safe preset or switch to Custom for an arbitrary function signature and parameters.</p></div><div class="segmented raw-presets" data-raw-presets aria-label="Contract call preset"><button class="active" data-raw-preset="auto" aria-pressed="true">Auto</button><button data-raw-preset="quantity" aria-pressed="false">mint(qty)</button><button data-raw-preset="simple" aria-pressed="false">mint()</button><button data-raw-preset="custom" aria-pressed="false">Custom</button></div><div class="field-grid" style="margin-top:14px"><div class="field"><label>Network</label><select><option>Ethereum Mainnet</option><option>Base</option><option>Robinhood</option></select></div><div class="field"><label>Contract address</label><input value="0xA4e24C9fD77bA1809B11262c8Ed4a5dE079B11" /></div></div><div class="field-grid" id="raw-standard-call" style="margin-top:14px"><div class="field"><label>Detected function</label><select id="raw-method"><option>mint(uint256 quantity)</option><option>publicMint(uint256 quantity)</option><option>allowlistMint(bytes32[] proof)</option></select></div><div class="field"><label>Quantity</label><input type="number" value="1" min="1" /></div></div><div class="field-grid hidden" id="raw-custom-call" style="margin-top:14px"><div class="field"><label>Discovered function</label><select><option>Type manually…</option><option>mint(uint256)</option><option>publicMint(uint256)</option></select><button class="button ghost small" type="button" data-action="raw-discover">${svg('bolt')}Discover functions</button></div><div class="field"><label>Function signature</label><input value="mint(uint256)" placeholder="mint(uint256)" /></div><div class="field full"><label>Parameters</label><textarea rows="2" placeholder="Comma-separated, e.g. 1">1</textarea></div><div class="field"><label>Value mode</label><select><option>Fixed value per wallet</option><option>Multiply value by quantity</option></select></div></div><div class="field-grid" style="margin-top:14px"><div class="field"><label>Value per wallet</label><input value="0.024" /><span class="field-hint">ETH · excludes network fee</span></div><div class="field"><label>Wallet group</label><select><option>Alpha group · 6 wallets</option><option>Reserve group · 2 wallets</option></select></div></div></div>
    <div class="form-section"><div class="section-heading"><h3>Calldata preview</h3><p>Decoded from the selected function and arguments.</p></div><div class="code-block"><span style="color:#657086">Function</span>  mint(uint256 quantity)\n<span style="color:#657086">Selector</span>  0xa0712d68\n<span style="color:#657086">Args</span>      quantity: 1\n<span style="color:#657086">Calldata</span>  0xa0712d68000000000000000000000000…0001</div></div>
    <div class="form-section"><div class="section-heading"><h3>Timing, gas and delivery</h3><p>Advanced controls remain visible without entering the hot execution summary.</p></div><div class="field-grid"><div class="field"><label>Execution timing</label><select><option>Send now</option><option>Scheduled timestamp</option><option>Wait for contract phase</option></select></div><div class="field"><label>Timeout</label><select><option>30 minutes</option><option>10 minutes</option><option>60 minutes</option></select></div><div class="field"><label>Priority fee</label><input placeholder="auto" /></div><div class="field"><label>Max fee</label><input placeholder="auto" /></div><div class="field"><label>Gas limit</label><input value="650000" /></div><div class="field"><label>Gas multiplier</label><input placeholder="auto" /></div><div class="field full"><label>Selected wallets</label><div class="wallet-pills">${['Alpha 01 · 1.842 ETH','Alpha 02 · 0.614 ETH','Alpha 03 · 0.208 ETH','Alpha 04 · 2.041 ETH','Alpha 05 · 0.093 ETH','Alpha 06 · 0.731 ETH'].map(x=>`<span class="wallet-pill">${x}</span>`).join('')}</div></div><label class="option-line"><input type="checkbox" checked />Only wallets with sufficient balance</label><label class="option-line"><input type="checkbox" />Dry run — pre-sign only</label><label class="option-line"><input type="checkbox" />Flashbots delivery · Ethereum send-now</label></div></div>
    <div class="form-section"><div class="notice">${svg('warning')}<span>Raw execution bypasses marketplace metadata checks. Review the target, value and decoded method before simulation.</span></div></div></section>
    <div class="stack"><section class="panel"><header class="panel-header"><div class="panel-title"><h3>Execution preview</h3></div>${status('Simulated','success')}</header><div class="panel-body"><div class="summary-list"><div class="summary-row"><span>Wallets</span><strong>6</strong></div><div class="summary-row"><span>Mint value</span><strong>0.144 ETH</strong></div><div class="summary-row"><span>Estimated gas</span><strong>0.0082 ETH</strong></div><div class="summary-row"><span>Max fee</span><strong>16.2 Gwei</strong></div><div class="summary-total"><div class="summary-row"><span>Estimated total</span><strong>0.1522 ETH</strong></div></div></div></div><div class="modal-actions">${button('Simulate again','bolt','ghost','simulate')}${button('Review LIVE run','play','primary','raw-live')}</div></section>
    ${panel('Safety checks','<div class="panel-body summary-list"><div class="summary-row"><span>Contract bytecode</span><span class="status success">Found</span></div><div class="summary-row"><span>Balance coverage</span><span class="status success">Passed</span></div><div class="summary-row"><span>Method decoded</span><span class="status success">Passed</span></div><div class="summary-row"><span>RPC agreement</span><span class="status success">3 / 3</span></div></div>')}</div></div></div>`;
}

function networkPage() {
  const endpoints = [
    ['Primary Ethereum','Alchemy','Ethereum','Healthy','84 ms',[7,11,9,14,12,16,13,18]], ['Fallback Public','Public','Ethereum','Healthy','112 ms',[5,8,7,12,9,11,13,12]], ['Base Primary','QuickNode','Base','Healthy','61 ms',[9,14,12,16,18,13,19,17]], ['Robinhood Primary','Custom','Robinhood','Healthy','93 ms',[6,9,11,8,13,12,15,14]], ['Archive Ethereum','Custom','Ethereum','Degraded','684 ms',[18,10,20,7,16,5,14,8]],
  ];
  const rows = endpoints.map(([name,provider,network,state,latency,bars]) => `<tr><td><strong>${name}</strong></td><td>${provider}</td><td>${network}</td><td>${status(state,state === 'Healthy'?'success':'pending')}</td><td class="mono">${latency}</td><td><span class="spark ${state === 'Healthy'?'':'warn'}">${bars.map(h=>`<i style="height:${h}px"></i>`).join('')}</span></td><td><button class="row-button" data-action="rpc-test">${svg('bolt')}</button></td></tr>`).join('');
  return `<div class="page">${intro('RPC health','Monitor endpoint agreement, latency and fallback readiness across every supported chain.', `${button('Run all checks','bolt','ghost','rpc-all')}${button('Add endpoint','plus','primary','add-rpc')}`)}<section class="metric-grid"><div class="metric"><span class="metric-label">Healthy endpoints</span><strong class="metric-value">7 / 8</strong><span class="metric-meta">All primary routes available</span></div><div class="metric"><span class="metric-label">Median latency</span><strong class="metric-value">84 ms</strong><span class="metric-meta"><i class="trend">−12 ms</i> over 15 minutes</span></div><div class="metric"><span class="metric-label">Block agreement</span><strong class="metric-value">100%</strong><span class="metric-meta">No chain divergence detected</span></div><div class="metric"><span class="metric-label">Requests today</span><strong class="metric-value">18.2K</strong><span class="metric-meta">0.04% failed requests</span></div></section><section class="panel" style="margin-top:16px"><div class="tabs"><button class="active">Endpoints</button><button>Request log</button><button>Routing rules</button></div><div class="table-wrap"><table class="data-table"><thead><tr><th>Endpoint</th><th>Provider</th><th>Network</th><th>Status</th><th>Latency</th><th>Last 8 checks</th><th></th></tr></thead><tbody>${rows}</tbody></table></div></section></div>`;
}

function toolFormPage(route) {
  if (route === 'sweep') {
    return `<div class="page">${intro('Sweep assets','Consolidate native balances or NFTs while preserving explicit source, reserve and destination controls.')}<section class="panel"><div class="tabs" data-tabs="sweep"><button class="active" data-tab="native">Native token</button><button data-tab="nft">NFT collection</button></div><div class="settings-pane" data-pane="native"><div class="form-layout" style="padding:16px"><div><div class="field-grid"><div class="field"><label>Network</label><select><option>Ethereum</option><option>Base</option><option>Polygon</option></select></div><div class="field"><label>Destination</label><select><option>Reserve 01 · 0x8AA1…111D</option></select></div><div class="field"><label>Source wallets</label><select><option>Alpha group · 6 wallets</option></select></div><div class="field"><label>Keep per wallet</label><input value="0.006" /></div><div class="field full"><label>Selected sources</label><div class="wallet-pills">${['Alpha 01','Alpha 02','Alpha 03','Alpha 04','Alpha 05','Alpha 06'].map(x=>`<span class="wallet-pill">${x}</span>`).join('')}</div></div><label class="option-line"><input type="checkbox" checked />Dry run first — calculate recoverable amount without signing</label></div></div><div>${panel('Sweep preview','<div class="panel-body summary-list"><div class="summary-row"><span>Recoverable</span><strong>3.122 ETH</strong></div><div class="summary-row"><span>Gas reserve</span><strong>0.036 ETH</strong></div><div class="summary-row"><span>Destination</span><strong>0x8AA1…111D</strong></div></div><div class="modal-actions">'+button('Preview native sweep','play','primary','preview-operation')+'</div>')}</div></div></div><div class="settings-pane hidden" data-pane="nft"><div class="form-layout" style="padding:16px"><div class="field-grid"><div class="field"><label>Network</label><select><option>Ethereum</option><option>Base</option></select></div><div class="field"><label>NFT contract</label><input value="0xA4e24C9fD77bA1809B11" /></div><div class="field"><label>Source wallets</label><select><option>Alpha group · 6 wallets</option></select></div><div class="field"><label>Destination</label><select><option>Reserve 01 · 0x8AA1…111D</option></select></div></div><div>${panel('NFT scan','<div class="panel-body summary-list"><div class="summary-row"><span>Matching tokens</span><strong>18</strong></div><div class="summary-row"><span>Source wallets</span><strong>6</strong></div><div class="summary-row"><span>Standard</span><strong>ERC-721</strong></div></div><div class="modal-actions">'+button('Preview NFT sweep','play','primary','preview-operation')+'</div>')}</div></div></div></section></div>`;
  }
  if (route === 'multicall') {
    return `<div class="page">${intro('Multicall composer','Batch several contract calls from one source through Multicall3.',button('Add call','plus','ghost','add-call'))}<div class="notice" style="margin-bottom:16px">${svg('warning')}<span>Target contracts see <strong>msg.sender = Multicall3</strong>, not your EOA. Use Raw Mint for multi-wallet mint execution.</span></div><div class="form-layout"><section class="panel"><div class="form-section"><div class="field-grid"><div class="field"><label>Network</label><select><option>Ethereum</option><option>Base</option></select></div><div class="field"><label>Source wallet</label><select><option>Alpha 01 · 0x71F4…2A9C</option></select></div><div class="field full"><label>Multicall3 override</label><input placeholder="Default deployment for selected network" /></div></div></div><div class="form-section"><div class="section-heading"><h3>Calls</h3><p>Each row can use a function signature and arguments or raw calldata.</p></div><div class="call-list" id="call-list">${callRow(1,'balanceOf(address)','0x71F4…2A9C')}${callRow(2,'ownerOf(uint256)','1842')}</div></div><div class="modal-actions">${button('Add call','plus','ghost','add-call')}</div></section><section class="panel"><header class="panel-header"><div class="panel-title"><h3>Batch preview</h3></div>${status('2 calls')}</header><div class="panel-body summary-list"><div class="summary-row"><span>Allow failures</span><strong>No</strong></div><div class="summary-row"><span>Attached value</span><strong>0 ETH</strong></div><div class="summary-row"><span>Estimated gas</span><strong>94,200</strong></div></div><div class="modal-actions">${button('Dry run','bolt','ghost','simulate')}${button('Review batch','play','primary','preview-operation')}</div></section></div></div>`;
  }
  const configs = {
    disperse: ['Disperse funds','Fund selected burner wallets through one deliberate distribution plan.','Source wallet','Reserve 01 · 0x8AA1…111D','Recipients','Alpha group · 6 wallets','Amount per wallet','0.08','Estimated total','0.486 ETH','Preview distribution'],
    sweep: ['Sweep funds','Consolidate residual native balances while preserving a configurable gas reserve.','Destination','Reserve 01 · 0x8AA1…111D','Source wallets','Alpha group · 6 wallets','Keep per wallet','0.006','Estimated recovery','3.122 ETH','Preview sweep'],
    multicall: ['Multicall','Prepare the same read or write call across a selected set of contracts.','Network','Ethereum Mainnet','Call targets','6 contracts loaded','Function','balanceOf(address)','Estimated calls','6','Simulate calls'],
  };
  const c = configs[route];
  return `<div class="page">${intro(c[0],c[1])}<div class="form-layout"><section class="panel"><div class="form-section"><div class="section-heading"><h3>Operation setup</h3><p>Configure the operation, then inspect the complete execution plan.</p></div><div class="field-grid"><div class="field"><label>${c[2]}</label><select><option>${c[3]}</option></select></div><div class="field"><label>${c[4]}</label><select><option>${c[5]}</option></select></div><div class="field"><label>${c[6]}</label><input value="${c[7]}" /></div><div class="field"><label>Network fee policy</label><select><option>Automatic · recommended</option><option>Manual EIP-1559</option></select></div><div class="field full"><label>Operator note</label><textarea placeholder="Optional local note for this run"></textarea></div></div></div></section><section class="panel"><header class="panel-header"><div class="panel-title"><h3>Plan summary</h3></div>${status('Draft')}</header><div class="panel-body summary-list"><div class="summary-row"><span>Network</span><strong>Ethereum</strong></div><div class="summary-row"><span>Wallets / calls</span><strong>6</strong></div><div class="summary-row"><span>${c[8]}</span><strong>${c[9]}</strong></div><div class="summary-row"><span>Estimated gas</span><strong>0.006 ETH</strong></div><div class="summary-total"><div class="summary-row"><span>Backend calls</span><strong>0 · demo only</strong></div></div></div><div class="modal-actions">${button(c[10],'play','primary','preview-operation')}</div></section></div></div>`;
}

function callRow(index, signature = '', args = '') {
  return `<div class="call-row"><span class="call-number">${index}</span><div class="field"><label>Target / function</label><input value="${signature}" placeholder="0x target · function(args)" /></div><div class="field"><label>Parameters or calldata</label><input value="${args}" placeholder="comma-separated or 0x…" /></div><button class="row-button" data-action="remove-call" aria-label="Remove call">${svg('close')}</button></div>`;
}

function wlPage() {
  return `<div class="page">${intro('Allowlist check','Resolve OpenSea drop phases and check eligibility across selected burner wallets.')}<div class="form-layout"><section class="panel"><div class="form-section"><div class="section-heading"><h3>Collection and wallets</h3><p>The production flow loads the collection by slug, selects wallets and streams per-stage results.</p></div><div class="field-grid"><div class="field full"><label>Collection slug or OpenSea URL</label><input value="ethereal-fragments" placeholder="https://opensea.io/collection/…" /></div><div class="field"><label>Wallet group</label><select><option>Alpha group · 6 wallets</option></select></div><div class="field"><label>Parallel checks</label><input type="number" min="1" max="16" value="4" /></div><div class="field full"><label>Selected wallets</label><div class="wallet-pills">${['Alpha 01','Alpha 02','Alpha 03','Alpha 04','Alpha 05','Alpha 06'].map(x=>`<span class="wallet-pill">${x}</span>`).join('')}</div></div></div></div><div class="modal-actions">${button('Stop','','ghost','stop-wl')}${button('Check eligibility','check','primary','run-wl')}</div></section><section class="panel" id="wl-results"><div class="empty-state">${svg('check')}<div><h3>No results yet</h3><p>Run the check to discover drop stages and stream eligibility for each selected wallet.</p></div></div></section></div></div>`;
}

function genericPage(route) {
  const generic = {
    proxies: ['Proxies','Keep access routes visible and testable without exposing credentials in the interface.','No proxy credentials are shown in this concept. Health, region and assignment remain visible to the operator.'],
    history: ['Execution history','Review confirmed outcomes, failed attempts and exported reports across every tool.','Every success is marked only after a confirmed receipt.'],
    settings: ['Settings','Configure local application behavior, safety limits and display preferences.','Business logic and vault policy are intentionally outside this design concept.'],
  }[route];
  if (route === 'history') return `<div class="page">${intro(generic[0],generic[1],button('Export report','upload','ghost','toast-export'))}${panel('Recent runs',`<div class="table-wrap"><table class="data-table"><thead><tr><th>Run</th><th>Operation</th><th>Network</th><th>Wallets</th><th>Outcome</th><th>Time</th></tr></thead><tbody><tr><td class="mono">RUN-0824</td><td>Ethereal Fragments</td><td>Ethereum</td><td>24</td><td>${status('14 confirmed · live','live')}</td><td>18:17</td></tr><tr><td class="mono">RUN-0823</td><td>Disperse funds</td><td>Base</td><td>8</td><td>${status('Confirmed','success')}</td><td>17:58</td></tr><tr><td class="mono">RUN-0822</td><td>Raw mint</td><td>Ethereum</td><td>6</td><td>${status('5 confirmed · 1 failed','error')}</td><td>16:42</td></tr></tbody></table></div>`,button('Filters','filter','ghost small','toast-filter'))}</div>`;
  if (route === 'settings') return `<div class="page">${intro(generic[0],generic[1])}<div class="form-layout"><section class="panel"><div class="tabs" data-tabs="settings"><button class="active" data-tab="general">General</button><button data-tab="network">Network</button><button data-tab="gas">Gas & mint</button><button data-tab="safety">Safety</button></div><div class="settings-pane" data-pane="general"><div class="form-section"><div class="section-heading"><h3>Operator experience</h3><p>Language, local output and feedback preferences.</p></div><div class="field-grid"><div class="field"><label>Language</label><select><option>English</option><option>Русский</option></select></div><div class="field"><label>Idle lock</label><select><option>30 minutes</option><option>Off</option><option>60 minutes</option></select></div></div><div class="setting-toggle"><span><strong>Export results</strong><small>Write JSON and CSV reports after completed runs.</small></span><button class="switch on" data-action="toggle-setting" aria-label="Toggle result export"></button></div><div class="setting-toggle"><span><strong>First-confirm sound</strong><small>Play a local signal only after the first confirmed receipt.</small></span><button class="switch on" data-action="toggle-setting" aria-label="Toggle confirmation sound"></button></div></div></div><div class="settings-pane hidden" data-pane="network"><div class="form-section"><div class="field-grid"><div class="field full"><label>Ethereum RPC URL</label><input type="password" value="https://••••••••••••••••" /></div><div class="field"><label>Alchemy network</label><select><option>Ethereum Mainnet</option></select></div><div class="field"><label>Proxy policy</label><select><option>Use assigned wallet route</option><option>Direct fallback</option></select></div><div class="field full"><label>Flashbots relay</label><input value="https://relay.flashbots.net" /></div><div class="field"><label>Target blocks</label><input type="number" value="3" /></div><div class="field"><label>Resubmit interval</label><input value="1200 ms" /></div></div></div></div><div class="settings-pane hidden" data-pane="gas"><div class="form-section"><div class="field-grid"><div class="field"><label>Default gas limit</label><input type="number" value="250000" /></div><div class="field"><label>Priority fee</label><input placeholder="auto" /></div><div class="field"><label>Base fee multiplier</label><input value="2.0" /></div><div class="field"><label>Gas multiplier</label><input value="1.15" /></div><div class="field"><label>Max retries</label><input type="number" value="12" /></div></div><div class="setting-toggle"><span><strong>GraphQL public-sale lookup</strong><small>Use GraphQL where supported for PUBLIC_SALE discovery.</small></span><button class="switch" data-action="toggle-setting" aria-label="Toggle GraphQL"></button></div></div></div><div class="settings-pane hidden" data-pane="safety"><div class="form-section"><div class="setting-toggle"><span><strong>Dry run by default</strong><small>Prepare and validate operations without broadcasting.</small></span><button class="switch on" data-action="toggle-setting" aria-label="Toggle dry run default"></button></div><div class="setting-toggle"><span><strong>Require LIVE confirmation</strong><small>Keep the typed gate and backend confirmation challenge.</small></span><button class="switch on" data-action="toggle-setting" aria-label="Toggle LIVE confirmation"></button></div><div class="setting-toggle"><span><strong>Quiet logs</strong><small>Reduce non-critical operational messages.</small></span><button class="switch" data-action="toggle-setting" aria-label="Toggle quiet logs"></button></div><div class="field" style="margin-top:14px"><label>Raw mint fee refresh at fire</label><select><option>Mainnet only</option><option>Always · includes L2 latency</option><option>Never</option></select></div></div></div><div class="modal-actions">${button('Save settings','success','primary','save-settings')}</div></section>${panel('Local application','<div class="panel-body summary-list"><div class="summary-row"><span>Vault</span><span class="status success">Encrypted</span></div><div class="summary-row"><span>Telemetry</span><strong>Disabled</strong></div><div class="summary-row"><span>Data location</span><strong>Local only</strong></div><div class="summary-row"><span>Concept version</span><strong>0.2.0-demo</strong></div></div>')}</div></div>`;
  return `<div class="page">${intro(generic[0],generic[1],`${button('Import file','upload','ghost','toast-import')}${button('Save routes','success','primary','save-proxies')}`)}<div class="form-layout"><section class="panel"><div class="form-section"><div class="section-heading"><h3>Proxy routes</h3><p>Credentials stay masked by default and are never copied into health tables or logs.</p></div><div class="field"><label>One proxy per line</label><textarea id="proxy-editor" style="-webkit-text-security:disc">http://operator:password@de-01.example:8443\nhttp://operator:password@us-01.example:8443\nhttp://operator:password@nl-01.example:8443</textarea><span class="field-hint">Supports URL and host:port:user:password formats.</span></div><div class="actions" style="margin-top:12px">${button('Reveal temporarily','','ghost','toggle-proxies')}${button('Test health','bolt','','test-proxies')}</div></div></section>${panel('Route health','<div class="panel-body"><div class="network-row"><span><i class="network-orb robinhood"></i><span><strong>EU Primary</strong><small>Alpha group · Frankfurt</small></span></span><span class="latency">42 ms</span></div><div class="network-row"><span><i class="network-orb robinhood"></i><span><strong>US East</strong><small>Reserve group · Virginia</small></span></span><span class="latency">91 ms</span></div><div class="network-row"><span><i class="network-orb"></i><span><strong>EU Backup</strong><small>Unassigned · Amsterdam</small></span></span><span class="latency">67 ms</span></div></div>','<span class="status success">3 available</span>')}</div></div>`;
}

function render(route = 'overview') {
  if (!pageMeta[route]) route = 'overview';
  if (!document.getElementById('modal-overlay').classList.contains('hidden')) closeModal();
  if (!document.getElementById('command-overlay').classList.contains('hidden')) closeCommands();
  closeDrawer();
  document.querySelectorAll('.nav-item[data-route]').forEach((el) => el.classList.toggle('active', el.dataset.route === route));
  document.getElementById('page-kicker').textContent = pageMeta[route][0];
  document.getElementById('page-title').textContent = pageMeta[route][1];
  const renderers = { overview: overviewPage, tasks: tasksPage, raw: rawPage, wallets: walletsPage, rpcs: networkPage, wl: wlPage };
  const content = renderers[route] ? renderers[route]() : ['disperse','sweep','multicall'].includes(route) ? toolFormPage(route) : genericPage(route);
  document.getElementById('page-content').innerHTML = content;
  document.getElementById('page-content').scrollTop = 0;
  document.querySelector('.sidebar').classList.remove('open');
  history.replaceState(null, '', `#${route}`);
  bindPageInteractions();
}

function toast(message) {
  const stack = document.getElementById('toast-stack');
  const node = document.createElement('div');
  node.className = 'toast';
  node.innerHTML = `${svg('success')}<span>${message}</span>`;
  stack.append(node);
  setTimeout(() => node.remove(), 3200);
}

let focusBeforeModal = null;
function wireFormLabels(root) {
  root.querySelectorAll('.field').forEach((field, index) => {
    const label = field.querySelector(':scope > label');
    const control = field.querySelector(':scope > input, :scope > select, :scope > textarea');
    if (!label || !control) return;
    if (!control.id) control.id = `demo-field-${Date.now()}-${index}`;
    label.htmlFor = control.id;
  });
}
function focusableWithin(root) {
  return [...root.querySelectorAll('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex="-1"])')].filter((node) => !node.hidden);
}
function openModal(html, wide = false) {
  focusBeforeModal = document.activeElement;
  const content = document.getElementById('modal-content');
  content.classList.toggle('wide', wide);
  content.innerHTML = html;
  document.getElementById('modal-overlay').classList.remove('hidden');
  wireFormLabels(content);
  focusableWithin(content)[0]?.focus();
}
function closeModal() {
  document.getElementById('modal-overlay').classList.add('hidden');
  focusBeforeModal?.focus?.();
  focusBeforeModal = null;
}
function newTaskModal() {
  openModal(`<div class="modal-header"><div><h2 id="modal-title">Create OpenSea mint task</h2><p>Collection → phase → execution policy → wallets.</p></div><button class="row-button" data-close-modal aria-label="Close dialog">${svg('close')}</button></div><div class="modal-body"><div class="section-heading"><h3>1 · Collection and phase</h3><p>The production integration resolves phases, network and eligibility through the existing OpenSea commands.</p></div><div class="field-grid"><div class="field"><label>Task name</label><input value="Ethereal Fragments public" /></div><div class="field"><label>Collection slug or URL</label><input value="ethereal-fragments" /></div><div class="field"><label>Drop phase</label><select><option>Public sale · 0.024 ETH</option><option>Allowlist · 0.018 ETH</option></select></div><div class="field"><label>Network</label><select><option>Auto · Ethereum</option><option>Base</option></select></div></div><div class="section-heading" style="margin-top:24px"><h3>2 · Mint policy</h3><p>Defaults remain hot-path safe; advanced options are explicit.</p></div><div class="field-grid"><div class="field"><label>Quantity per wallet</label><input type="number" min="1" max="50" value="1" /></div><div class="field"><label>Gas mode</label><select><option>Automatic</option><option>Manual fixed gas</option></select></div><div class="field"><label>Trigger</label><select><option>Manual start</option><option>Phase open</option><option>Scheduled time</option></select></div><div class="field"><label>Wallet group</label><select><option>Alpha group · 6 wallets</option><option>Reserve group · 2 wallets</option></select></div><div class="field full"><label>Selected wallets</label><div class="wallet-pills">${['Alpha 01','Alpha 02','Alpha 03','Alpha 04','Alpha 05','Alpha 06'].map(x=>`<span class="wallet-pill">${x}</span>`).join('')}</div></div><label class="option-line"><input type="checkbox" />Use different quantity per wallet</label><label class="option-line"><input type="checkbox" checked />Warm authentication before phase opens</label><label class="option-line"><input type="checkbox" />Auto-sweep proceeds after confirmations</label></div><div class="summary-total" style="margin-top:20px"><div class="summary-row"><span>Current estimate · 6 wallets</span><strong>0.1522 ETH total</strong></div><div class="summary-row"><span>Funding status</span><strong>6 ready · 0 unknown</strong></div></div></div><div class="modal-actions">${button('Cancel','','ghost','close-modal')}${button('Save draft task','plus','primary','create-task')}</div>`, true);
}
function liveModal() {
  openModal(`<div class="modal-header"><div><h2 id="modal-title">Review LIVE execution</h2><p>This is a safe mock flow. No backend call will be made.</p></div><button class="row-button" data-close-modal aria-label="Close dialog">${svg('close')}</button></div><div class="modal-body"><ul class="risk-list"><li><span>Target</span><strong>0xA4e2…9B11</strong></li><li><span>Wallets</span><strong>6</strong></li><li><span>Maximum spend</span><strong>0.1522 ETH</strong></li><li><span>Network</span><strong>Ethereum</strong></li></ul><div class="field"><label>Type LIVE to confirm</label><input id="live-confirm" placeholder="LIVE" autocomplete="off" /></div></div><div class="modal-actions">${button('Cancel','','ghost','close-modal')}<button class="button danger" id="confirm-live" disabled>${svg('play')}Start execution</button></div>`);
  const input = document.getElementById('live-confirm');
  const confirm = document.getElementById('confirm-live');
  input.addEventListener('input', () => confirm.disabled = input.value !== 'LIVE');
  confirm.addEventListener('click', () => { closeModal(); toast('Mock execution started — no transaction sent'); render('overview'); });
  input.focus();
}

function unlockModal() {
  openModal(`<div class="modal-header"><div><h2 id="modal-title">Unlock local vault</h2><p>Keys remain encrypted on this device.</p></div><button class="row-button" data-close-modal aria-label="Close dialog">${svg('close')}</button></div><div class="modal-body"><div class="notice">${svg('warning')}<span>MINTER is intended for burner wallets only. Never import a primary or high-value wallet.</span></div><div class="field" style="margin-top:18px"><label>Vault password</label><input type="password" value="demo-password" autocomplete="current-password" /></div><label class="option-line" style="margin-top:14px"><input type="checkbox" checked />I understand and will use burner wallets only</label><div class="summary-list" style="margin-top:18px"><div class="summary-row"><span>Storage</span><strong>Local encrypted vault</strong></div><div class="summary-row"><span>Telemetry</span><strong>Disabled</strong></div><div class="summary-row"><span>Auto-lock</span><strong>30 minutes</strong></div></div></div><div class="modal-actions">${button('Cancel','','ghost','close-modal')}${button('Unlock demo vault','lock','primary','unlock-demo')}</div>`);
}
function walletDrawer(index) {
  const w = wallets[index] || wallets[0];
  const drawer = document.getElementById('drawer');
  drawer.innerHTML = `<div class="drawer-header"><div><h2>${w[0]}</h2><p>${w[1]}</p></div><button class="row-button" id="drawer-close">${svg('close')}</button></div><div class="drawer-body"><div class="summary-list"><div class="summary-row"><span>Native balance</span><strong>${w[2]} ETH</strong></div><div class="summary-row"><span>USD estimate</span><strong>${w[3]}</strong></div><div class="summary-row"><span>Nonce</span><strong>${182 + index}</strong></div><div class="summary-row"><span>Last activity</span><strong>8 min ago</strong></div></div><div class="actions" style="margin-top:20px">${button('Copy address','copy','ghost','copy-address')}${button('Open explorer','external','','open-explorer')}</div><div class="section-heading" style="margin-top:28px"><h3>Recent activity</h3><p>Confirmed local transaction history for this burner wallet.</p></div><div class="activity-list"><div class="activity"><span class="activity-icon">${svg('success')}</span><div><p>Mint receipt confirmed</p><small>0x93d…fe2</small></div><time>18:22</time></div><div class="activity"><span class="activity-icon">${svg('wallet')}</span><div><p>Funding received · 0.08 ETH</p><small>0x412…c90</small></div><time>17:58</time></div></div></div>`;
  drawer.classList.remove('hidden'); document.getElementById('drawer-backdrop').classList.remove('hidden');
  document.getElementById('drawer-close').onclick = closeDrawer;
  drawer.querySelector('[data-action="copy-address"]').onclick = () => toast('Address copied');
  drawer.querySelector('[data-action="open-explorer"]').onclick = () => toast('External links are disabled in this demo');
}
function closeDrawer() { document.getElementById('drawer').classList.add('hidden'); document.getElementById('drawer-backdrop').classList.add('hidden'); }

function taskDrawer() {
  const drawer = document.getElementById('drawer');
  drawer.innerHTML = `<div class="drawer-header"><div><h2>Ethereal Fragments</h2><p>TASK-0824 · Ethereum</p></div><button class="row-button" id="drawer-close" aria-label="Close details">${svg('close')}</button></div><div class="drawer-body"><div class="execution-name"><span class="collection-avatar">EF</span><div><h4>Public mint · 0.024 ETH</h4><p>0xA4e2…9B11</p></div></div><div class="summary-list"><div class="summary-row"><span>State</span>${status('Live','live')}</div><div class="summary-row"><span>Wallets</span><strong>24</strong></div><div class="summary-row"><span>Confirmed</span><strong>14</strong></div><div class="summary-row"><span>Gas policy</span><strong>Auto · 16.2 Gwei max</strong></div><div class="summary-row"><span>Trigger</span><strong>Manual</strong></div></div><div class="section-heading" style="margin-top:28px"><h3>Execution controls</h3><p>The production port keeps the existing start, stop, edit and persisted-task contracts.</p></div><div class="actions">${button('Open Mission Control','bolt','primary','open-mission')}${button('Pause task','','ghost','pause-task')}</div></div>`;
  drawer.classList.remove('hidden');
  document.getElementById('drawer-backdrop').classList.remove('hidden');
  document.getElementById('drawer-close').onclick = closeDrawer;
  drawer.querySelector('[data-action="open-mission"]').onclick = () => { closeDrawer(); render('overview'); toast('Mission Control focused'); };
  drawer.querySelector('[data-action="pause-task"]').onclick = () => toast('Task paused in demo state');
}

function handleAction(action, trigger = null) {
  if (action === 'new-task') newTaskModal();
  else if (action === 'raw-live') liveModal();
  else if (action === 'go-tasks') render('tasks');
  else if (action === 'go-history') render('history');
  else if (action === 'close-modal') closeModal();
  else if (action === 'create-task') { closeModal(); toast('Draft task created'); render('tasks'); }
  else if (action === 'run-wl') runWlCheck();
  else if (action === 'clear-wl') { const area = document.querySelector('.field textarea'); if (area) area.value = ''; }
  else if (action === 'task-details') taskDrawer();
  else if (action === 'rpc-all' || action === 'rpc-test') toast('RPC checks completed · mock latency refreshed');
  else if (action === 'simulate') toast('Simulation passed on 3 agreeing RPC endpoints');
  else if (action === 'save-settings') toast('Demo preferences saved locally');
  else if (action === 'preview-operation') toast('Execution plan ready for operator review');
  else if (action === 'add-call') {
    const list = document.getElementById('call-list');
    if (list) { list.insertAdjacentHTML('beforeend', callRow(list.children.length + 1)); wireFormLabels(list.lastElementChild); }
  }
  else if (action === 'remove-call') {
    const calls = document.querySelectorAll('#call-list .call-row');
    if (calls.length <= 1) toast('Multicall requires at least one call');
    else trigger?.closest('.call-row')?.remove();
  }
  else if (action === 'stop-wl') toast('No allowlist check is currently running');
  else if (action === 'toggle-setting') {
    trigger.classList.toggle('on');
    trigger.setAttribute('aria-pressed', String(trigger.classList.contains('on')));
  }
  else if (action === 'toggle-proxies') {
    const editor = document.getElementById('proxy-editor');
    const revealed = editor?.style.webkitTextSecurity === 'none';
    if (editor) editor.style.webkitTextSecurity = revealed ? 'disc' : 'none';
    trigger.textContent = revealed ? 'Reveal temporarily' : 'Mask credentials';
  }
  else if (action === 'test-proxies') toast('3 proxy routes responded · credentials remained masked');
  else if (action === 'save-proxies') toast('Proxy routes validated and saved in mock state');
  else if (action === 'unlock-demo') { closeModal(); toast('Demo vault unlocked · no keys were accessed'); }
  else if (action === 'raw-discover') toast('3 callable functions discovered from mock bytecode');
  else if (action === 'toast-import') toast('Native file picker is represented by this mock action');
  else if (action === 'toast-export') toast('Mock report exported');
  else if (action === 'toast-filter') toast('Advanced filter popover previewed');
  else if (action === 'create-wallets') toast('Burner generation requires the encrypted production vault');
  else if (action === 'add-rpc') toast('Endpoint editor opened in mock state');
  else if (action === 'add-proxy') toast('Proxy editor opened in mock state');
}

function bindPageInteractions() {
  wireFormLabels(document.getElementById('page-content'));
  document.querySelectorAll('.switch').forEach((node) => node.setAttribute('aria-pressed', String(node.classList.contains('on'))));
  document.querySelectorAll('[data-raw-preset]').forEach((preset) => preset.addEventListener('click', () => {
    document.querySelectorAll('[data-raw-preset]').forEach(node => {
      node.classList.toggle('active', node === preset);
      node.setAttribute('aria-pressed', String(node === preset));
    });
    document.getElementById('raw-standard-call')?.classList.toggle('hidden', preset.dataset.rawPreset === 'custom');
    document.getElementById('raw-custom-call')?.classList.toggle('hidden', preset.dataset.rawPreset !== 'custom');
    wireFormLabels(document.getElementById('page-content'));
  }));
  document.querySelectorAll('[data-tabs]').forEach((tabs, groupIndex) => {
    tabs.setAttribute('role', 'tablist');
    const tabNodes = [...tabs.querySelectorAll('[data-tab]')];
    const panel = tabs.closest('.panel');
    tabNodes.forEach((tab, tabIndex) => {
      const pane = panel?.querySelector(`[data-pane="${tab.dataset.tab}"]`);
      const paneId = `demo-tab-panel-${groupIndex}-${tabIndex}`;
      tab.setAttribute('role', 'tab');
      tab.setAttribute('aria-selected', String(tab.classList.contains('active')));
      tab.setAttribute('aria-controls', paneId);
      tab.tabIndex = tab.classList.contains('active') ? 0 : -1;
      if (pane) { pane.id = paneId; pane.setAttribute('role', 'tabpanel'); }
      tab.addEventListener('click', () => {
        tabNodes.forEach(x => { const active = x === tab; x.classList.toggle('active', active); x.setAttribute('aria-selected', String(active)); x.tabIndex = active ? 0 : -1; });
        panel?.querySelectorAll('[data-pane]').forEach(p => p.classList.toggle('hidden', p.dataset.pane !== tab.dataset.tab));
        wireFormLabels(panel || document.getElementById('page-content'));
      });
      tab.addEventListener('keydown', (event) => {
        if (!['ArrowLeft','ArrowRight'].includes(event.key)) return;
        event.preventDefault();
        const offset = event.key === 'ArrowRight' ? 1 : -1;
        tabNodes[(tabIndex + offset + tabNodes.length) % tabNodes.length].click();
        tabNodes[(tabIndex + offset + tabNodes.length) % tabNodes.length].focus();
      });
    });
  });
  document.querySelectorAll('.wallet-open').forEach((node) => node.addEventListener('click', () => walletDrawer(Number(node.dataset.wallet))));
  const taskSearch = document.getElementById('task-search');
  if (taskSearch) taskSearch.addEventListener('input', () => filterRows('#task-table tr', taskSearch.value));
  const walletSearch = document.getElementById('wallet-search');
  if (walletSearch) walletSearch.addEventListener('input', () => filterRows('#wallet-table tr', walletSearch.value));
  const checks = [...document.querySelectorAll('.wallet-check')];
  const all = document.getElementById('wallet-all');
  if (all) all.addEventListener('change', () => { checks.forEach(c => c.checked = all.checked); updateSelection(); });
  checks.forEach(c => c.addEventListener('change', updateSelection));
  document.querySelectorAll('[data-wallet-filter]').forEach((node) => node.addEventListener('click', () => {
    node.parentElement.querySelectorAll('button').forEach(x => x.classList.toggle('active', x === node));
    document.querySelectorAll('#wallet-table tr').forEach((row) => {
      const text = row.textContent.toLowerCase();
      row.hidden = node.dataset.walletFilter === 'ready' ? !text.includes('ready') : node.dataset.walletFilter === 'low' ? !text.includes('low balance') : false;
    });
  }));
  document.querySelectorAll('.segmented[data-filter="tasks"] button').forEach((node) => node.addEventListener('click', () => {
    node.parentElement.querySelectorAll('button').forEach(x => x.classList.toggle('active', x === node));
    document.querySelectorAll('#task-table tr').forEach(row => row.hidden = node.dataset.value !== 'all' && row.dataset.taskState !== node.dataset.value);
  }));
}
function filterRows(selector, query) { document.querySelectorAll(selector).forEach(row => row.hidden = !row.textContent.toLowerCase().includes(query.trim().toLowerCase())); }
function updateSelection() { const n = document.querySelectorAll('.wallet-check:checked').length; const label = document.getElementById('wallet-selection'); if (label) label.textContent = `${n} selected`; }
function runWlCheck() {
  document.getElementById('wl-results').innerHTML = `<header class="panel-header"><div class="panel-title"><h3>Eligibility results</h3></div>${status('4 / 6 eligible','success')}</header><div class="panel-body"><div class="summary-list"><div class="summary-row"><span>Alpha 01 · 0x71F4…2A9C</span>${status('Eligible','success')}</div><div class="summary-row"><span>Alpha 02 · 0x98C2…7D11</span>${status('Not found','error')}</div><div class="summary-row"><span>Alpha 03 · 0x42D8…A512</span>${status('Eligible','success')}</div><div class="summary-row"><span>Alpha 04 · 0xB91A…10FE</span>${status('Not found','error')}</div><div class="summary-row"><span>Alpha 05 · 0x12F3…9C40</span>${status('Eligible','success')}</div><div class="summary-row"><span>Alpha 06 · 0xD77E…F283</span>${status('Eligible','success')}</div></div></div>`;
  toast('Allowlist check completed');
}

const commandItems = Object.entries(pageMeta).map(([route, meta]) => ({ route, label: meta[1], group: meta[0], icon: document.querySelector(`[data-route="${route}"]`)?.dataset.icon || 'overview' }));
let commandIndex = 0;
let focusBeforeCommands = null;
function renderCommands(query = '') {
  const list = commandItems.filter(x => `${x.label} ${x.group}`.toLowerCase().includes(query.toLowerCase()));
  commandIndex = Math.min(commandIndex, Math.max(0,list.length-1));
  document.getElementById('command-results').innerHTML = `<span class="command-group-label">Pages & actions</span>${list.map((item,i)=>`<button class="command-result ${i===commandIndex?'active':''}" data-command-route="${item.route}">${svg(item.icon)}<span>${item.label}</span><small>${item.group}</small></button>`).join('')}`;
  document.querySelectorAll('[data-command-route]').forEach(n => n.onclick = () => { closeCommands(); render(n.dataset.commandRoute); });
  return list;
}
function openCommands() {
  if (!document.getElementById('modal-overlay').classList.contains('hidden')) return;
  focusBeforeCommands = document.activeElement;
  const overlay = document.getElementById('command-overlay'); overlay.classList.remove('hidden'); commandIndex = 0; renderCommands();
  const input = document.getElementById('command-input'); input.value=''; input.focus();
}
function closeCommands() {
  const overlay = document.getElementById('command-overlay');
  if (overlay.classList.contains('hidden')) return;
  overlay.classList.add('hidden');
  focusBeforeCommands?.focus?.();
  focusBeforeCommands = null;
}

document.getElementById('page-content').addEventListener('click', (event) => {
  const trigger = event.target.closest('[data-action]');
  if (trigger) handleAction(trigger.dataset.action, trigger);
});
document.getElementById('modal-content').addEventListener('click', (event) => {
  const trigger = event.target.closest('[data-action]');
  if (trigger) handleAction(trigger.dataset.action, trigger);
});
document.getElementById('main-nav').addEventListener('click', (event) => { const item = event.target.closest('[data-route]'); if (item) render(item.dataset.route); });
document.querySelector('.sidebar-footer').addEventListener('click', (event) => { const item = event.target.closest('[data-route]'); if (item) render(item.dataset.route); });
document.getElementById('command-trigger').onclick = openCommands;
document.getElementById('profile-button').onclick = unlockModal;
document.getElementById('command-input').addEventListener('input', e => { commandIndex = 0; renderCommands(e.target.value); });
document.getElementById('command-input').addEventListener('keydown', e => { const list = renderCommands(e.target.value); if (e.key === 'ArrowDown') { e.preventDefault(); commandIndex = Math.min(commandIndex+1,list.length-1); renderCommands(e.target.value); } if (e.key === 'ArrowUp') { e.preventDefault(); commandIndex = Math.max(commandIndex-1,0); renderCommands(e.target.value); } if (e.key === 'Enter' && list[commandIndex]) { closeCommands(); render(list[commandIndex].route); } });
document.getElementById('command-overlay').addEventListener('mousedown', e => { if (e.target.id === 'command-overlay') closeCommands(); });
document.getElementById('modal-overlay').addEventListener('mousedown', e => { if (e.target.id === 'modal-overlay') closeModal(); });
document.getElementById('modal-content').addEventListener('click', e => { if (e.target.closest('[data-close-modal]')) closeModal(); });
document.getElementById('drawer-backdrop').onclick = closeDrawer;
document.getElementById('mobile-menu').onclick = () => document.querySelector('.sidebar').classList.toggle('open');
const networkTrigger = document.getElementById('network-trigger');
const networkMenu = document.getElementById('network-menu');
function closeNetworkMenu(restoreFocus = false) {
  networkMenu.classList.add('hidden');
  networkTrigger.setAttribute('aria-expanded', 'false');
  if (restoreFocus) networkTrigger.focus();
}
networkTrigger.onclick = () => {
  const willOpen = networkMenu.classList.contains('hidden');
  networkMenu.classList.toggle('hidden', !willOpen);
  networkTrigger.setAttribute('aria-expanded', String(willOpen));
};
document.querySelectorAll('.network-option').forEach(option => option.onclick = () => { document.querySelectorAll('.network-option').forEach(x=>x.classList.toggle('selected',x===option)); document.getElementById('network-name').textContent=option.dataset.network; document.getElementById('network-symbol').textContent=option.dataset.symbol; networkTrigger.setAttribute('aria-label', `Active network: ${option.dataset.network}`); closeNetworkMenu(); toast(`Network switched to ${option.dataset.network}`); });
document.addEventListener('pointerdown', (event) => { if (!event.target.closest('.network-select')) closeNetworkMenu(); });
document.getElementById('notifications-button').onclick = () => toast('No unresolved operator alerts');
document.addEventListener('keydown', e => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); openCommands(); }
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'f' && location.hash === '#wallets') { e.preventDefault(); document.getElementById('wallet-search')?.focus(); }
  if (e.key === 'Tab' && !document.getElementById('modal-overlay').classList.contains('hidden')) {
    const items = focusableWithin(document.getElementById('modal-content'));
    if (!items.length) return;
    const first = items[0], last = items[items.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  }
  if (e.key === 'Tab' && !document.getElementById('command-overlay').classList.contains('hidden')) {
    const items = focusableWithin(document.querySelector('.command-panel'));
    const first = items[0], last = items[items.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  }
  if (e.key === 'Escape') {
    if (!document.getElementById('command-overlay').classList.contains('hidden')) { closeCommands(); return; }
    if (!document.getElementById('modal-overlay').classList.contains('hidden')) { closeModal(); return; }
    closeNetworkMenu(true); closeDrawer(); document.querySelector('.sidebar').classList.remove('open');
  }
});
window.addEventListener('hashchange', () => render(location.hash.slice(1) || 'overview'));

render(location.hash.slice(1) || 'overview');
