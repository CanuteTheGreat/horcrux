/* errant ui-kit v0.1 - ui.js (optional, no dependencies, ES2017). Progressive enhancement via data-ui-*:
   <table class="ui-grid" data-ui-grid="vms">  th[data-sort] sorts (data-type="num" for numbers), aria-sort set
   <input data-ui-filter="vms">                filters that grid's rows by text
   <details class="ui-menu" data-ui-columns="vms"> with <label><input type=checkbox value=col> - hides
                                                td/th[data-col=col]; choice kept in localStorage ui.cols.<id>
   <div class="ui-tabs" data-ui-tabs>          [role=tab][aria-controls] switch [role=tabpanel]
   <div class="ui-log" data-ui-log>            follows the end while the user is at the bottom
   License GPL-3.0-or-later. */
(function () {
  "use strict";
  var $$ = function (s, r) { return Array.prototype.slice.call((r || document).querySelectorAll(s)); };
  function store(k, v) { try { if (v === undefined) return localStorage.getItem(k); localStorage.setItem(k, v); } catch (e) { return null; } }

  function grid(t) {
    var id = t.getAttribute("data-ui-grid"), body = t.tBodies[0];
    if (!body) return;
    $$("th[data-sort]", t).forEach(function (th) {
      th.tabIndex = 0;
      var go = function () {
        var col = th.cellIndex, num = th.getAttribute("data-type") === "num";
        var asc = th.getAttribute("aria-sort") !== "ascending";
        $$("th[data-sort]", t).forEach(function (o) { o.setAttribute("aria-sort", "none"); });
        th.setAttribute("aria-sort", asc ? "ascending" : "descending");
        var rows = $$(":scope > tr", body);
        rows.sort(function (a, b) {
          var x = (a.cells[col] || {}).textContent || "", y = (b.cells[col] || {}).textContent || "";
          var r = num ? (parseFloat(x) || 0) - (parseFloat(y) || 0) : x.localeCompare(y, undefined, { numeric: true, sensitivity: "base" });
          return asc ? r : -r;
        });
        rows.forEach(function (r) { body.appendChild(r); });
      };
      th.addEventListener("click", go);
      th.addEventListener("keydown", function (e) { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); go(); } });
    });
    $$('[data-ui-filter="' + id + '"]').forEach(function (inp) {
      inp.addEventListener("input", function () {
        var q = inp.value.toLowerCase();
        $$(":scope > tr", body).forEach(function (r) { r.hidden = q && r.textContent.toLowerCase().indexOf(q) < 0; });
      });
    });
    $$('[data-ui-columns="' + id + '"]').forEach(function (menu) {
      var hidden = (store("ui.cols." + id) || "").split(",").filter(Boolean);
      var apply = function () {
        $$("[data-col]", t).forEach(function (c) { c.hidden = hidden.indexOf(c.getAttribute("data-col")) >= 0; });
      };
      $$("input[type=checkbox]", menu).forEach(function (cb) {
        cb.checked = hidden.indexOf(cb.value) < 0;
        cb.addEventListener("change", function () {
          hidden = hidden.filter(function (h) { return h !== cb.value; });
          if (!cb.checked) hidden.push(cb.value);
          store("ui.cols." + id, hidden.join(","));
          apply();
        });
      });
      apply();
    });
  }

  function tabs(box) {
    var all = $$('[role="tab"]', box);
    all.forEach(function (tab, i) {
      tab.addEventListener("click", function () {
        all.forEach(function (o) {
          var on = o === tab;
          o.setAttribute("aria-selected", on ? "true" : "false");
          o.tabIndex = on ? 0 : -1;
          var p = document.getElementById(o.getAttribute("aria-controls"));
          if (p) p.hidden = !on;
        });
      });
      tab.addEventListener("keydown", function (e) {
        var d = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
        if (d) { var n = all[(i + d + all.length) % all.length]; n.focus(); n.click(); }
      });
    });
  }

  function log(el) {
    var atEnd = true;
    el.addEventListener("scroll", function () { atEnd = el.scrollTop + el.clientHeight >= el.scrollHeight - 4; });
    new MutationObserver(function () { if (atEnd) el.scrollTop = el.scrollHeight; }).observe(el, { childList: true });
    el.scrollTop = el.scrollHeight;
  }

  function init(root) {
    $$("table[data-ui-grid]", root).forEach(grid);
    $$("[data-ui-tabs]", root).forEach(tabs);
    $$("[data-ui-log]", root).forEach(log);
  }
  window.uiKit = { init: init };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", function () { init(document); });
  else init(document);
})();
