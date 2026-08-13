'use strict';

/* ================================================================
   HELPERS
================================================================ */
const $  = (id) => document.getElementById(id);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));
const setText = (id, v) => { const el = $(id); if (el) el.textContent = v; };
const el = (tag, cls, text) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null) n.textContent = text;
  return n;
};
const isMobile = () => window.matchMedia('(max-width:1000px)').matches;

const nfInt = new Intl.NumberFormat('en-US', { maximumFractionDigits: 0 });
const nf2   = new Intl.NumberFormat('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });

function fmtMoney(v, d = 2) {
  if (!isFinite(v)) return '—';
  return '$' + v.toLocaleString('en-US', { minimumFractionDigits: d, maximumFractionDigits: d });
}
function fmtPrice(v) {
  if (!isFinite(v) || v <= 0) return '—';
  const d = v >= 1 ? 4 : v >= 0.01 ? 5 : v >= 0.0001 ? 6 : 8;
  return '$' + v.toFixed(d);
}
function cssVar(name, fallback) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}
function hexA(hex, a) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex);
  if (!m) return hex;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
}
function niceTicks(min, max, count) {
  const span = max - min;
  if (!(span > 0)) return [min];
  const raw = span / Math.max(1, count);
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const norm = raw / mag;
  const step = (norm >= 5 ? 5 : norm >= 2.5 ? 2.5 : norm >= 2 ? 2 : 1) * mag;
  const out = [];
  for (let v = Math.ceil(min / step) * step; v <= max + step * 1e-6; v += step) out.push(v);
  return out;
}

/* dates */
const DAY_MS = 86400000;
function toInputDate(d) {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}
function fromInputDate(v) { return new Date(v + 'T00:00:00'); }
function inputToDDMMYYYY(v) { const [y, m, d] = v.split('-'); return `${d}-${m}-${y}`; }
function ddmmyyyyToUTC(s) {
  const m = /^(\d{2})-(\d{2})-(\d{4})$/.exec(s || '');
  if (!m) return null;
  const d = new Date(Date.UTC(+m[3], +m[2] - 1, +m[1]));
  return isNaN(d.getTime()) ? null : d;
}

/* ================================================================
   LINE CHART — dependency-free canvas chart with tooltip
================================================================ */
class LineChart {
  constructor(canvas, opts = {}) {
    this.cv = canvas;
    this.ctx = canvas.getContext('2d');
    this.o = Object.assign({
      color: '#00b7eb', fill: true, log: false,
      yFmt: (v) => String(v),
      padL: 58, padR: 14, padT: 14, padB: 26,
    }, opts);
    this.raw = [];
    this.pts = [];
    this.hover = -1;
    this._plot = null;

    this.tip = el('div', 'chart-tip');
    const wrap = canvas.parentElement;
    wrap.classList.add('loading');
    wrap.appendChild(this.tip);

    this._ro = new ResizeObserver(() => this.draw());
    this._ro.observe(wrap);
    canvas.addEventListener('mousemove', (e) => this._onMove(e));
    canvas.addEventListener('mouseleave', () => this._clearHover());
  }

  get data() { return this.o.log ? this.raw.filter((p) => p.y > 0) : this.raw; }

  setData(raw) {
    this.raw = raw || [];
    this.hover = -1;
    this.tip.classList.remove('show');
    this.cv.parentElement.classList.remove('loading');
    this.draw();
  }
  setLog(on) { this.o.log = !!on; this.draw(); }

  draw() {
    const wrap = this.cv.parentElement;
    const w = wrap.clientWidth, h = wrap.clientHeight;
    if (w < 40 || h < 40) return;
    const dpr = window.devicePixelRatio || 1;
    if (this.cv.width !== Math.round(w * dpr) || this.cv.height !== Math.round(h * dpr)) {
      this.cv.width = Math.round(w * dpr);
      this.cv.height = Math.round(h * dpr);
    }
    const ctx = this.ctx;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    this.pts = [];
    this._plot = null;

    const muted = cssVar('--muted', '#8b93a7');
    const grid = cssVar('--border', '#242c3d');
    const data = this.data;
    const n = data.length;

    ctx.font = '11px ' + cssVar('--font', 'system-ui');
    if (!n) { this._center(w, h, 'No data yet', muted); return; }
    if (n === 1) { this._center(w, h, this.o.yFmt(data[0].y), muted); return; }

    const pl = this.o.padL, pr = this.o.padR, pt = this.o.padT, pb = this.o.padB;
    const pw = w - pl - pr, ph = h - pt - pb;
    if (pw < 20 || ph < 20) return;

    const tf = this.o.log ? Math.log10 : (v) => v;
    const inv = this.o.log ? (t) => Math.pow(10, t) : (t) => t;

    let ymin = Infinity, ymax = -Infinity;
    for (const p of data) { const t = tf(p.y); if (t < ymin) ymin = t; if (t > ymax) ymax = t; }
    if (!isFinite(ymin) || !isFinite(ymax)) { this._center(w, h, 'No data', muted); return; }
    if (ymin === ymax) { ymin -= 1; ymax += 1; }
    const span = ymax - ymin;
    ymin -= span * 0.06; ymax += span * 0.06;

    const X = (i) => pl + (i / (n - 1)) * pw;
    const Y = (v) => pt + (1 - (tf(v) - ymin) / (ymax - ymin)) * ph;

    /* grid + y labels */
    const yticks = niceTicks(ymin, ymax, 4).map(inv);
    ctx.lineWidth = 1;
    ctx.textAlign = 'right';
    ctx.textBaseline = 'middle';
    for (const tv of yticks) {
      const y = Y(tv);
      if (y < pt - 2 || y > pt + ph + 2) continue;
      ctx.strokeStyle = grid;
      ctx.globalAlpha = 0.65;
      ctx.beginPath(); ctx.moveTo(pl, y); ctx.lineTo(pl + pw, y); ctx.stroke();
      ctx.globalAlpha = 1;
      ctx.fillStyle = muted;
      ctx.fillText(this.o.yFmt(tv), pl - 8, y);
    }

    /* x labels */
    ctx.textAlign = 'center';
    ctx.textBaseline = 'top';
    const xtCount = Math.max(2, Math.min(6, Math.floor(pw / 95)));
    const used = [];
    for (let k = 0; k < xtCount; k++) {
      const i = Math.round(k * (n - 1) / Math.max(1, xtCount - 1));
      const x = X(i);
      if (used.some((u) => Math.abs(u - x) < 46)) continue;
      used.push(x);
      ctx.fillStyle = muted;
      ctx.fillText('Day ' + data[i].x, x, pt + ph + 8);
    }

    /* line path */
    const tracePath = () => {
      ctx.beginPath();
      for (let i = 0; i < n; i++) {
        const x = X(i), y = Y(data[i].y);
        i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
      }
    };

    if (this.o.fill) {
      tracePath();
      ctx.lineTo(pl + pw, pt + ph);
      ctx.lineTo(pl, pt + ph);
      ctx.closePath();
      const g = ctx.createLinearGradient(0, pt, 0, pt + ph);
      g.addColorStop(0, hexA(this.o.color, 0.22));
      g.addColorStop(1, hexA(this.o.color, 0));
      ctx.fillStyle = g;
      ctx.fill();
    }

    tracePath();
    ctx.strokeStyle = this.o.color;
    ctx.lineWidth = 2;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';
    ctx.stroke();

    this.pts = data.map((p, i) => ({ px: X(i), py: Y(p.y), x: p.x, y: p.y }));
    this._plot = { pl, pt, pw, ph };

    /* hover marker */
    if (this.hover >= 0 && this.hover < this.pts.length) {
      const hp = this.pts[this.hover];
      ctx.save();
      ctx.setLineDash([4, 4]);
      ctx.strokeStyle = muted;
      ctx.globalAlpha = 0.6;
      ctx.beginPath(); ctx.moveTo(hp.px, pt); ctx.lineTo(hp.px, pt + ph); ctx.stroke();
      ctx.restore();
      ctx.beginPath();
      ctx.arc(hp.px, hp.py, 4.5, 0, Math.PI * 2);
      ctx.fillStyle = this.o.color;
      ctx.fill();
      ctx.lineWidth = 2;
      ctx.strokeStyle = cssVar('--surface', '#121722');
      ctx.stroke();
    }
  }

  _center(w, h, text, color) {
    const ctx = this.ctx;
    ctx.fillStyle = color;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.font = '13px ' + cssVar('--font', 'system-ui');
    ctx.fillText(text, w / 2, h / 2);
  }

  _onMove(e) {
    if (!this.pts.length || !this._plot) return;
    const r = this.cv.getBoundingClientRect();
    const mx = e.clientX - r.left, my = e.clientY - r.top;
    const { pl, pt, pw, ph } = this._plot;
    if (mx < pl - 12 || mx > pl + pw + 12 || my < 0 || my > pt + ph + 34) {
      this._clearHover();
      return;
    }
    let best = 0, bd = Infinity;
    for (let i = 0; i < this.pts.length; i++) {
      const d = Math.abs(this.pts[i].px - mx);
      if (d < bd) { bd = d; best = i; }
    }
    if (best !== this.hover) { this.hover = best; this.draw(); }
    const p = this.pts[best];
    this.tip.innerHTML =
      `<span class="tip-x">Day ${p.x}</span><span class="tip-y" style="color:${this.o.color}">${this.o.yFmt(p.y)}</span>`;
    const wrapW = this.cv.parentElement.clientWidth;
    const flip = p.px > wrapW * 0.62;
    this.tip.style.left = (p.px + (flip ? -10 : 10)) + 'px';
    this.tip.style.transform = flip ? 'translateX(-100%)' : 'translateX(0)';
    this.tip.style.top = Math.max(6, p.py - 16) + 'px';
    this.tip.classList.add('show');
  }

  _clearHover() {
    if (this.hover !== -1) { this.hover = -1; this.draw(); }
    this.tip.classList.remove('show');
  }
}

/* ================================================================
   STATE
================================================================ */
const state = {
  live: null,
  hexjson: [],
  miners: [],
  totalTShares: 0,
  config: { liveDataFrequency: 15, liquidHEX: 0, historicalStartDay: 1260 },
  chartRange: 'all',
  logScale: false,
  theme: 'dark',
};
const charts = {};

const CHART_DEFS = [
  { id: 'price',  field: 'pricePulseX',         color: '#00b7eb', fmt: (v) => fmtPrice(v) },
  { id: 'tshare', field: 'tshareRateHEX',       color: '#34d399', fmt: (v) => nfInt.format(Math.round(v)) + ' HEX' },
  { id: 'payout', field: 'payoutPerTshareHEX',  color: '#a78bfa', fmt: (v) => v.toFixed(4) + ' HEX' },
  { id: 'daily',  field: 'dailyPayoutHEX',      color: '#fb7185', fmt: (v) => nfInt.format(Math.round(v)) + ' HEX' },
];

/* ================================================================
   API / TOAST / CONFIRM
================================================================ */
async function api(url, opts) {
  const res = await fetch(url, opts);
  if (!res.ok) throw new Error('HTTP ' + res.status);
  const ct = res.headers.get('content-type') || '';
  return ct.includes('json') ? res.json() : null;
}
const JSON_OPTS = { headers: { 'Content-Type': 'application/json' } };

function toast(msg, type = 'success') {
  const box = $('toasts');
  if (!box) return;
  const t = el('div', 'toast ' + type, msg);
  box.appendChild(t);
  setTimeout(() => {
    t.classList.add('out');
    setTimeout(() => t.remove(), 320);
  }, 2600);
}

function confirmDialog(title, msg, okText = 'Confirm') {
  return new Promise((resolve) => {
    const d = $('confirm-dialog');
    let settled = false;
    const done = (v) => { if (!settled) { settled = true; try { d.close(); } catch (e) {} resolve(v); } };
    setText('cf-title', title);
    setText('cf-msg', msg);
    $('cf-ok').textContent = okText;
    $('cf-ok').onclick = () => done(true);
    $('cf-cancel').onclick = () => done(false);
    d.oncancel = () => done(false);
    if (d.open) d.close();
    d.showModal();
  });
}

/* ================================================================
   CONNECTION STATUS + POLLING ENGINE
================================================================ */
const RING_C = 2 * Math.PI * 15.5;
let connMode = '';
let pollTimer = null;
let pollBusy = false;
let nextRefreshAt = Date.now();
let lastOkAt = 0;

const freqMs = () => (state.config.liveDataFrequency || 15) * 60000;

function setConn(mode) {
  if (mode === connMode) return;
  connMode = mode;
  const dot = $('conn-dot');
  dot.className = 'conn-dot ' + (mode === 'live' ? 'live' : mode === 'error' ? 'err' : mode === 'stale' ? 'stale' : '');
  setText('conn-label',
    mode === 'live' ? 'Live' :
    mode === 'error' ? 'Reconnecting' :
    mode === 'stale' ? 'Stale data' : 'Connecting');
}

async function pollOnce() {
  if (pollBusy) return;
  pollBusy = true;
  let ok = false;
  try {
    const d = await api('/api/live-data');
    if (d && d.price_Pulsechain > 0) {
      state.live = d;
      lastOkAt = Date.now();
      ok = true;
      setConn('live');
      renderLive();
    } else {
      setConn('stale');
    }
  } catch (err) {
    console.error('Live data poll failed:', err);
    setConn('error');
  } finally {
    pollBusy = false;
    const wait = ok ? freqMs() : 20000; /* retry quickly on failure */
    nextRefreshAt = Date.now() + wait;
    clearTimeout(pollTimer);
    pollTimer = setTimeout(pollOnce, wait);
  }
}

function tickCountdown() {
  const rem = Math.max(0, nextRefreshAt - Date.now());
  const s = Math.ceil(rem / 1000);
  setText('countdown', `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`);
  const frac = Math.max(0, Math.min(1, rem / freqMs()));
  const ring = $('ring-fg');
  if (ring) ring.style.strokeDashoffset = String(RING_C * (1 - frac));
  const wrap = $('refresh-wrap');
  if (wrap) wrap.title = lastOkAt
    ? 'Last update ' + new Date(lastOkAt).toLocaleTimeString()
    : 'Waiting for data…';
  if (connMode === 'live' && lastOkAt && Date.now() - lastOkAt > freqMs() * 2 + 60000) setConn('stale');
}

/* ================================================================
   RENDERERS
================================================================ */
function refreshNote() {
  const f = state.config.liveDataFrequency || 15;
  setText('lv-note', `Auto-refreshes every ${f} minute${f === 1 ? '' : 's'}.`);
}

function renderLive() {
  const d = state.live;
  if (!d) return;
  const ok = d.price_Pulsechain > 0;

  const priceTxt    = ok ? fmtPrice(d.price_Pulsechain) : '—';
  const tsharePTxt  = ok && d.tsharePrice_Pulsechain > 0 ? fmtPrice(d.tsharePrice_Pulsechain) : '—';
  const rateTxt     = ok ? nfInt.format(Math.round(d.tshareRateHEX_Pulsechain || 0)) + ' HEX' : '—';
  const payoutTxt   = ok ? (d.payoutPerTshare_Pulsechain || 0).toFixed(4) + ' HEX' : '—';
  const penaltiesTxt= ok ? nfInt.format(Math.round(d.penaltiesHEX_Pulsechain || 0)) + ' HEX' : '—';
  const beatTxt     = ok ? nfInt.format(Math.round(d.beat || 0)) : '—';
  const updatedTxt  = 'Updated ' + new Date().toLocaleTimeString();

  /* desktop header ticker */
  setText('tick-price', priceTxt);
  setText('tick-tshare-price', tsharePTxt);
  setText('tick-tshare-rate', rateTxt);
  setText('tick-payout', payoutTxt);
  setText('tick-penalties', penaltiesTxt);
  setText('tick-beat', beatTxt);
  setText('last-updated', updatedTxt);

  /* mobile Live tab */
  setText('lv-price', priceTxt);
  setText('lv-tshare-price', tsharePTxt);
  setText('lv-tshare-rate', rateTxt);
  setText('lv-payout', payoutTxt);
  setText('lv-penalties', penaltiesTxt);
  setText('lv-beat', beatTxt);
  setText('lv-updated', updatedTxt);
  refreshNote();

  document.title = ok ? `HEX Stats · ${fmtPrice(d.price_Pulsechain)}` : 'HEX Stats';
  renderPriceDelta();
  renderStats();
}

function renderPriceDelta() {
  const withPrice = state.hexjson.filter((e) => e.pricePulseX > 0);
  const targets = ['tick-price-delta', 'lv-price-delta'];
  if (withPrice.length < 2) { targets.forEach((id) => setText(id, '')); return; }
  const a = withPrice[withPrice.length - 2].pricePulseX;
  const b = withPrice[withPrice.length - 1].pricePulseX;
  const pct = ((b - a) / a) * 100;
  const txt = (pct >= 0 ? '▲ ' : '▼ ') + Math.abs(pct).toFixed(2) + '%';
  const cls = 'tick-delta ' + (pct >= 0 ? 'up' : 'down');
  targets.forEach((id) => {
    const e = $(id);
    if (e) { e.textContent = txt; e.className = cls; }
  });
}

function renderStats() {
  const lv = state.live;
  const ok = lv && lv.price_Pulsechain > 0;
  const price = ok ? lv.price_Pulsechain : 0;
  const tsp = ok ? (lv.tsharePrice_Pulsechain || 0) : 0;
  const ppt = ok ? (lv.payoutPerTshare_Pulsechain || 0) : 0;
  const rate = ok ? (lv.tshareRateHEX_Pulsechain || 0) : 0;
  const active = state.miners.filter((m) => m.status !== 'completed');

  setText('ov-total-value', ok ? fmtMoney(state.totalTShares * tsp) : '—');
  setText('ov-total-sub', ok ? `1 T-Share = ${fmtPrice(tsp)}` : '');
  setText('ov-liquid-value', ok ? fmtMoney(state.config.liquidHEX * price) : '—');
  setText('ov-liquid-sub', nf2.format(state.config.liquidHEX) + ' HEX');
  setText('ov-tshares', nf2.format(state.totalTShares));
  setText('ov-tshares-sub', `${active.length} active miner${active.length === 1 ? '' : 's'}`);

  const iHex = state.totalTShares * ppt;
  setText('ov-interest-hex', ok ? nf2.format(iHex) + ' HEX' : '—');
  setText('ov-interest-usd', ok ? '≈ ' + fmtMoney(iHex * price) : '');

  setText('inc-daily-hex', ok ? nf2.format(iHex) + ' HEX' : '—');
  setText('inc-daily-usd', ok ? '≈ ' + fmtMoney(iHex * price) : '');
  setText('inc-yearly-hex', ok ? nfInt.format(Math.round(iHex * 365)) + ' HEX' : '—');
  setText('inc-yearly-usd', ok ? '≈ ' + fmtMoney(iHex * 365 * price) : '');
  const apy = rate > 0 && ppt > 0 ? (ppt * 365 / rate) * 100 : null;
  setText('inc-apy', apy !== null ? apy.toFixed(1) + '%' : '—');
}

function applyRange(sorted) {
  if (state.chartRange === 'all') return sorted;
  return sorted.slice(-parseInt(state.chartRange, 10));
}

function renderCharts() {
  const data = applyRange(state.hexjson);
  setText('range-info', data.length
    ? `Day ${data[0].currentDay} → Day ${data[data.length - 1].currentDay} · ${data.length} days`
    : '');
  CHART_DEFS.forEach((def) => {
    const pts = data.map((e) => ({ x: e.currentDay, y: e[def.field] }));
    if (!charts[def.id]) {
      charts[def.id] = new LineChart($('ch-' + def.id), { color: def.color, yFmt: def.fmt });
    }
    charts[def.id].setLog(state.logScale);
    charts[def.id].setData(pts);
    const last = pts.length ? pts[pts.length - 1].y : null;
    setText('ch-' + def.id + '-val', last === null ? '—' : def.fmt(last));
  });
}

function renderPortfolio() {
  setText('hist-range-tag', `since day ${state.config.historicalStartDay}`);
  if (!charts.portfolio) {
    charts.portfolio = new LineChart($('portfolioChart'), {
      color: '#00b7eb',
      yFmt: (v) => '$' + nfInt.format(Math.round(v)),
    });
  }
  const filtered = state.hexjson.filter((e) => e.currentDay >= state.config.historicalStartDay);
  if (!filtered.length) {
    charts.portfolio.setData([]);
    ['hist-start-value', 'hist-current-value', 'hist-ath', 'hist-growth'].forEach((id) => setText(id, '—'));
    setText('hist-growth-pct', '');
    return;
  }
  const series = filtered.map((e) => ({
    x: e.currentDay,
    y: state.totalTShares * e.tshareRateHEX * e.pricePulseX + state.config.liquidHEX * e.pricePulseX,
  }));
  charts.portfolio.setData(series);

  const start = series[0].y;
  const curr = series[series.length - 1].y;
  const ath = series.reduce((m, p) => Math.max(m, p.y), 0);
  const growth = curr - start;
  const pct = start > 0 ? (growth / start) * 100 : 0;

  setText('hist-start-value', fmtMoney(start));
  setText('hist-current-value', fmtMoney(curr));
  setText('hist-ath', fmtMoney(ath));
  const g = $('hist-growth');
  g.textContent = (growth >= 0 ? '+' : '−') + fmtMoney(Math.abs(growth));
  g.className = 'hs-value ' + (growth >= 0 ? 'pos' : 'neg');
  const p = $('hist-growth-pct');
  p.textContent = (growth >= 0 ? '+' : '−') + Math.abs(pct).toFixed(2) + '%';
  p.className = 'hs-pct ' + (growth >= 0 ? 'pos' : 'neg');
}

/* ---------- miners ---------- */
function minerProgress(m) {
  const s = ddmmyyyyToUTC(m.startDate), e = ddmmyyyyToUTC(m.endDate);
  if (!s || !e) return { pct: 0, daysLeft: 0, total: 0, matured: false };
  const now = Date.now();
  const total = Math.max(0, Math.round((e.getTime() - s.getTime()) / DAY_MS));
  const elapsed = Math.min(total, Math.max(0, Math.round((now - s.getTime()) / DAY_MS)));
  const matured = now >= e.getTime();
  const daysLeft = matured ? 0 : Math.max(0, Math.ceil((e.getTime() - now) / DAY_MS));
  return { pct: total > 0 ? Math.min(100, (elapsed / total) * 100) : 100, daysLeft, total, matured };
}

function endMiner(index) {
  confirmDialog('End Miner', 'Have you ended the mining contract and minted HEX?', 'End Miner')
    .then(async (yes) => {
      if (!yes) return;
      try {
        await api('/api/end-miner', Object.assign({ method: 'POST', body: JSON.stringify({ index }) }, JSON_OPTS));
        toast('Miner ended successfully');
        await loadMiners();
      } catch (e) {
        toast('Network error', 'danger');
      }
    });
}

function deleteMiner(index) {
  confirmDialog('Delete Miner', 'Delete this HEX miner? This cannot be undone.', 'Delete')
    .then(async (yes) => {
      if (!yes) return;
      try {
        await api('/api/delete-miner', Object.assign({ method: 'POST', body: JSON.stringify({ index }) }, JSON_OPTS));
        toast('Miner deleted');
        await loadMiners();
      } catch (e) {
        toast('Network error', 'danger');
      }
    });
}

function deleteBtn(index, label) {
  const b = el('button', 'btn btn-ghost btn-sm icon-only', '✕');
  b.title = label;
  b.setAttribute('aria-label', label);
  b.addEventListener('click', () => deleteMiner(index));
  return b;
}

function minerCard(m) {
  const p = minerProgress(m);
  const card = el('div', 'miner-card');

  const top = el('div', 'miner-top');
  const dates = el('div', 'miner-dates');
  const d1 = el('div'); d1.appendChild(el('span', 'md-label', 'Start')); d1.appendChild(el('span', 'md-value', m.startDate));
  const d2 = el('div'); d2.appendChild(el('span', 'md-label', 'End'));   d2.appendChild(el('span', 'md-value', m.endDate));
  dates.append(d1, d2);
  const ts = el('div', 'miner-ts');
  ts.appendChild(el('span', 'md-label', 'T-Shares'));
  ts.appendChild(el('strong', null, nf2.format(+m.tShares || 0)));
  top.append(dates, ts);

  const wrap = el('div');
  const info = el('div', 'progress-info');
  const left = el('span');
  if (p.matured) left.innerHTML = '<strong>Matured</strong>';
  else left.innerHTML = `<strong>${p.daysLeft}</strong> day${p.daysLeft === 1 ? '' : 's'} left · ${p.total}d term`;
  info.appendChild(left);
  info.appendChild(el('span', 'pct', p.pct.toFixed(1) + '%'));
  const bar = el('div', 'progress');
  const fill = el('div', 'progress-fill' + (p.matured ? ' matured' : ''));
  fill.style.width = p.pct.toFixed(1) + '%';
  bar.appendChild(fill);
  wrap.append(info, bar);

  const actions = el('div', 'miner-actions');
  if (p.matured) {
    actions.appendChild(el('span', 'badge matured', 'Matured'));
    const endB = el('button', 'btn btn-danger btn-sm', 'End Miner');
    endB.addEventListener('click', () => endMiner(m._i));
    actions.appendChild(endB);
  } else {
    actions.appendChild(el('span', 'badge active', 'Active'));
  }
  actions.appendChild(deleteBtn(m._i, 'Delete miner'));

  card.append(top, wrap, actions);
  return card;
}

function renderMiners() {
  const indexed = state.miners.map((m, i) => Object.assign({}, m, { _i: i, tShares: +m.tShares || 0 }));
  const active = indexed.filter((m) => m.status !== 'completed');
  const completed = indexed.filter((m) => m.status === 'completed');
  state.totalTShares = active.reduce((s, m) => s + m.tShares, 0);

  /* summary */
  setText('ms-active-count', String(active.length));
  setText('ms-total-tshares', nf2.format(state.totalTShares));
  setText('ms-matured-count', String(active.filter((m) => minerProgress(m).matured).length));
  const running = active
    .filter((m) => !minerProgress(m).matured)
    .sort((a, b) => (ddmmyyyyToUTC(a.endDate) || 0) - (ddmmyyyyToUTC(b.endDate) || 0));
  if (running.length) {
    const nxt = running[0];
    setText('ms-next-maturity', nxt.endDate);
    setText('ms-next-sub', 'in ' + minerProgress(nxt).daysLeft + ' days');
  } else {
    setText('ms-next-maturity', active.length ? 'All matured' : '—');
    setText('ms-next-sub', '');
  }
  setText('active-count-tag', active.length ? active.length + ' active' : '');

  /* active grid */
  const grid = $('active-miners');
  grid.innerHTML = '';
  if (!active.length) {
    grid.appendChild(el('div', 'empty', 'No miners yet. Add your first HEX miner above.'));
  } else {
    active.forEach((m) => grid.appendChild(minerCard(m)));
  }

  /* completed list */
  setText('completed-count', String(completed.length));
  const cl = $('completed-list');
  cl.innerHTML = '';
  if (!completed.length) {
    cl.appendChild(el('div', 'empty sm', 'No completed miners.'));
  } else {
    completed.forEach((m) => {
      const row = el('div', 'list-row');
      const txt = el('span');
      txt.appendChild(document.createTextNode(m.startDate + ' → ' + m.endDate + ' · '));
      txt.appendChild(el('strong', null, nf2.format(m.tShares) + ' TS'));
      row.append(txt, deleteBtn(m._i, 'Delete completed miner'));
      cl.appendChild(row);
    });
  }

  renderOverviewMinis(active);
}

function renderOverviewMinis(active) {
  const box = $('ov-active-miners');
  box.innerHTML = '';
  if (!active.length) {
    box.appendChild(el('div', 'empty sm', 'No active miners — add one in the Miners tab.'));
    return;
  }
  const sorted = [...active].sort(
    (a, b) => ((ddmmyyyyToUTC(a.endDate) || new Date(8640000000000000)).getTime() -
               (ddmmyyyyToUTC(b.endDate) || new Date(8640000000000000)).getTime())
  );
  sorted.slice(0, 3).forEach((m) => {
    const p = minerProgress(m);
    const row = el('div', 'mini-row');
    row.appendChild(el('span', 'mini-dates', m.startDate + ' → ' + m.endDate));
    const bar = el('div', 'mini-bar');
    const fill = el('div', 'mini-fill' + (p.matured ? ' matured' : ''));
    fill.style.width = p.pct.toFixed(1) + '%';
    bar.appendChild(fill);
    row.appendChild(bar);
    row.appendChild(el('span', 'mini-ts', nf2.format(m.tShares) + ' TS'));
    box.appendChild(row);
  });
  if (active.length > 3) box.appendChild(el('div', 'mini-more', '+ ' + (active.length - 3) + ' more…'));
}

/* ================================================================
   LOADERS
================================================================ */
async function loadMiners() {
  try {
    const data = await api('/api/miners');
    state.miners = Array.isArray(data) ? data : [];
  } catch (e) {
    console.error('Failed to load miners:', e);
    state.miners = [];
  }
  renderMiners();
  renderStats();
  renderPortfolio();
}

async function loadHexjson() {
  try {
    const raw = await api('/api/hexjson');
    state.hexjson = (Array.isArray(raw) ? raw : [])
      .map((e) => ({
        currentDay: Math.round(+e.currentDay) || 0,
        tshareRateHEX: +e.tshareRateHEX || 0,
        dailyPayoutHEX: +e.dailyPayoutHEX || 0,
        payoutPerTshareHEX: +e.payoutPerTshareHEX || 0,
        pricePulseX: +e.pricePulseX || 0,
      }))
      .filter((e) => e.currentDay > 0)
      .sort((a, b) => a.currentDay - b.currentDay);
  } catch (e) {
    console.error('Failed to load hexjson:', e);
    state.hexjson = [];
  }
  renderCharts();
  renderPortfolio();
  renderPriceDelta();
}

/* ================================================================
   MINER FORM
================================================================ */
function clearChipSel() {
  $$('#duration-chips .chip').forEach((c) => c.classList.remove('selected'));
}
function updateDurationLabel() {
  const s = $('m-start').value, e = $('m-end').value, lbl = $('m-duration');
  if (!s || !e) { lbl.textContent = 'Duration: —'; lbl.className = ''; return; }
  const days = Math.round((fromInputDate(e) - fromInputDate(s)) / DAY_MS);
  lbl.textContent = `Duration: ${days} day${days === 1 ? '' : 's'}`;
  lbl.className = days > 390 ? 'warn-text' : '';
}

function bindMinerForm() {
  const s = $('m-start'), e = $('m-end'), t = $('m-tshares'), form = $('miner-form');
  const today = new Date();
  s.value = toInputDate(today);
  s.max = toInputDate(today);

  s.addEventListener('change', () => { clearChipSel(); updateDurationLabel(); });
  e.addEventListener('change', () => { clearChipSel(); updateDurationLabel(); });

  $('duration-chips').addEventListener('click', (ev) => {
    const b = ev.target.closest('button[data-days]');
    if (!b) return;
    const base = s.value ? fromInputDate(s.value) : new Date();
    const end = new Date(base);
    end.setDate(end.getDate() + (+b.dataset.days));
    e.value = toInputDate(end);
    clearChipSel();
    b.classList.add('selected');
    updateDurationLabel();
  });

  form.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    const err = $('m-error');
    err.textContent = '';
    const fail = (msg) => { err.textContent = msg; };

    const sd = s.value, ed = e.value, ts = parseFloat(t.value);
    if (!sd || !ed) return fail('Please choose both start and end dates.');
    const ds = fromInputDate(sd), de = fromInputDate(ed);
    if (ds > new Date()) return fail('Start date cannot be in the future.');
    if (de <= ds) return fail('End date must be after start date.');
    const days = Math.round((de - ds) / DAY_MS);
    if (days > 5555) return fail('Maximum stake length is 5555 days.');
    if (!isFinite(ts) || ts <= 0) return fail('Enter a valid T-Shares amount.');

    const btn = $('m-submit');
    btn.disabled = true;
    try {
      await api('/api/add-miner', Object.assign({
        method: 'POST',
        body: JSON.stringify({ startDate: inputToDDMMYYYY(sd), endDate: inputToDDMMYYYY(ed), tShares: ts }),
      }, JSON_OPTS));
      toast('Miner added');
      t.value = '';
      e.value = '';
      clearChipSel();
      updateDurationLabel();
      await loadMiners();
    } catch (x) {
      toast('Failed to add miner', 'danger');
    } finally {
      btn.disabled = false;
    }
  });
}

/* ================================================================
   SETTINGS
================================================================ */
async function saveConfig(patch) {
  try {
    const cfg = Object.assign({}, state.config, patch);
    await api('/api/config', Object.assign({ method: 'POST', body: JSON.stringify(cfg) }, JSON_OPTS));
    state.config = cfg;
    toast('Settings saved');
    refreshNote();
    if ('liquidHEX' in patch || 'historicalStartDay' in patch) {
      renderStats();
      renderPortfolio();
    }
  } catch (e) {
    toast('Failed to save settings', 'danger');
  }
}

function bindSettings() {
  $('save-frequency').addEventListener('click', () => {
    const v = parseInt($('set-frequency').value, 10);
    if (!(v >= 1 && v <= 1440)) return toast('Frequency must be 1–1440 minutes', 'danger');
    saveConfig({ liveDataFrequency: v });
  });
  $('save-liquid').addEventListener('click', () => {
    const v = parseFloat($('set-liquid').value);
    if (!isFinite(v) || v < 0) return toast('Liquid HEX must be ≥ 0', 'danger');
    saveConfig({ liquidHEX: v });
  });
  $('save-histday').addEventListener('click', () => {
    const v = parseInt($('set-histday').value, 10);
    if (!(v >= 1)) return toast('Starting day must be ≥ 1', 'danger');
    saveConfig({ historicalStartDay: v });
  });
  $('export-csv').addEventListener('click', () => {
    if (!state.hexjson.length) return toast('No data to export', 'danger');
    const cols = ['currentDay', 'tshareRateHEX', 'dailyPayoutHEX', 'payoutPerTshareHEX', 'pricePulseX'];
    const rows = [cols.join(',')].concat(
      state.hexjson.map((e) => cols.map((c) => e[c]).join(','))
    );
    const blob = new Blob([rows.join('\n')], { type: 'text/csv;charset=utf-8' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = 'hex-history-' + toInputDate(new Date()) + '.csv';
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(a.href);
    toast('CSV downloaded');
  });
}

/* ================================================================
   TABS / THEME / KEYBOARD
================================================================ */
function switchTab(name) {
  $$('.tab-btn').forEach((b) => b.classList.toggle('active', b.dataset.tab === name));
  $$('.tab-panel').forEach((p) => p.classList.toggle('active', p.id === 'tab-' + name));
}

function bindTabs() {
  $$('.tab-btn').forEach((b) => b.addEventListener('click', () => switchTab(b.dataset.tab)));
  $('manage-miners-btn').addEventListener('click', () => switchTab('miners'));

  window.addEventListener('keydown', (ev) => {
    if (ev.altKey || ev.ctrlKey || ev.metaKey) return;
    if (ev.target && ev.target.matches && ev.target.matches('input,textarea,select')) return;
    const map = { '1': 'overview', '2': 'charts', '3': 'miners', '4': 'settings' };
    if (map[ev.key]) switchTab(map[ev.key]);
    else if (ev.key === '5') { if (isMobile()) switchTab('live'); }
    else if (ev.key === 't' || ev.key === 'T') toggleTheme();
  });

  /* if rotated/resized to desktop while on the mobile-only Live tab, fall back */
  window.addEventListener('resize', () => {
    if (!isMobile()) {
      const active = document.querySelector('.tab-btn.active');
      if (active && active.dataset.tab === 'live') switchTab('overview');
    }
  });
}

function applyTheme(t) {
  state.theme = t;
  document.documentElement.setAttribute('data-theme', t);
  try { localStorage.setItem('hexstats-theme', t); } catch (e) { /* private mode */ }
  Object.values(charts).forEach((c) => c.draw());
}
function toggleTheme() { applyTheme(state.theme === 'dark' ? 'light' : 'dark'); }
function bindTheme() {
  $('theme-toggle').addEventListener('click', toggleTheme);
  let saved = null;
  try { saved = localStorage.getItem('hexstats-theme'); } catch (e) { /* ignore */ }
  if (saved === 'light' || saved === 'dark') state.theme = saved;
  document.documentElement.setAttribute('data-theme', state.theme);
}

/* ================================================================
   CHART TOOLBAR
================================================================ */
function bindChartToolbar() {
  $('range-seg').addEventListener('click', (ev) => {
    const b = ev.target.closest('button[data-range]');
    if (!b) return;
    state.chartRange = b.dataset.range;
    $$('#range-seg button').forEach((x) => x.classList.toggle('active', x === b));
    renderCharts();
  });
  $('scale-btn').addEventListener('click', () => {
    state.logScale = !state.logScale;
    const b = $('scale-btn');
    b.setAttribute('aria-pressed', String(state.logScale));
    renderCharts();
  });
}

/* ================================================================
   INIT
================================================================ */
async function init() {
  bindTheme();
  bindTabs();
  bindChartToolbar();
  bindMinerForm();
  bindSettings();
  setInterval(tickCountdown, 500);

  try {
    const cfg = await api('/api/config');
    if (cfg) {
      state.config = {
        liveDataFrequency: Math.max(1, parseInt(cfg.liveDataFrequency, 10) || 15),
        liquidHEX: Math.max(0, parseFloat(cfg.liquidHEX) || 0),
        historicalStartDay: Math.max(1, parseInt(cfg.historicalStartDay, 10) || 1260),
      };
    }
  } catch (e) { console.error('Config load failed:', e); }

  $('set-frequency').value = state.config.liveDataFrequency;
  $('set-liquid').value = state.config.liquidHEX || '';
  $('set-histday').value = state.config.historicalStartDay;
  refreshNote();

  loadMiners();
  loadHexjson();
  pollOnce();
}

document.addEventListener('DOMContentLoaded', init);
