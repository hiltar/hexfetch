'use strict';

const LS = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); return true; } catch { return false; } },
  del(k) { try { localStorage.removeItem(k); } catch {} }
};
const $ = id => document.getElementById(id);
const setText = (id, txt) => { const e = $(id); if (e) e.textContent = txt; };
const NF = new Map();
function fmt(n, d = 2) {
  const v = Number(n);
  if (!Number.isFinite(v)) return '0';
  const key = d;
  if (!NF.has(key)) NF.set(key, new Intl.NumberFormat(undefined, { minimumFractionDigits: d, maximumFractionDigits: d }));
  return NF.get(key).format(v);
}
const usd = (n, d = 2) => `$${fmt(n, d)}`;
function priceFmt(p) {
  const a = Math.abs(p);
  if (a === 0) return '0.00';
  if (a >= 1) return p.toFixed(2);
  if (a >= 0.01) return p.toFixed(4);
  if (a >= 0.0001) return p.toFixed(6);
  return p.toFixed(8);
}
function parseDateDDMMYYYY(str) {
  if (typeof str !== 'string') return undefined;
  const t = str.trim();
  if (!/^\d{2}-\d{2}-\d{4}$/.test(t)) return undefined;
  const [d, m, y] = t.split('-').map(Number);
  if (d < 1 || d > 31 || m < 1 || m > 12 || y < 2000 || y > 2100) return undefined;
  const dt = new Date(y, m - 1, d);
  if (isNaN(dt.getTime()) || dt.getDate() !== d || dt.getMonth() !== m - 1) return undefined;
  return dt;
}
const DAY_MS = 86400000;
const utcDay = d => Date.UTC(d.getFullYear(), d.getMonth(), d.getDate());
const S = {
  cfg: null, miners: [], hexjson: [], live: null, status: null,
  totalTShares: 0, liquidHEX: 0, historicalStartDay: 1256,
  frequency: 1, nextRefreshAt: 0, loadGen: 0,
  es: null, esFails: 0, pollTimer: null, countdownTimer: null,
  saveQueue: null, saving: false,
  charts: {}, hexETag: LS.get('hexjson-etag') || '',
  live: { chart: null, series: null, type: 'candles', tfMinutes: 1440, all: [], rendered: [], hasRendered: false, serverNow: 0, range: 10080, candle: 15 }
};
window.HEXJSON_CACHE = S.hexjson;
const RING_C = 2 * Math.PI * 16;
function showNotification(message, type = 'success') {
  const c = $('notification-container'); if (!c) return;
  const t = document.createElement('div');
  t.className = `toast ${type}`;
  t.setAttribute('role', type === 'danger' ? 'alert' : 'status');
  const span = document.createElement('span'); span.textContent = message;
  const btn = document.createElement('button');
  btn.className = 'toast-close'; btn.innerHTML = '&times;'; btn.setAttribute('aria-label', 'Close notification');
  t.append(span, btn); c.appendChild(t);
  const remove = () => { clearTimeout(t._tid); t.style.opacity = '0'; setTimeout(() => t.remove(), 250); };
  btn.addEventListener('click', remove);
  t.addEventListener('mouseenter', () => clearTimeout(t._tid));
  t.addEventListener('mouseleave', () => { t._tid = setTimeout(remove, 1500); });
  t._tid = setTimeout(remove, 4000);
}
function showConfirmModal(title, message) {
  return new Promise(resolve => {
    const d = document.createElement('dialog'); d.className = 'modal';
    const id = `dlg-${Date.now()}`;
    const h = document.createElement('header'); h.className = 'modal-header';
    const h3 = document.createElement('h3'); h3.textContent = title; h3.id = id;
    const x = document.createElement('button'); x.className = 'btn-close'; x.textContent = '×';
    x.setAttribute('aria-label', 'Close');
    h.append(h3, x);
    const b = document.createElement('p'); b.className = 'modal-body'; b.textContent = message;
    const f = document.createElement('footer'); f.className = 'modal-footer';
    const cancel = document.createElement('button'); cancel.className = 'btn btn-secondary'; cancel.textContent = 'Cancel';
    const ok = document.createElement('button'); ok.className = 'btn btn-danger'; ok.textContent = 'Confirm';
    f.append(cancel, ok); d.append(h, b, f);
    d.setAttribute('aria-labelledby', id);           // 2.14
    document.body.appendChild(d);
    let done = false;
    const cleanup = r => { if (done) return; done = true; if (d.open) d.close(); d.remove(); resolve(r); };
    x.onclick = () => cleanup(false);
    cancel.onclick = () => cleanup(false);
    ok.onclick = () => cleanup(true);
    d.addEventListener('cancel', e => { e.preventDefault(); cleanup(false); });
    d.showModal(); cancel.focus();
  });
}
function copyToClipboard(text) {
  if (navigator.clipboard && window.isSecureContext) return navigator.clipboard.writeText(text);
  return new Promise((res, rej) => {
    const ta = document.createElement('textarea');
    ta.value = text; ta.setAttribute('readonly', ''); ta.contentEditable = 'true';
    ta.style.cssText = 'position:fixed;top:0;left:0;opacity:0';
    document.body.appendChild(ta); ta.focus(); ta.select();
    try { ta.setSelectionRange(0, text.length); } catch {}
    let ok = false; try { ok = document.execCommand('copy'); } catch {}
    ta.remove(); ok ? res() : rej(new Error('copy failed'));
  });
}
async function fetchHexJson() {
  const headers = {}; if (S.hexETag) headers['If-None-Match'] = S.hexETag;
  let res = await fetch('/api/hexjson', { headers, cache: 'no-cache' });
  if (res.status === 304) {
    const cached = LS.get('hexjson-cache');
    if (cached) { try { return JSON.parse(cached); } catch { LS.del('hexjson-cache'); LS.del('hexjson-etag'); S.hexETag = ''; } }
    res = await fetch('/api/hexjson', { cache: 'no-store' });
  }
  if (!res.ok) throw new Error(`hexjson HTTP ${res.status}`);
  const text = await res.text();
  let data;
  try { data = JSON.parse(text); } catch { throw new Error('Server returned malformed history JSON'); }
  if (!Array.isArray(data)) throw new Error('Unexpected history payload');
  const etag = res.headers.get('ETag') || '';
  if (LS.set('hexjson-cache', text)) { etag ? LS.set('hexjson-etag', etag) : LS.del('hexjson-etag'); }
  S.hexETag = etag;
  return data;
}
function setConnectionStatus(status) {
  const pill = $('connection-status'), label = $('connection-label');
  if (pill) pill.dataset.status = status;
  if (label) label.textContent = status === 'online' ? 'Live' : status === 'offline' ? 'Off' : 'Conn';
  const tt = document.querySelector('.ticker-title'); if (tt) tt.dataset.status = status;
}
function updateCountdown() {
  const total = Math.max(1, S.frequency) * 60000;
  const ms = Math.max(0, S.nextRefreshAt - Date.now());
  const s = Math.floor(ms / 1000);
  setText('countdown', `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`);
  const ring = $('ring-progress');
  if (ring) {
    ring.style.strokeDashoffset = (RING_C * (1 - Math.min(1, ms / total))).toFixed(2);
    ring.classList.toggle('urgent', ms > 0 && ms <= 10000);
  }
}
function startCountdown() {
  if (S.countdownTimer) return;
  S.countdownTimer = setInterval(updateCountdown, 1000);
}
function scheduleFromLive(d) {
  const base = (Number(d?.updatedAt) || Math.floor(Date.now() / 1000)) * 1000;
  S.nextRefreshAt = base + S.frequency * 60000;
  updateCountdown();
}
function isValidLive(d) { return d && Number.isFinite(Number(d.price_Pulsechain)) && Number(d.price_Pulsechain) > 0; }
function applyLive(d) {
  if (!isValidLive(d)) return;
  S.live = d;
  const price = Number(d.price_Pulsechain) || 0;
  setText('price', usd(price, 5));
  setText('tshare-price', usd(Number(d.tsharePrice_Pulsechain) || 0, 2));
  setText('tshare-rate', `${fmt(Math.floor(Number(d.tshareRateHEX_Pulsechain) || 0), 0)} HEX`);
  setText('payout', `${fmt(Number(d.payoutPerTshare_Pulsechain) || 0, 4)} HEX`);
  setText('penalties', `${fmt(Math.floor(Number(d.penaltiesHEX_Pulsechain) || 0), 0)} HEX`);
  setText('beat', fmt(Math.floor(Number(d.beat) || 0), 0));
  const age = Math.max(0, Math.floor(Date.now() / 1000) - (Number(d.updatedAt) || 0));
  const when = d.updatedAt ? new Date(d.updatedAt * 1000).toLocaleTimeString() : '—';
  setText('last-updated', `Updated ${when}${age > 90 ? ` (${Math.round(age / 60)}m ago)` : ''}${d.stale ? ' • stale' : ''}`);
  document.title = `HEXTRACK - $${price.toFixed(5)}`;
  scheduleFromLive(d);
  setConnectionStatus(d.stale ? 'connecting' : 'online');
  updateProfileStats();
}
function updateProfileStats() {
  const d = S.live; if (!d) return;
  const price = Number(d.price_Pulsechain) || 0;
  const tsharePrice = Number(d.tsharePrice_Pulsechain) || 0;
  const payout = Number(d.payoutPerTshare_Pulsechain) || 0;
  if (d.liquidHEX !== undefined && Number.isFinite(Number(d.liquidHEX))) {
    S.liquidHEX = Number(d.liquidHEX);
    const inp = $('liquid-hex');
    if (inp && document.activeElement !== inp) inp.value = S.liquidHEX;
  }
  setText('total-value', usd(S.totalTShares * tsharePrice, 2));
  setText('total-tshares', fmt(S.totalTShares, 2));
  setText('wallet-hex-value', `${fmt(S.liquidHEX, 0)} HEX`);
  setText('wallet-usd-value', usd(S.liquidHEX * price, 2));
  const iHex = S.totalTShares * payout;
  setText('interest-hex', `${fmt(iHex, 2)} HEX`);
  setText('interest-usd', usd(iHex * price, 2));
}
async function pollLiveOnce() {
  try {
    const r = await fetch('/api/live-data', { cache: 'no-store' });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    applyLive(await r.json());
  } catch (e) { console.error('live poll failed', e); setConnectionStatus('offline'); }
}
async function manualRefresh() {
  showNotification('Refreshing live data…', 'info');
  try {
    const r = await fetch('/api/refresh', { method: 'POST' });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    applyLive(await r.json());
    loadLiveChart();
  } catch { pollLiveOnce(); }
}
function connectEvents() {
  if (!('EventSource' in window)) return startPolling();
  stopPolling();
  const es = new EventSource('/api/events');
  S.es = es;
  es.onopen = () => { S.esFails = 0; setConnectionStatus('online'); };
  es.onmessage = ev => handleEvent(ev.data);
  ['live', 'chart', 'miners', 'config', 'hexJson', 'status'].forEach(name => {
    es.addEventListener(name, ev => handleEvent(ev.data));
  });
  es.onerror = () => {
    setConnectionStatus('connecting');
    if (++S.esFails >= 3) { es.close(); S.es = null; startPolling(); setTimeout(connectEvents, 60000); }
  };
}
function handleEvent(raw) {
  let msg; try { msg = JSON.parse(raw); } catch { return; }
  switch (msg.type) {
    case 'live': applyLive(msg.data); if (chartsVisible()) loadLiveChart(); break;
    case 'chart': if (chartsVisible()) loadLiveChart(); break;
    case 'miners': refreshMiners(); break;
    case 'config': S.cfg = msg.config; applyConfigToForm(msg.config); break;
    case 'hexJson': refreshHistory(); break;
    case 'status': S.status = msg.status; renderStatus(msg.status); break;
  }
}
function startPolling() {
  if (S.pollTimer) return;
  pollLiveOnce();
  S.pollTimer = setInterval(() => { pollLiveOnce(); if (chartsVisible()) loadLiveChart(); }, Math.max(1, S.frequency) * 60000);
}
function stopPolling() { if (S.pollTimer) { clearInterval(S.pollTimer); S.pollTimer = null; } }
function restartPolling() { if (S.pollTimer) { stopPolling(); startPolling(); } }
function applyConfigToForm(cfg) {
  if (!cfg) return;
  S.cfg = cfg;
  S.frequency = Number(cfg.liveDataFrequency) || 1;
  S.historicalStartDay = Number(cfg.historicalStartDay) || 1256;
  S.liquidHEX = Number(cfg.liquidHEX) || 0;
  const setVal = (id, v) => { const e = $(id); if (e && document.activeElement !== e) e.value = v; };
  setVal('frequency', cfg.liveDataFrequency);
  setVal('liquid-hex', cfg.liquidHEX || '');
  setVal('hist-start-day', cfg.historicalStartDay);
  setVal('wallet-addresses', cfg.walletAddresses || '');
  const range = Number(cfg.liveChartRangeMinutes ?? 10080);
  const candle = Number(cfg.liveChartCandleMinutes ?? 15);
  setVal('live-chart-range-hours', Math.round((range / 60) * 100) / 100);
  setVal('live-chart-candle-minutes', candle);
  const ci = $('live-chart-candle-minutes'); if (ci) ci.min = '1';
  S.live.range = range; S.live.candle = candle;
  applyTimeframeFromConfig(range);
  updateWalletAddressesUI(cfg.walletAddresses || '');
  restartPolling(); scheduleFromLive(S.live);
}
function readSection(section) {
  const num = (id, fb) => { const v = parseFloat($(id)?.value); return Number.isFinite(v) ? v : fb; };
  switch (section) {
    case 'frequency': return { liveDataFrequency: Math.round(num('frequency', 1)) };
    case 'liquid': return { liquidHEX: num('liquid-hex', 0) };
    case 'historical': return { historicalStartDay: Math.round(num('hist-start-day', 1256)) };
    case 'wallet': return { walletAddresses: $('wallet-addresses')?.value || '' };
    case 'chart': {
      const hours = num('live-chart-range-hours', 168);
      return {
        liveChartRangeMinutes: Math.max(5, Math.round(hours * 60)),
        liveChartCandleMinutes: Math.max(1, Math.round(num('live-chart-candle-minutes', 15)))
      };
    }
    default: return {};
  }
}
function saveConfigSection(section, btn) {
  const base = S.cfg || {};
  const patch = readSection(section);
  S.saveQueue = { ...base, ...(S.saveQueue || {}), ...patch };
  if (btn) btn.disabled = true;
  if (S.saving) return;
  S.saving = true;
  (async () => {
    try {
      while (S.saveQueue) {
        const payload = S.saveQueue; S.saveQueue = null;
        const r = await fetch('/api/config', {
          method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(payload)
        });
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        const cfg = await (await fetch('/api/config')).json();
        applyConfigToForm(cfg);
        showNotification('Settings saved', 'success');
        if (patch.historicalStartDay !== undefined) refreshHistory();
      }
    } catch (e) { console.error(e); showNotification('Error saving settings', 'danger'); }
    finally { S.saving = false; if (btn) btn.disabled = false; }
  })();
}
const ADDR_RE = /^0x[0-9a-fA-F]{40}$/;
function saveWalletSettings() {
  const raw = $('wallet-addresses')?.value || '';
  const list = raw.split(',').map(s => s.trim()).filter(Boolean);
  const bad = list.filter(a => !ADDR_RE.test(a));
  if (bad.length) return showNotification(`Invalid address: ${bad[0]}`, 'danger');
  saveConfigSection('wallet');
}
function saveFrequency() {
  const v = parseInt($('frequency')?.value, 10);
  if (!(v >= 1 && v <= 1440)) return showNotification('Frequency must be 1–1440 minutes', 'danger');
  saveConfigSection('frequency');
}
function saveLiquidHEX() {
  const v = parseFloat($('liquid-hex')?.value);
  if (!(v >= 0)) return showNotification('Liquid HEX must be ≥ 0', 'danger');
  saveConfigSection('liquid');
}
function saveHistoricalStartDay() {
  const v = parseInt($('hist-start-day')?.value, 10);
  if (!(v >= 1)) return showNotification('Start day must be ≥ 1', 'danger');
  saveConfigSection('historical');
}
function saveLiveChartSettings() { saveConfigSection('chart'); }
Object.assign(window, { saveFrequency, saveLiquidHEX, saveHistoricalStartDay, saveWalletSettings, saveLiveChartSettings });

function updateWalletAddressesUI(str) {
  const c = $('saved-wallet-addresses'); if (!c) return;
  c.innerHTML = '';
  const seen = new Set();
  str.split(',').map(s => s.trim().toLowerCase()).filter(a => ADDR_RE.test(a) && !seen.has(a) && seen.add(a))
    .forEach(addr => {
      const masked = `${addr.slice(0, 6)}…${addr.slice(-4)}`;
      const item = document.createElement('div'); item.className = 'wallet-address-item';
      const span = document.createElement('span'); span.className = 'wallet-address-masked'; span.textContent = masked;
      const btn = document.createElement('button');
      btn.className = 'copy-address-btn'; btn.textContent = 'Copy'; btn.title = 'Copy full address';
      btn.setAttribute('aria-label', `Copy address ${masked}`);
      btn.addEventListener('click', () => copyToClipboard(addr).then(() => {
        showNotification('Address copied', 'success');
        btn.textContent = 'Copied!'; btn.classList.add('copied');
        setTimeout(() => { btn.textContent = 'Copy'; btn.classList.remove('copied'); }, 2000);
      }).catch(() => showNotification('Failed to copy', 'danger')));
      item.append(span, btn); c.appendChild(item);
    });
}
function minerProgress(m) {
  const s = parseDateDDMMYYYY(m.startDate), e = parseDateDDMMYYYY(m.endDate);
  if (!s || !e) return null;
  const start = utcDay(s), end = utcDay(e), now = Date.now();
  const total = Math.max(1, Math.round((end - start) / DAY_MS));
  const elapsed = Math.max(0, Math.min(total, Math.floor((now - start) / DAY_MS)));
  const left = Math.max(0, Math.ceil((end - now) / DAY_MS));
  return { total, elapsed, left, matured: end <= now, pct: Math.min(100, (elapsed / total) * 100) };
}
function renderMiners() {
  const active = S.miners.filter(m => m.status !== 'completed');
  S.totalTShares = active.reduce((a, m) => a + (Number(m.tShares) || 0), 0);
  setText('total-tshares', fmt(S.totalTShares, 2));
  updateProfileStats();
  const byEnd = (a, b) => {
    const ea = parseDateDDMMYYYY(a.endDate)?.getTime() ?? 0, eb = parseDateDDMMYYYY(b.endDate)?.getTime() ?? 0;
    return ea - eb || (parseDateDDMMYYYY(a.startDate)?.getTime() ?? 0) - (parseDateDDMMYYYY(b.startDate)?.getTime() ?? 0);
  };
  const box = $('active-miners');
  if (box) {
    box.innerHTML = '';
    if (!active.length) { const p = document.createElement('p'); p.className = 'empty-msg'; p.textContent = 'No active miners.'; box.appendChild(p); }
    [...active].sort(byEnd).forEach(m => {
      const p = minerProgress(m); if (!p) return;
      const card = document.createElement('div'); card.className = 'miner-card';
      const head = document.createElement('div'); head.className = 'miner-card-header';
      const dates = document.createElement('div'); dates.className = 'miner-dates';
      [['Start Date', m.startDate], ['End Date', m.endDate]].forEach(([l, v]) => {
        const w = document.createElement('div');
        const lab = document.createElement('span'); lab.className = 'miner-date-label'; lab.textContent = l;
        const val = document.createElement('div'); val.className = 'miner-date-value'; val.textContent = v;
        w.append(lab, val); dates.appendChild(w);
      });
      const ts = document.createElement('div'); ts.className = 'miner-tshares';
      const tl = document.createElement('div'); tl.className = 'miner-tshares-label'; tl.textContent = 'T-Shares';
      const tv = document.createElement('div'); tv.className = 'miner-tshares-value'; tv.textContent = fmt(Number(m.tShares) || 0, 2);
      ts.append(tl, tv); head.append(dates, ts);
      const pc = document.createElement('div'); pc.className = 'miner-progress-container';
      const info = document.createElement('div'); info.className = 'miner-progress-info';
      const dl = document.createElement('span'); dl.className = 'miner-days-left';
      const strong = document.createElement('strong'); strong.textContent = p.matured ? 'Matured' : String(p.left);
      dl.appendChild(strong);
      if (!p.matured) dl.appendChild(document.createTextNode(' days remaining'));
      const pct = document.createElement('span'); pct.className = 'miner-percentage'; pct.textContent = `${p.pct.toFixed(1)}%`;
      info.append(dl, pct);
      const bar = document.createElement('div'); bar.className = 'miner-progress-bar';
      const fill = document.createElement('div');
      fill.className = `miner-progress-fill${p.matured ? ' matured' : ''}`;
      fill.style.width = `${p.pct}%`;
      fill.setAttribute('role', 'progressbar');
      fill.setAttribute('aria-valuenow', p.pct.toFixed(0));
      fill.setAttribute('aria-valuemin', '0'); fill.setAttribute('aria-valuemax', '100');
      bar.appendChild(fill); pc.append(info, bar);
      const foot = document.createElement('div'); foot.className = 'miner-card-footer';
      const badge = document.createElement('span');
      badge.className = `miner-status-badge ${p.matured ? 'matured' : 'active'}`;
      badge.textContent = p.matured ? 'Matured' : 'Active';
      foot.appendChild(badge);
      if (m.address) {
        const tag = document.createElement('span'); tag.className = 'miner-source-tag';
        tag.textContent = `on-chain ${m.address.slice(0, 6)}…${m.address.slice(-4)}`;
        foot.appendChild(tag);
      } else if (p.matured) {
        const end = document.createElement('button'); end.className = 'btn btn-sm btn-danger'; end.textContent = 'End Miner';
        end.addEventListener('click', () => endMiner(m.id));
        foot.appendChild(end);
      }
      card.append(head, pc, foot); box.appendChild(card);
    });
  }
  const list = $('existing-miners');
  if (list) {
    list.innerHTML = '';
    [...S.miners].sort(byEnd).forEach(m => {
      const row = document.createElement('div'); row.className = 'list-item';
      const span = document.createElement('span');
      const done = m.status === 'completed' ? ' ✓' : '';
      span.textContent = `${m.startDate} – ${m.endDate} • ${fmt(Number(m.tShares) || 0, 2)} T-Shares${done}`;
      row.appendChild(span);
      if (!m.address) {
        const del = document.createElement('button'); del.className = 'btn btn-sm btn-danger'; del.textContent = 'Delete';
        del.addEventListener('click', () => deleteMiner(m.id));
        row.appendChild(del);
      }
      list.appendChild(row);
    });
  }
}
async function refreshMiners() {
  try {
    const r = await fetch('/api/miners', { cache: 'no-store' });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const m = await r.json();
    S.miners = Array.isArray(m) ? m : [];
    renderMiners();
  } catch (e) { console.error('miners fetch failed', e); }
}
function showCompletedMiners() {
  const list = $('completed-miners-list'); if (!list) return;
  list.innerHTML = '';
  const done = S.miners.filter(m => m.status === 'completed');
  if (!done.length) {
    const li = document.createElement('li'); li.textContent = 'No completed miners.'; list.appendChild(li);
  } else {
    done.forEach(m => {
      const li = document.createElement('li');
      const extra = m.stakedHEX ? ` • staked ${fmt(m.stakedHEX, 0)} HEX` : '';
      li.textContent = `${m.startDate} – ${m.endDate} • ${fmt(Number(m.tShares) || 0, 2)} T-Shares${extra}`;
      list.appendChild(li);
    });
  }
  const modal = $('completed-miners-modal');
  if (modal && !modal.open) modal.showModal();
}
async function endMiner(id) {
  if (!Number.isFinite(Number(id))) return showNotification('Invalid miner ID', 'danger');
  if (!await showConfirmModal('End Miner', 'Have you ended the mining contract and minted HEX?')) return;
  await minerAction('/api/end-miner', id, 'Miner ended');
}
async function deleteMiner(id) {
  if (!Number.isFinite(Number(id))) return showNotification('Invalid miner ID', 'danger');
  if (!await showConfirmModal('Delete Miner', 'Delete this HEX miner?')) return;
  await minerAction('/api/delete-miner', id, 'Miner deleted');
}
async function minerAction(url, id, okMsg) {
  try {
    const r = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: Number(id) }) });
    showNotification(r.ok ? okMsg : 'Action failed', r.ok ? 'success' : 'danger');
    if (r.ok) refreshMiners();                            // 2.11: targeted, not a full reload
  } catch { showNotification('Network error', 'danger'); }
}
Object.assign(window, { endMiner, deleteMiner, showCompletedMiners });
function submitAddMiner() {
  const sd = $('start-date')?.value, ed = $('end-date')?.value;
  const ts = parseFloat($('tshares')?.value);
  if (!sd || !ed || !Number.isFinite(ts) || ts <= 0) return showNotification('Fill all fields with valid data', 'danger');
  const s = parseDateDDMMYYYY(sd), e = parseDateDDMMYYYY(ed);
  if (!s || !e) return showNotification('Dates must be DD-MM-YYYY', 'danger');
  if (e < s) return showNotification('End date must be after start date', 'danger');
  const btn = $('add-miner-btn'); if (btn) btn.disabled = true;
  fetch('/api/add-miner', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ startDate: sd, endDate: ed, tShares: ts }) })
    .then(r => {
      if (r.ok) {
        showNotification('Miner added', 'success');
        ['start-date', 'end-date', 'tshares'].forEach(id => { const el = $(id); if (el) el.value = ''; });
        refreshMiners();
      } else showNotification(r.status === 409 ? 'That miner already exists' : 'Error adding miner', 'danger');
    })
    .catch(() => showNotification('Network error', 'danger'))
    .finally(() => { if (btn) btn.disabled = false; });
}
window.submitAddMiner = submitAddMiner;
const chartsVisible = () => $('charts')?.classList.contains('active');
const profileVisible = () => $('profile')?.classList.contains('active');
async function refreshHistory() {
  try {
    S.hexjson = await fetchHexJson();
    window.HEXJSON_CACHE = S.hexjson;
    renderVisibleCharts();
  } catch (e) {
    console.error(e);
    showNotification('Could not load price history', 'danger');
  }
}
function baseChartOptions() {
  return {
    animation: false, responsive: true, maintainAspectRatio: false,
    interaction: { intersect: false, mode: 'index' },
    scales: { x: { ticks: { color: '#9ca3af', maxTicksLimit: 12 }, grid: { color: '#374151' } },
              y: { ticks: { color: '#9ca3af' }, grid: { color: '#374151' } } },
    plugins: { legend: { display: false }, tooltip: { backgroundColor: '#1a1d23', titleColor: '#fff', bodyColor: '#fff' } }
  };
}
function renderHistoryCharts() {
  if (!window.Chart || !S.hexjson.length) return;
  const sorted = [...S.hexjson].sort((a, b) => a.currentDay - b.currentDay);
  const last = sorted[sorted.length - 1] || {};
  setText('price-value', last.pricePulseX ? usd(last.pricePulseX, 5) : '$0.00000');
  setText('tshare-rate-value', `${fmt(last.tshareRateHEX || 0, 0)} HEX`);
  setText('payout-per-tshare-value', `${fmt(last.payoutPerTshareHEX || 0, 4)} HEX`);
  setText('daily-payout-value', `${fmt(last.dailyPayoutHEX || 0, 0)} HEX`);
  const defs = [
    ['priceChart', 'HEX Price', 'pricePulseX', '#00b7eb'],
    ['tshareRateChart', 'T-Share Rate', 'tshareRateHEX', '#00cc99'],
    ['payoutPerTshareChart', 'Payout Per T-Share', 'payoutPerTshareHEX', '#9966ff'],
    ['dailyPayoutChart', 'Daily Payout', 'dailyPayoutHEX', '#ff6f61']
  ];
  defs.forEach(([id, label, field, color]) => {
    const canvas = $(id); if (!canvas) return;
    const labels = sorted.map(e => e.currentDay);
    const values = sorted.map(e => Number(e[field]) || 0);
    if (S.charts[id]) {
      S.charts[id].data.labels = labels;
      S.charts[id].data.datasets[0].data = values;
      S.charts[id].update('none');
    } else {
      S.charts[id] = new Chart(canvas.getContext('2d'), {
        type: 'line',
        data: { labels, datasets: [{ label, data: values, borderColor: color, fill: false, pointRadius: 0, pointHoverRadius: 5, tension: 0.25, spanGaps: true }] },
        options: baseChartOptions()
      });
    }
  });
}
function renderPortfolioChart() {
  if (!window.Chart || !S.hexjson.length) return;
  const rows = [...S.hexjson].sort((a, b) => a.currentDay - b.currentDay)
    .filter(e => e.currentDay >= S.historicalStartDay)
    .map(e => ({ day: e.currentDay, value: S.totalTShares * (e.tshareRateHEX * e.pricePulseX) + S.liquidHEX * e.pricePulseX }));
  if (!rows.length) return;
  const start = rows[0].value, curr = rows[rows.length - 1].value;
  const ath = rows.reduce((a, r) => Math.max(a, r.value), 0);
  const growth = curr - start, pct = start > 0 ? (growth / start) * 100 : 0;
  setText('hist-start-day-label', String(S.historicalStartDay));
  setText('hist-start-value', usd(start, 0));
  setText('hist-current-value', usd(curr, 0));
  setText('hist-ath', usd(ath, 0));
  const g = $('hist-growth'), p = $('hist-growth-pct');
  const col = growth >= 0 ? 'var(--success)' : 'var(--danger)';
  if (g) { g.textContent = `${growth >= 0 ? '+' : ''}${usd(growth, 0)}`; g.style.color = col; }
  if (p) { p.textContent = `${growth >= 0 ? '+' : ''}${fmt(pct, 2)}%`; p.style.color = col; }
  const canvas = $('historicalValueChart'); if (!canvas) return;
  const labels = rows.map(r => r.day), values = rows.map(r => r.value);
  if (S.charts.historicalValueChart) {
    const c = S.charts.historicalValueChart;
    c.data.labels = labels; c.data.datasets[0].data = values; c.update('none');
  } else {
    const o = baseChartOptions();
    o.scales.x.title = { display: true, text: 'Day', color: '#9ca3af' };
    o.scales.y.title = { display: true, text: 'Portfolio Value', color: '#9ca3af' };
    o.scales.y.ticks.callback = v => usd(v, 0);
    o.plugins.tooltip.callbacks = { label: c => usd(c.parsed.y, 0) };
    S.charts.historicalValueChart = new Chart(canvas.getContext('2d'), {
      type: 'line',
      data: { labels, datasets: [{ label: 'Portfolio Value', data: values, borderColor: '#00b7eb', backgroundColor: 'rgba(0,183,235,0.15)', borderWidth: 3, tension: 0.25, fill: true, pointRadius: 0, pointHoverRadius: 5, spanGaps: true }] },
      options: o
    });
  }
}
function renderVisibleCharts() {
  if (chartsVisible()) renderHistoryCharts();
  if (profileVisible()) renderPortfolioChart();
  requestAnimationFrame(() => Object.values(S.charts).forEach(c => c?.resize?.()));
}
function initLiveChart() {
  const el = $('live-chart-container');
  if (!el || typeof LightweightCharts === 'undefined' || S.live.chart) return;
  S.live.chart = LightweightCharts.createChart(el, {
    autoSize: true,
    layout: { background: { color: '#1a1d23' }, textColor: '#e4e6eb' },
    grid: { vertLines: { color: '#374151' }, horzLines: { color: '#374151' } },
    timeScale: { timeVisible: true, secondsVisible: false },
    rightPriceScale: { borderColor: '#374151' },
    crosshair: { mode: LightweightCharts.CrosshairMode.Normal }
  });
  createLiveSeries();
  S.live.chart.subscribeCrosshairMove(param => {
    const legend = $('live-chart-legend'); if (!legend) return;
    const d = param?.seriesData?.get(S.live.series);
    if (!param?.time || !d) return setLegendLatest(S.live.rendered);
    renderLegend(legend, d, param.time);
  });
}
function addSeriesCompat(kind, opts) {
  const c = S.live.chart;
  if (typeof c.addSeries === 'function') {
    const map = { candlestick: LightweightCharts.CandlestickSeries, line: LightweightCharts.LineSeries, area: LightweightCharts.AreaSeries };
    if (map[kind]) { try { return c.addSeries(map[kind], opts); } catch {} }
  }
  const legacy = { candlestick: 'addCandlestickSeries', line: 'addLineSeries', area: 'addAreaSeries' }[kind];
  return typeof c[legacy] === 'function' ? c[legacy](opts) : null;
}
function createLiveSeries() {
  const c = S.live.chart; if (!c) return;
  if (S.live.series) { try { c.removeSeries(S.live.series); } catch {} }
  const priceFormat = { type: 'custom', minMove: 0.00000001, formatter: priceFmt };
  const t = S.live.type;
  S.live.series = t === 'line'
    ? addSeriesCompat('line', { color: '#00b7eb', lineWidth: 2, priceFormat })
    : t === 'area'
      ? addSeriesCompat('area', { lineColor: '#00b7eb', topColor: 'rgba(0,183,235,0.35)', bottomColor: 'rgba(0,183,235,0.02)', lineWidth: 2, priceFormat })
      : addSeriesCompat('candlestick', { upColor: '#22c55e', downColor: '#ef4444', borderVisible: false, wickUpColor: '#22c55e', wickDownColor: '#ef4444', priceFormat });
}
async function loadLiveChart() {
  if (!S.live.series) return;
  const range = S.live.tfMinutes;
  const interval = Math.max(1, Math.min(S.live.candle, Math.ceil(range / 1000)));
  try {
    const r = await fetch(`/api/live-chart?range=${range}&interval=${interval}`, { cache: 'no-store' });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const d = await r.json();
    S.live.serverNow = Number(d.now) || Math.floor(Date.now() / 1000);
    S.live.all = (d.candles || []).map(c => ({ time: c.t, open: c.o, high: c.h, low: c.l, close: c.c }));
    const fit = !S.live.hasRendered;
    S.live.hasRendered = true;
    renderLiveView(fit);
  } catch (e) { console.error('live chart', e); }
}
const candleEq = (a, b) => a.time === b.time && a.open === b.open && a.high === b.high && a.low === b.low && a.close === b.close;
function incrementalOk(next, prev) {
  if (!prev.length) return false;
  if (next.length === prev.length) return prev.slice(0, -1).every((c, i) => candleEq(c, next[i]));
  if (next.length === prev.length + 1) return prev.every((c, i) => candleEq(c, next[i]));
  return false;
}
const mapForType = c => S.live.type === 'candles' ? c : { time: c.time, value: c.close };
function renderLiveView(fit = false) {
  if (!S.live.series || !S.live.chart) return;
  const visible = S.live.all;
  updateLiveStats(visible);
  setLegendLatest(visible);
  if (!fit && incrementalOk(visible, S.live.rendered)) {
    S.live.series.update(mapForType(visible[visible.length - 1]));
  } else {
    const prev = fit ? null : S.live.chart.timeScale().getVisibleLogicalRange();
    S.live.series.setData(visible.map(mapForType));
    if (fit) S.live.chart.timeScale().fitContent();
    else if (prev) { try { S.live.chart.timeScale().setVisibleLogicalRange(prev); } catch {} }
  }
  S.live.rendered = visible;
}
function updateLiveStats(v) {
  if (!$('lc-stat-price')) return;
  if (!v.length) {
    ['lc-stat-price', 'lc-stat-change', 'lc-stat-high', 'lc-stat-low'].forEach(id => setText(id, '–'));
    $('lc-stat-change')?.classList.remove('up', 'down');
    return;
  }
  const first = v[0], last = v[v.length - 1];
  const chg = first.open > 0 ? ((last.close - first.open) / first.open) * 100 : 0;
  const hi = v.reduce((a, c) => Math.max(a, c.high), -Infinity);
  const lo = v.reduce((a, c) => Math.min(a, c.low), Infinity);
  setText('lc-stat-price', priceFmt(last.close));
  setText('lc-stat-change', `${chg >= 0 ? '+' : ''}${chg.toFixed(2)}%`);
  setText('lc-stat-high', priceFmt(hi));
  setText('lc-stat-low', priceFmt(lo));
  const ce = $('lc-stat-change');
  if (ce) { ce.classList.toggle('up', chg >= 0); ce.classList.toggle('down', chg < 0); }
}
function legendTime(t) {
  const d = new Date(t * 1000);
  const hm = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  return d.toDateString() === new Date().toDateString() ? hm
    : `${d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })} ${hm}`;
}
function renderLegend(el, d, time) {
  const parts = [`<span class="lcl-time">${legendTime(time)}</span>`];
  if (S.live.type === 'candles' && typeof d.open === 'number') {
    const chg = d.open ? ((d.close - d.open) / d.open) * 100 : 0;
    [['O', d.open], ['H', d.high], ['L', d.low], ['C', d.close]].forEach(([k, val]) =>
      parts.push(`<span class="lcl-item"><b>${k}</b>${priceFmt(val)}</span>`));
    parts.push(`<span class="lcl-item lcl-change ${chg >= 0 ? 'up' : 'down'}">${chg >= 0 ? '+' : ''}${chg.toFixed(2)}%</span>`);
  } else {
    parts.push(`<span class="lcl-item">${priceFmt(typeof d.value === 'number' ? d.value : d.close)}</span>`);
  }
  el.innerHTML = parts.join('');
}
function setLegendLatest(v) {
  const el = $('live-chart-legend'); if (!el) return;
  if (!v.length) { el.innerHTML = '<span class="lcl-time">Waiting for data…</span>'; return; }
  const last = v[v.length - 1];
  renderLegend(el, S.live.type === 'candles' ? last : { value: last.close }, last.time);
}
function switchChartType(type) {
  if (!type || type === S.live.type) return;
  S.live.type = type;
  document.querySelectorAll('.chart-type-btn').forEach(b => b.classList.toggle('active', b.dataset.type === type));
  if (!S.live.chart) return;
  createLiveSeries(); S.live.rendered = []; renderLiveView(false);
}
function setTimeframe(minutes) {
  if (!minutes || minutes === S.live.tfMinutes) return;
  S.live.tfMinutes = minutes;
  document.querySelectorAll('.timeframe-btn').forEach(b => b.classList.toggle('active', Number(b.dataset.minutes) === minutes));
  S.live.rendered = [];
  loadLiveChart();
}
function applyTimeframeFromConfig(rangeMinutes) {
  const btns = [...document.querySelectorAll('.timeframe-btn')];
  btns.forEach(b => {                                    // 2.9: never show a window the server can't serve
    const m = Number(b.dataset.minutes);
    const ok = m <= rangeMinutes;
    b.disabled = !ok;
    b.title = ok ? '' : 'Increase the saved time range to use this window';
  });
  const usable = btns.map(b => Number(b.dataset.minutes)).filter(m => m <= rangeMinutes);
  const target = usable.includes(S.live.tfMinutes) ? S.live.tfMinutes : (usable.length ? Math.max(...usable) : rangeMinutes);
  S.live.tfMinutes = target;
  btns.forEach(b => b.classList.toggle('active', Number(b.dataset.minutes) === target));
  if (S.live.series) loadLiveChart();
}
Object.assign(window, { switchChartType, setTimeframe });
function renderStatus(s) {
  const el = $('status-panel'); if (!el || !s) return;
  const bf = s.backfill?.running ? ` • backfill ${s.backfill.phase} ${s.backfill.done}/${s.backfill.total}` : '';
  el.textContent =
    `v${s.version} • up ${Math.round(s.uptime_secs / 60)}m • ${s.multicall ? 'multicall' : 'batch'} • ` +
    `rpc ${s.active_rpc.replace('https://', '')} • history ${s.hexjson_days} days (to ${s.hexjson_last_day}) • ` +
    `samples ${s.live_points} @${s.sample_secs}s • miners ${s.miners_active}/${s.miners_completed}${bf}`;
}
async function loadStatus() {
  try { const r = await fetch('/api/status'); if (r.ok) { S.status = await r.json(); renderStatus(S.status); } } catch {}
}
let datepickersReady = false;
function initAirDatepickers() {
  if (datepickersReady || typeof AirDatepicker === 'undefined') return;
  const locale = {
    days: ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'],
    daysShort: ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'],
    daysMin: ['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa'],
    months: ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'],
    monthsShort: ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'],
    today: 'Today', clear: 'Clear', dateFormat: 'dd-MM-yyyy', firstDay: 1
  };
  const common = { locale, dateFormat: 'dd-MM-yyyy', autoClose: true, classes: 'air-datepicker-dark' };
  const sEl = $('start-date'), eEl = $('end-date');
  if (!sEl || !eEl) return;
  const end = new AirDatepicker(eEl, { ...common });
  const start = new AirDatepicker(sEl, {
    ...common, minDate: new Date(2019, 11, 2), maxDate: new Date(),
    onSelect: ({ date }) => {
      if (date instanceof Date && !isNaN(date)) {
        end.update({ minDate: date });
        const cur = parseDateDDMMYYYY(eEl.value);
        if (cur && cur < date) end.clear();
      }
    }
  });
  datepickersReady = true;
  window._dp = { start, end };
}
async function initialLoad() {
  const gen = ++S.loadGen;
  const results = await Promise.allSettled([
    fetchHexJson(),
    fetch('/api/miners').then(r => r.ok ? r.json() : Promise.reject(new Error('miners'))),
    fetch('/api/config').then(r => r.ok ? r.json() : Promise.reject(new Error('config'))),
    fetch('/api/live-data').then(r => r.ok ? r.json() : Promise.reject(new Error('live')))
  ]);
  if (gen !== S.loadGen) return;
  const [hist, miners, cfg, live] = results;
  if (hist.status === 'fulfilled') { S.hexjson = hist.value; window.HEXJSON_CACHE = S.hexjson; }
  else showNotification('Could not load price history', 'danger');
  if (miners.status === 'fulfilled' && Array.isArray(miners.value)) S.miners = miners.value;
  else showNotification('Could not load miners', 'danger');
  if (cfg.status === 'fulfilled') applyConfigToForm(cfg.value);
  else { S.frequency = 1; showNotification('Could not load settings', 'danger'); }
  renderMiners();
  if (live.status === 'fulfilled') applyLive(live.value);
  startCountdown();
  scheduleFromLive(S.live);
  connectEvents();
  renderVisibleCharts();
  loadStatus();
  initAirDatepickers();
}

// ---------- init ----------
document.addEventListener('DOMContentLoaded', () => {
  // ticker toggle
  const tbtn = $('ticker-toggle'), wrap = $('live-ticker-wrap');
  if (tbtn && wrap) {
    const collapsed = LS.get('hex-ticker-collapsed') === '1';
    wrap.classList.toggle('collapsed', collapsed);
    tbtn.setAttribute('aria-expanded', String(!collapsed));
    tbtn.addEventListener('click', () => {
      const now = wrap.classList.toggle('collapsed');
      tbtn.setAttribute('aria-expanded', String(!now));
      LS.set('hex-ticker-collapsed', now ? '1' : '0');
    });
  }
  // tabs
  const tabs = [...document.querySelectorAll('.tab-btn')];
  const panels = [...document.querySelectorAll('.tab-content')];
  function activate(btn) {
    tabs.forEach(b => { b.classList.remove('active'); b.setAttribute('aria-selected', 'false'); b.tabIndex = -1; });
    panels.forEach(p => p.classList.remove('active'));
    btn.classList.add('active'); btn.setAttribute('aria-selected', 'true'); btn.tabIndex = 0;
    const panel = $(btn.dataset.tab); if (panel) panel.classList.add('active');
    if (btn.dataset.tab === 'charts') { renderHistoryCharts(); initLiveChart(); loadLiveChart(); }
    if (btn.dataset.tab === 'profile') renderPortfolioChart();
    if (btn.dataset.tab === 'settings') loadStatus();
    requestAnimationFrame(() => Object.values(S.charts).forEach(c => c?.resize?.()));
  }
  tabs.forEach((btn, i) => {
    btn.addEventListener('click', () => activate(btn));
    btn.addEventListener('keydown', e => {
      const map = { ArrowRight: (i + 1) % tabs.length, ArrowLeft: (i - 1 + tabs.length) % tabs.length, Home: 0, End: tabs.length - 1 };
      if (!(e.key in map)) return;
      e.preventDefault(); tabs[map[e.key]].focus(); activate(tabs[map[e.key]]);
    });
  });
  const bind = (id, fn) => { const b = $(id); if (b && !b.hasAttribute('onclick')) b.addEventListener('click', fn); };
  bind('add-miner-btn', submitAddMiner);
  bind('show-completed-btn', showCompletedMiners);
  bind('save-frequency-btn', saveFrequency);
  bind('save-liquid-btn', saveLiquidHEX);
  bind('save-hist-btn', saveHistoricalStartDay);
  bind('save-wallet-btn', saveWalletSettings);
  bind('save-live-chart-settings-btn', saveLiveChartSettings);
  document.querySelectorAll('.modal-close-btn, .btn-close').forEach(b =>
    b.addEventListener('click', () => b.closest('dialog')?.close()));
  document.querySelectorAll('.chart-type-btn').forEach(b => b.addEventListener('click', () => switchChartType(b.dataset.type)));
  document.querySelectorAll('.timeframe-btn').forEach(b => b.addEventListener('click', () => setTimeframe(Number(b.dataset.minutes))));
  const timer = document.querySelector('.refresh-timer');
  if (timer) {
    if (timer.tagName !== 'BUTTON') { timer.setAttribute('role', 'button'); timer.tabIndex = 0; }
    timer.title = 'Refresh live data';
    timer.setAttribute('aria-label', 'Refresh live data now');
    timer.addEventListener('click', manualRefresh);
    timer.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); manualRefresh(); } });
  }
  const cd = $('countdown'); if (cd) cd.setAttribute('aria-live', 'off');
  initAirDatepickers();
  if (!datepickersReady) {
    const iv = setInterval(() => { initAirDatepickers(); if (datepickersReady) clearInterval(iv); }, 150);
    setTimeout(() => clearInterval(iv), 8000);
  }
  window.addEventListener('beforeunload', () => { S.es?.close(); });
  initialLoad();
});
