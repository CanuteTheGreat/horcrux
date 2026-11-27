// Pure-function tests of ui-kit/map/map.js (run: node ui-kit/map/test.js; ui-kit/map/check.py runs it).
'use strict';
const assert = require('assert');
const M = require('./map.js');
const g = {
  schema: 'infra-map/v1',
  groups: [{ id: 'seg:@lan', label: 'LAN', sub: '192.168.1.0/24' }, { id: 'seg:@opt1', label: 'IoT "lab"' }],
  nodes: [
    { id: 'router', label: 'gw', kind: 'router', status: 'up' },
    { id: 'dev:aa', label: 'printer-1', group: 'seg:@lan', status: 'up', ip: '192.168.1.20' },
    { id: 'dev:bb', label: 'cam', group: 'seg:@opt1', status: 'down' },
    { id: 'dev:cc', label: 'loose', group: 'nope', status: 'running' },
  ],
  edges: [{ from: 'router', to: 'seg:@lan', label: 'br0' }, { from: 'dev:aa', to: 'dev:bb', kind: 'member' }],
};
const L = M.layout(g);
assert.ok(L.pos.router.hub, 'router is a hub');
assert.strictEqual(L.groups.length, 3, 'unknown group lands in Other');
assert.ok(L.pos['dev:aa'].x < L.pos['dev:bb'].x, 'groups are columns in order');
assert.ok(L.pos['dev:aa'].y > L.pos.router.y, 'hubs are on top');
const L2 = M.layout({ nodes: [{ id: 'r', kind: 'router' }, { id: 'i', kind: 'internet' }], groups: [] });
assert.ok(L2.pos.i.y < L2.pos.r.y, 'the internet row sits above the router');
assert.deepStrictEqual(M.layout(g), L, 'layout is deterministic');
const m = M.toMermaid(g);
assert.ok(m.startsWith('flowchart LR\n'));
assert.ok(m.includes('subgraph nseg__lan["LAN 192.168.1.0/24"]'), m);
assert.ok(m.includes('IoT \'lab\''), 'quotes are escaped');
assert.ok(m.includes('ndev_aa["printer-1<br/>192.168.1.20"]:::ok'));
assert.ok(m.includes('ndev_bb["cam"]:::crit'));
assert.ok(m.includes('ndev_cc["loose"]:::ok'), 'node of an unknown group still appears');
assert.ok(m.includes('nrouter ---|"br0"| nseg__lan'));
assert.ok(!m.includes('ndev_aa --- ndev_bb'), 'member edges are not drawn');
assert.strictEqual(M.tone('Running'), 'ok');
assert.strictEqual(M.tone('weird'), 'off');
console.log('ui-kit/map test.js: OK');
