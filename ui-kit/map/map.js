/* ui-kit/map v0.1: infra-map/v1 graph viewer shared by Horcrux and Patronus (docs/design/infra-map.md).
   No dependencies, no build step. UiMap.mount(el, {src, headers, refresh, readonly}) draws the graph;
   UiMap.toMermaid(graph) / UiMap.layout(graph) are pure (tested by test.js). */
(function (root) {
  'use strict';
  var COLW = 210, GAP = 70, PAD = 24, ROWH = 34, HUBW = 170, HUBH = 40, HEAD = 38;
  var TONES = { up: 'ok', running: 'ok', online: 'ok', down: 'crit', stopped: 'crit', offline: 'crit', failed: 'crit', warn: 'warn', paused: 'warn', degraded: 'warn' };
  function tone(s) { return TONES[String(s || '').toLowerCase()] || 'off'; }
  function isHub(n) { return !n.group && (n.kind === 'router' || n.kind === 'host' || n.kind === 'internet'); }

  /* Deterministic layout: hubs on top, one column per group, ungrouped nodes in a last column. */
  function layout(g) {
    var groups = (g.groups || []).slice(), pos = {}, cols = {}, i;
    var nodes = g.nodes || [], hubs = nodes.filter(isHub), rest = nodes.filter(function (n) { return !isHub(n); });
    var known = {};
    groups.forEach(function (gr) { known[gr.id] = 1; });
    if (rest.some(function (n) { return !known[n.group]; })) groups.push({ id: '__other', label: 'Other', kind: 'other' });
    groups.forEach(function (gr) { cols[gr.id] = []; });
    rest.forEach(function (n) { cols[known[n.group] ? n.group : '__other'].push(n); });
    // the internet (upstream) row sits above the router/host row
    var tiers = [hubs.filter(function (n) { return n.kind === 'internet'; }), hubs.filter(function (n) { return n.kind !== 'internet'; })]
      .filter(function (t) { return t.length; });
    var widest = Math.max.apply(null, [0].concat(tiers.map(function (t) { return t.length; })));
    var width = Math.max(PAD * 2 + groups.length * COLW + Math.max(0, groups.length - 1) * GAP, PAD * 2 + widest * (HUBW + GAP));
    tiers.forEach(function (t, r) {
      var hx = (width - (t.length * HUBW + Math.max(0, t.length - 1) * GAP)) / 2;
      t.forEach(function (n, k) { pos[n.id] = { x: hx + k * (HUBW + GAP), y: PAD + r * (HUBH + 36), w: HUBW, h: HUBH, hub: true, tier: r }; });
    });
    var top = PAD + (tiers.length ? tiers.length * (HUBH + 36) + 20 : 0), height = top;
    groups.forEach(function (gr, c) {
      var x = PAD + c * (COLW + GAP), list = cols[gr.id];
      var h = HEAD + Math.max(1, list.length) * ROWH + 10;
      pos[gr.id] = { x: x, y: top, w: COLW, h: h, group: true };
      for (i = 0; i < list.length; i++) pos[list[i].id] = { x: x + 8, y: top + HEAD + i * ROWH, w: COLW - 16, h: ROWH - 6 };
      height = Math.max(height, top + h);
    });
    return { pos: pos, groups: groups, width: Math.ceil(width), height: Math.ceil(height + PAD) };
  }

  function mid(s) { return 'n' + String(s).replace(/[^A-Za-z0-9_]/g, '_'); }
  function q(s) { return String(s == null ? '' : s).replace(/"/g, "'").replace(/[\r\n]+/g, ' '); }
  /* Mermaid flowchart: one subgraph per group, status as a class. */
  function toMermaid(g) {
    var out = ['flowchart LR'], seen = {};
    function line(n, ind) {
      seen[n.id] = 1;
      var lbl = q(n.label || n.id) + (n.ip ? '<br/>' + q(n.ip) : '');
      out.push(ind + mid(n.id) + '["' + lbl + '"]:::' + tone(n.status));
    }
    (g.nodes || []).filter(function (n) { return !n.group; }).forEach(function (n) { line(n, '  '); });
    (g.groups || []).forEach(function (gr) {
      out.push('  subgraph ' + mid(gr.id) + '["' + q(gr.label || gr.id) + (gr.sub ? ' ' + q(gr.sub) : '') + '"]');
      (g.nodes || []).filter(function (n) { return n.group === gr.id; }).forEach(function (n) { line(n, '    '); });
      out.push('  end');
    });
    (g.nodes || []).forEach(function (n) { if (!seen[n.id]) line(n, '  '); });
    (g.edges || []).forEach(function (e) {
      if (e.kind === 'member') return;
      out.push('  ' + mid(e.from) + (e.label ? ' ---|"' + q(e.label) + '"| ' : ' --- ') + mid(e.to));
    });
    out.push('  classDef ok fill:#dcfce7,stroke:#15803d', '  classDef crit fill:#fee2e2,stroke:#b91c1c',
      '  classDef warn fill:#fef3c7,stroke:#b45309', '  classDef off fill:#f1f5f9,stroke:#64748b');
    return out.join('\n') + '\n';
  }

  var NS = 'http://www.w3.org/2000/svg';
  function el(name, attrs, parent) {
    var e = document.createElementNS(NS, name);
    for (var k in attrs) if (attrs[k] != null) e.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(e);
    return e;
  }
  function h(tag, cls, text, parent) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    if (parent) parent.appendChild(e);
    return e;
  }
  function colors(host) {
    var cs = getComputedStyle(host), v = function (n, d) { return (cs.getPropertyValue(n) || '').trim() || d; };
    // --m-* come from map.css (kit tokens, else the product's own, else a palette per colour scheme)
    return { ink: v('--m-ink', '#1f2933'), muted: v('--m-muted', '#5c6b7a'), line: v('--c-border-strong', v('--m-line', '#9aa5b1')),
      surface: v('--m-surface', '#ffffff'), sunken: v('--m-sunken', '#f3f5f7'), accent: v('--m-accent', '#2563eb'),
      ok: v('--c-ok-solid', '#15803d'), crit: v('--c-crit-solid', '#b91c1c'), warn: v('--c-warn-solid', '#b45309'), off: v('--c-neutral-solid', '#64748b') };
  }
  function trunc(s, n) { s = String(s || ''); return s.length > n ? s.slice(0, n - 1) + '\u2026' : s; }
  function download(name, data, type) {
    var blob = data instanceof Blob ? data : new Blob([data], { type: type });
    var a = document.createElement('a'); a.href = URL.createObjectURL(blob); a.download = name;
    document.body.appendChild(a); a.click(); setTimeout(function () { URL.revokeObjectURL(a.href); a.remove(); }, 1000);
  }

  function mount(host, opts) {
    opts = opts || {};
    var st = { graph: null, filter: '', status: '', zoom: 0, timer: null, sel: null, view: 'map' };
    host.classList.add('ui-map');
    host.innerHTML = '';
    var bar = h('div', 'ui-map-bar', null, host);
    var search = h('input', 'ui-input ui-map-filter', null, bar);
    search.type = 'search'; search.placeholder = 'Filter'; search.title = 'Filter by name, IP, MAC or class'; search.setAttribute('aria-label', 'Filter the map');
    var sel = h('select', 'ui-input ui-map-status', null, bar);
    sel.setAttribute('aria-label', 'Status');
    [['', 'All states'], ['ok', 'Up'], ['crit', 'Down'], ['warn', 'Warning'], ['off', 'Unknown']].forEach(function (o) { var x = h('option', null, o[1], sel); x.value = o[0]; });
    var summary = h('span', 'ui-map-summary', '', bar);
    summary.setAttribute('aria-live', 'polite');
    var tools = h('span', 'ui-map-tools', null, bar);
    function btn(label, fn, title) { var b = h('button', 'ui-btn sm', label, tools); b.type = 'button'; if (title) b.title = title; b.addEventListener('click', fn); return b; }
    var viewBtn = btn('List', function () { st.view = st.view === 'map' ? 'list' : 'map'; viewBtn.textContent = st.view === 'map' ? 'List' : 'Map'; draw(); }, 'Switch between the map and a list');
    btn('\u2212', function () { st.zoom = Math.max(0.4, cur() - 0.2); draw(); }, 'Zoom out');
    btn('+', function () { st.zoom = Math.min(2.5, cur() + 0.2); draw(); }, 'Zoom in');
    btn('Fit', function () { st.zoom = 0; draw(); }, 'Fit the width (0 = fit)');
    var exp = h('details', 'ui-menu ui-map-export', null, tools);
    h('summary', 'ui-btn sm', 'Export', exp);
    var items = h('div', 'ui-menu-items', null, exp);
    [['PNG', png], ['SVG', function () { download(fname('svg'), svgText(), 'image/svg+xml'); }],
      ['Mermaid', function () { download(fname('mmd'), toMermaid(st.graph || {}), 'text/plain'); }],
      ['JSON', function () { download(fname('json'), JSON.stringify(st.graph, null, 2), 'application/json'); }]].forEach(function (x) {
      var b = h('button', null, x[0], items); b.type = 'button'; b.addEventListener('click', function () { exp.open = false; x[1](); });
    });
    var findings = h('div', 'ui-map-findings', null, host);
    var stage = h('div', 'ui-map-stage', null, host);
    stage.tabIndex = 0; stage.setAttribute('role', 'region'); stage.setAttribute('aria-label', 'Infrastructure map');
    var panel = h('aside', 'ui-map-panel', null, host);
    panel.hidden = true;
    search.addEventListener('input', function () { st.filter = search.value.trim().toLowerCase(); draw(); });
    sel.addEventListener('change', function () { st.status = sel.value; draw(); });

    // zoom 0 = fit the stage width (never above 1, never below 0.5: below that the list view reads better)
    function fit(w) { var avail = (stage.clientWidth || w) - 2; return Math.max(0.5, Math.min(1, avail / w)); }
    function cur() { return st.zoom || st.last || 1; }
    function fname(ext) { var s = (st.graph && st.graph.source) || {}; return (s.product || 'map') + '-' + (s.name || 'map').replace(/[^A-Za-z0-9_.-]/g, '_') + '.' + ext; }
    function match(n) {
      if (st.status && tone(n.status) !== st.status) return false;
      if (!st.filter) return true;
      var hay = [n.label, n.id, n.ip, n.mac, n.class, n.kind].join(' ').toLowerCase();
      return st.filter.split(/\s+/).every(function (w) { return hay.indexOf(w) >= 0; });
    }
    function svgText() { var s = stage.querySelector('svg'); return s ? new XMLSerializer().serializeToString(s) : ''; }
    function png() {
      var s = stage.querySelector('svg'); if (!s) return;
      var w = +s.getAttribute('width'), ht = +s.getAttribute('height'), img = new Image(), scale = 2;
      img.onload = function () {
        var c = document.createElement('canvas'); c.width = w * scale; c.height = ht * scale;
        var x = c.getContext('2d'); x.scale(scale, scale); x.drawImage(img, 0, 0);
        c.toBlob(function (b) { download(fname('png'), b, 'image/png'); });
      };
      img.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svgText());
    }
    function show(n) {
      st.sel = n ? n.id : null; panel.hidden = !n; panel.innerHTML = '';
      if (!n) return;
      var head = h('div', 'ui-map-panel-head', null, panel);
      h('strong', null, n.label || n.id, head);
      var x = h('button', 'ui-btn sm', 'Close', head); x.type = 'button'; x.addEventListener('click', function () { show(null); });
      var dl = h('dl', 'ui-kv', null, panel);
      function kv(k, v) { if (v == null || v === '') return; h('dt', null, k, dl); h('dd', null, String(v), dl); }
      kv('Status', n.status); kv('Kind', n.kind); kv('Class', n.class); kv('IP', n.ip); kv('MAC', n.mac);
      var m = n.meta || {}; Object.keys(m).forEach(function (k) { kv(k, m[k]); });
      if (n.href && !opts.readonly) { var a = h('a', 'ui-btn sm', 'Open', panel); a.href = n.href; }
      if (opts.onSelect) opts.onSelect(n, panel);
    }
    function draw() {
      var g = st.graph; if (!g) return;
      var all = g.nodes || [], vis = all.filter(match), ids = {};
      vis.forEach(function (n) { ids[n.id] = 1; });
      var up = all.filter(function (n) { return tone(n.status) === 'ok'; }).length;
      var down = all.filter(function (n) { return tone(n.status) === 'crit'; }).length;
      summary.textContent = all.length + ' items \u00b7 ' + up + ' up \u00b7 ' + down + ' down' + (vis.length !== all.length ? ' \u00b7 ' + vis.length + ' shown' : '');
      findings.innerHTML = '';
      (g.findings || []).forEach(function (f) {
        var b = h('div', 'ui-banner ' + (f.severity === 'high' ? 'crit' : f.severity === 'info' ? 'info' : 'warn'), null, findings);
        h('strong', null, f.title + ' ', b); h('span', null, f.detail || '', b);
        if (f.action && !opts.readonly) { var a = h('a', 'ui-btn sm', f.action.label, b); a.href = f.action.href; }
      });
      stage.innerHTML = '';
      if (!all.length) { var em = h('div', 'ui-empty', null, stage); h('strong', null, opts.emptyTitle || 'Nothing to show yet', em); em.appendChild(document.createTextNode(opts.emptyText || 'Items appear here as soon as they are discovered.')); return; }
      if (st.view === 'list') return drawList(g, vis);
      var sub = { nodes: vis, groups: g.groups, edges: g.edges };
      var L = layout(sub), C = colors(host), z = st.zoom || fit(L.width);
      st.last = z;
      var svg = el('svg', { xmlns: NS, width: Math.round(L.width * z), height: Math.round(L.height * z), viewBox: '0 0 ' + L.width + ' ' + L.height, role: 'img', 'aria-label': 'Map of ' + vis.length + ' items', 'font-family': 'system-ui, sans-serif' }, stage);
      el('rect', { x: 0, y: 0, width: L.width, height: L.height, fill: C.sunken }, svg);
      var eg = el('g', { fill: 'none', stroke: C.line, 'stroke-width': 1.4 }, svg);
      (g.edges || []).forEach(function (e) {
        if (e.kind === 'member') return;
        var a = L.pos[e.from], b = L.pos[e.to];
        if (!a || !b || (!ids[e.from] && !L.pos[e.from].group) || (!ids[e.to] && !L.pos[e.to].group)) return;
        var ax, ay, bx, by, d;
        if (a.hub && b.hub && a.tier !== b.tier) {
          var up = a.tier < b.tier ? a : b, dn = a.tier < b.tier ? b : a;
          ax = up.x + up.w / 2; ay = up.y + up.h; bx = dn.x + dn.w / 2; by = dn.y;
          d = 'M' + ax + ' ' + ay + ' C' + ax + ' ' + (ay + 18) + ' ' + bx + ' ' + (by - 18) + ' ' + bx + ' ' + by;
        } else if (a.hub && b.hub) {
          var l = a.x < b.x ? a : b, rr = a.x < b.x ? b : a;
          d = 'M' + (l.x + l.w) + ' ' + (l.y + l.h / 2) + ' L' + rr.x + ' ' + (rr.y + rr.h / 2);
        } else if (a.hub || b.hub) {
          var hub = a.hub ? a : b, o = a.hub ? b : a;
          ax = hub.x + hub.w / 2; ay = hub.y + hub.h; bx = o.x + o.w / 2; by = o.y;
          d = 'M' + ax + ' ' + ay + ' C' + ax + ' ' + (ay + 40) + ' ' + bx + ' ' + (by - 40) + ' ' + bx + ' ' + by;
        } else {
          var lr = a.x <= b.x; ax = lr ? a.x + a.w : a.x; bx = lr ? b.x : b.x + b.w; ay = a.y + a.h / 2; by = b.y + b.h / 2;
          if (a.x === b.x && Math.abs(a.y - b.y) <= ROWH + 1) { var t2 = a.y < b.y ? a : b, b2 = a.y < b.y ? b : a; d = 'M' + (t2.x + t2.w / 2) + ' ' + (t2.y + t2.h) + ' L' + (b2.x + b2.w / 2) + ' ' + b2.y; }
          else if (a.x === b.x) { ax = a.x + a.w; bx = b.x + b.w; d = 'M' + ax + ' ' + ay + ' C' + (ax + 30) + ' ' + ay + ' ' + (bx + 30) + ' ' + by + ' ' + bx + ' ' + by; }
          else d = 'M' + ax + ' ' + ay + ' C' + (ax + (lr ? 40 : -40)) + ' ' + ay + ' ' + (bx + (lr ? -40 : 40)) + ' ' + by + ' ' + bx + ' ' + by;
        }
        var p = el('path', { d: d, 'stroke-dasharray': e.kind === 'same' ? '5 4' : null, stroke: e.kind === 'same' ? C.accent : null }, eg);
        if (e.label) { var t = el('title', {}, p); t.textContent = e.label; }
      });
      L.groups.forEach(function (gr) {
        var p = L.pos[gr.id];
        el('rect', { x: p.x, y: p.y, width: p.w, height: p.h, rx: 8, fill: C.surface, stroke: C.line }, svg);
        var t = el('text', { x: p.x + 10, y: p.y + 17, 'font-size': 13, 'font-weight': 600, fill: C.ink }, svg); t.textContent = trunc(gr.label || gr.id, 26);
        if (gr.sub) { var s = el('text', { x: p.x + 10, y: p.y + 31, 'font-size': 11, fill: C.muted }, svg); s.textContent = trunc(gr.sub, 30); }
      });
      var flagged = {};
      (g.findings || []).forEach(function (f) { (f.nodes || []).forEach(function (id) { flagged[id] = 1; }); });
      vis.forEach(function (n) {
        var p = L.pos[n.id]; if (!p) return;
        var a = el('a', { href: '#', 'class': 'ui-map-node', 'data-id': n.id, 'aria-label': (n.label || n.id) + ', ' + (n.status || 'unknown') + (n.ip ? ', ' + n.ip : '') }, svg);
        var flag = flagged[n.id] && st.sel !== n.id;
        el('rect', { x: p.x, y: p.y, width: p.w, height: p.h, rx: 6, fill: p.hub ? C.surface : C.sunken, stroke: st.sel === n.id ? C.accent : flag ? C.warn : C.line, 'stroke-width': p.hub || st.sel === n.id || flag ? 2 : 1 }, a);
        if (flag) { var fx = el('text', { x: p.x + p.w - 14, y: p.y + p.h / 2 + 4, 'font-size': 13, 'font-weight': 700, fill: C.warn }, a); fx.textContent = '!'; }
        el('circle', { cx: p.x + 12, cy: p.y + p.h / 2, r: 5, fill: C[tone(n.status)] }, a);
        var t = el('text', { x: p.x + 24, y: p.y + p.h / 2 + (n.ip && !p.hub ? -2 : 4), 'font-size': 12, fill: C.ink }, a); t.textContent = trunc(n.label || n.id, p.hub ? 20 : 24);
        if (n.ip && !p.hub) { var ip = el('text', { x: p.x + 24, y: p.y + p.h / 2 + 10, 'font-size': 11, fill: C.muted }, a); ip.textContent = trunc(n.ip + (n.class && n.class !== 'unknown' ? ' \u00b7 ' + n.class : ''), 30); }
        var tt = el('title', {}, a); tt.textContent = (n.label || n.id) + ' (' + (n.status || 'unknown') + ')' + (flagged[n.id] ? ': flagged by a finding' : '');
        a.addEventListener('click', function (ev) { ev.preventDefault(); show(n); draw(); });
      });
    }
    function drawList(g, vis) {
      var wrap = h('div', 'ui-grid-wrap', null, stage), t = h('table', 'ui-grid', null, wrap);
      var hr = h('tr', null, null, h('thead', null, null, t));
      ['Name', 'Status', 'Kind', 'Class', 'Group', 'IP', 'MAC'].forEach(function (c) { h('th', null, c, hr); });
      var body = h('tbody', null, null, t), gl = {};
      (g.groups || []).forEach(function (x) { gl[x.id] = x.label; });
      vis.forEach(function (n) {
        var r = h('tr', null, null, body), c = h('td', null, null, r), a = h('a', null, n.label || n.id, c);
        a.href = '#'; a.addEventListener('click', function (ev) { ev.preventDefault(); show(n); });
        var s = h('td', null, null, r), b = h('span', 'ui-badge ' + tone(n.status), null, s); h('span', 'ui-dot ' + tone(n.status), null, b); b.appendChild(document.createTextNode(n.status || 'unknown'));
        [n.kind, n.class, gl[n.group] || n.group, n.ip, n.mac].forEach(function (v) { h('td', null, v || '', r); });
      });
    }
    function load() {
      var hdr = opts.headers || {};
      return fetch(opts.src, { headers: hdr, credentials: 'same-origin' }).then(function (r) {
        if (!r.ok) return r.json().catch(function () { return {}; }).then(function (j) { throw new Error(j.error || ('HTTP ' + r.status)); });
        return r.json();
      }).then(function (g) {
        st.graph = g; draw();
        if (st.sel) { var n = (g.nodes || []).filter(function (x) { return x.id === st.sel; })[0]; if (n && !panel.hidden) show(n); }
        if (opts.onLoad) opts.onLoad(g);
        schedule((opts.refresh != null ? opts.refresh : g.refresh_secs) || 0);
      }).catch(function (e) {
        stage.innerHTML = ''; var d = h('div', 'ui-grid-state error', 'Could not load the map: ' + e.message + ' ', stage);
        var b = h('button', 'ui-btn sm', 'Retry', d); b.type = 'button'; b.addEventListener('click', load);
        schedule(30);
      });
    }
    function schedule(s) { clearTimeout(st.timer); if (s > 0) st.timer = setTimeout(function () { if (document.hidden) schedule(s); else load(); }, s * 1000); }
    // the SVG carries resolved colours (so exports look the same): redraw when the scheme changes
    if (window.matchMedia) { var mq = window.matchMedia('(prefers-color-scheme: dark)'); if (mq.addEventListener) mq.addEventListener('change', draw); }
    if (window.MutationObserver) new MutationObserver(draw).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'data-accent'] });
    load();
    return { reload: load, graph: function () { return st.graph; }, stop: function () { clearTimeout(st.timer); } };
  }

  var api = { mount: mount, layout: layout, toMermaid: toMermaid, tone: tone, version: '0.1' };
  if (typeof module === 'object' && module.exports) module.exports = api; else root.UiMap = api;
})(typeof window !== 'undefined' ? window : this);
