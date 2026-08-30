let chartInstances = {
    priceChart: null,
    tshareRateChart: null,
    payoutPerTshareChart: null,
    dailyPayoutChart: null,
    historicalValueChart: null
};
let userTotalTShares = 0;
let userLiquidHEX = 0;
let historicalStartDay = 1260;
let liveDataCache = null;
let countdownIntervalId = null;
let nextRefreshTime = Date.now();
let currentFrequency = 15;
let saveInProgress = false;
let scheduledNextFetch = null;
let hasConnectedOnce = false;
let liveFetchGeneration = 0;
let hexJsonETag = localStorage.getItem('hexjson-etag') || '';
window.HEXJSON_CACHE = [];
let datepickers = {
    startDate: null,
    endDate: null
};
const RING_CIRCUMFERENCE = 2 * Math.PI * 16;

// =============================================
// HELPERS
// =============================================

function safeParseArray(text) {
    try {
        const parsed = JSON.parse(text);
        return Array.isArray(parsed) ? parsed : [];
    } catch {
        return [];
    }
}

function cacheHexJsonRaw(text, etag) {
    try {
        localStorage.setItem('hexjson-cache', text);
        if (etag) {
            localStorage.setItem('hexjson-etag', etag);
        } else {
            localStorage.removeItem('hexjson-etag');
        }
    } catch (e) {
        console.warn('Could not cache hexjson locally, likely quota exceeded.', e);
        try {
            localStorage.removeItem('hexjson-cache');
            localStorage.removeItem('hexjson-etag');
        } catch {}
    }
}

async function fetchHexJsonWithCache() {
    const headers = {};
    if (hexJsonETag) {
        headers['If-None-Match'] = hexJsonETag;
    }

    let res = await fetch('/api/hexjson', { headers, cache: 'no-cache' });

    if (res.status === 304) {
        const cached = localStorage.getItem('hexjson-cache');
        if (cached) {
            try { return JSON.parse(cached); } 
            catch {
                localStorage.removeItem('hexjson-cache');
                localStorage.removeItem('hexjson-etag');
                hexJsonETag = '';
            }
        }
        const forceRes = await fetch('/api/hexjson', { cache: 'no-store' });
        if (!forceRes.ok) throw new Error(`HTTP ${forceRes.status}`);
        const text = await forceRes.text();
        const data = safeParseArray(text);
        const newEtag = forceRes.headers.get('ETag') || '';
        cacheHexJsonRaw(text, newEtag);
        hexJsonETag = newEtag;
        return data;
    }

    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const text = await res.text();
    const data = safeParseArray(text);
    const newEtag = res.headers.get('ETag') || '';
    cacheHexJsonRaw(text, newEtag);
    hexJsonETag = newEtag;
    return data;
}

function formatWithCommas(num) {
    const n = Number(num);
    if (!Number.isFinite(n)) return '0';
    return n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

function parseDateDDMMYYYY(str) {
    if (!str || typeof str !== 'string') return undefined;
    const trimmed = str.trim();
    if (!/^\d{2}-\d{2}-\d{4}$/.test(trimmed)) return undefined;
    const [d, m, y] = trimmed.split('-').map(Number);
    if (d < 1 || d > 31 || m < 1 || m > 12 || y < 2000 || y > 2100) return undefined;
    const date = new Date(y, m - 1, d);
    if (isNaN(date.getTime()) || date.getDate() !== d || date.getMonth() !== m - 1) return undefined;
    return date;
}

function formatDateDDMMYYYY(date) {
    if (!(date instanceof Date) || isNaN(date)) return '';
    const d = String(date.getDate()).padStart(2, '0');
    const m = String(date.getMonth() + 1).padStart(2, '0');
    const y = date.getFullYear();
    return `${d}-${m}-${y}`;
}

function showNotification(message, type = 'success') {
    const container = document.getElementById('notification-container');
    if (!container) return;
    const toast = document.createElement('div');
    toast.className = `toast ${type}`;
    toast.setAttribute('role', type === 'danger' ? 'alert' : 'status');
    toast.setAttribute('aria-live', type === 'danger' ? 'assertive' : 'polite');
    
    const msgSpan = document.createElement('span');
    msgSpan.textContent = message;
    toast.appendChild(msgSpan);

    const closeBtn = document.createElement('button');
    closeBtn.className = 'toast-close';
    closeBtn.innerHTML = '&times;';
    closeBtn.setAttribute('aria-label', 'Close notification');
    
    toast.appendChild(closeBtn);
    container.appendChild(toast);

    const removeToast = () => {
        if (toast._timeoutId) clearTimeout(toast._timeoutId);
        toast.style.opacity = '0';
        toast.style.transform = 'translateX(100%)';
        setTimeout(() => toast.remove(), 300);
    };

    closeBtn.addEventListener('click', removeToast);
    toast._timeoutId = setTimeout(removeToast, 4000);
}

function showConfirmModal(title, message) {
    return new Promise(resolve => {
        const dialog = document.createElement('dialog');
        dialog.className = 'modal';
        const header = document.createElement('header');
        header.className = 'modal-header';
        const h3 = document.createElement('h3');
        h3.textContent = title;
        const closeBtn = document.createElement('button');
        closeBtn.className = 'btn-close';
        closeBtn.setAttribute('aria-label', 'Close');
        closeBtn.textContent = '×';
        header.appendChild(h3);
        header.appendChild(closeBtn);
        const body = document.createElement('p');
        body.className = 'modal-body';
        body.textContent = message;
        const footer = document.createElement('footer');
        footer.className = 'modal-footer';
        const cancelBtn = document.createElement('button');
        cancelBtn.className = 'btn btn-secondary confirm-cancel';
        cancelBtn.textContent = 'Cancel';
        const confirmBtn = document.createElement('button');
        confirmBtn.className = 'btn btn-danger confirm-action';
        confirmBtn.textContent = 'Confirm';
        footer.appendChild(cancelBtn);
        footer.appendChild(confirmBtn);
        dialog.appendChild(header);
        dialog.appendChild(body);
        dialog.appendChild(footer);
        document.body.appendChild(dialog);
        dialog.showModal();
        
        // Focus cancel button for safety against accidental confirmation
        cancelBtn.focus(); 
        
        const cleanup = result => { dialog.close(); dialog.remove(); resolve(result); };
        closeBtn.onclick = () => cleanup(false);
        cancelBtn.onclick = () => cleanup(false);
        confirmBtn.onclick = () => cleanup(true);
        dialog.addEventListener('cancel', () => cleanup(false));
    });
}

function isValidLiveData(data) {
    if (!data) return false;
    const price = Number(data.price_Pulsechain);
    return Number.isFinite(price) && price > 0;
}

// =============================================
// AIR DATEPICKER
// =============================================

function initAirDatepickers() {
    const localeEn = {
        days: ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'],
        daysShort: ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'],
        daysMin: ['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa'],
        months: ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'],
        monthsShort: ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'],
        today: 'Today', clear: 'Clear', dateFormat: 'dd-MM-yyyy', firstDay: 1
    };
    const commonOpts = {
        locale: localeEn, dateFormat: 'dd-MM-yyyy', autoClose: true, classes: 'air-datepicker-dark',
        onShow: (isFinished, dp) => { if (isFinished && dp?.$datepicker) dp.$datepicker.style.setProperty('z-index', '1050', 'important'); },
        onSelect: ({ datepicker }) => { datepicker.$el.dispatchEvent(new Event('change', { bubbles: true })); }
    };
    const startDateEl = document.getElementById('start-date');
    if (startDateEl && typeof AirDatepicker !== 'undefined') {
        if (datepickers.startDate) datepickers.startDate.destroy();
        datepickers.startDate = new AirDatepicker(startDateEl, {
            ...commonOpts, minDate: new Date(2021, 0, 1), maxDate: new Date(),
            onSelect: ({ datepicker, date }) => {
                commonOpts.onSelect?.({ datepicker, formattedDate: datepicker.$el.value });
                if (datepickers.endDate && date instanceof Date && !isNaN(date)) datepickers.endDate.update({ minDate: date });
            }
        });
    }
    const endDateEl = document.getElementById('end-date');
    if (endDateEl && typeof AirDatepicker !== 'undefined') {
        if (datepickers.endDate) datepickers.endDate.destroy();
        let initialMinDate = undefined;
        if (startDateEl?.value) {
            const parsed = parseDateDDMMYYYY(startDateEl.value);
            if (parsed instanceof Date && !isNaN(parsed)) initialMinDate = parsed;
        }
        const endDateOpts = { ...commonOpts };
        if (initialMinDate) endDateOpts.minDate = initialMinDate;
        datepickers.endDate = new AirDatepicker(endDateEl, endDateOpts);
    }
}

// =============================================
// TAB & CHART MANAGEMENT
// =============================================

function renderAllCharts() {
    if (!window.Chart || !window.HEXJSON_CACHE || window.HEXJSON_CACHE.length === 0) return;
    renderChartsWithData(window.HEXJSON_CACHE);
    renderPortfolioHistoryChartWithData(window.HEXJSON_CACHE);
    requestAnimationFrame(() => { Object.values(chartInstances).forEach(c => { if (c && typeof c.resize === 'function') c.resize(); }); });
}

function createBaseChartOptions() {
    return {
        animation: true, responsive: true, maintainAspectRatio: false,
        interaction: { intersect: false, mode: 'index' },
        scales: {
            x: { ticks: { color: '#9ca3af', maxTicksLimit: 12 }, grid: { color: '#374151' } },
            y: { ticks: { color: '#9ca3af' }, grid: { color: '#374151' } }
        },
        plugins: { legend: { display: false }, tooltip: { backgroundColor: '#1a1d23', titleColor: '#fff', bodyColor: '#fff' } }
    };
}

function renderChartsWithData(data) {
    if (!window.Chart || !Array.isArray(data) || data.length === 0) return;
    const sorted = [...data].sort((a, b) => a.currentDay - b.currentDay);
    const latest = sorted[sorted.length - 1] || {};
    const priceEl = document.getElementById('price-value');
    if (priceEl) priceEl.textContent = latest.pricePulseX ? `$${latest.pricePulseX.toFixed(5)}` : '$0.0000';
    const tshareEl = document.getElementById('tshare-rate-value');
    if (tshareEl) tshareEl.textContent = latest.tshareRateHEX ? `${formatWithCommas(latest.tshareRateHEX.toFixed(0))} HEX` : '0 HEX';
    const payoutEl = document.getElementById('payout-per-tshare-value');
    if (payoutEl) payoutEl.textContent = latest.payoutPerTshareHEX ? `${formatWithCommas(latest.payoutPerTshareHEX.toFixed(2))} HEX` : '0.000 HEX';
    const dailyEl = document.getElementById('daily-payout-value');
    if (dailyEl) dailyEl.textContent = latest.dailyPayoutHEX ? `${formatWithCommas(latest.dailyPayoutHEX.toFixed(0))} HEX` : '0 HEX';

    const configs = [
        { id: 'priceChart', label: 'HEX Price', field: 'pricePulseX', border: '#00b7eb', data: sorted },
        { id: 'tshareRateChart', label: 'T-Share Rate', field: 'tshareRateHEX', border: '#00cc99', data: sorted },
        { id: 'payoutPerTshareChart', label: 'Payout Per T-Share', field: 'payoutPerTshareHEX', border: '#9966ff', data: sorted },
        { id: 'dailyPayoutChart', label: 'Daily Payout', field: 'dailyPayoutHEX', border: '#ff6f61', data: sorted }
    ];

    configs.forEach(c => {
        if (!Array.isArray(c.data)) return;
        const labels = c.data.map(e => e.currentDay);
        const values = c.data.map(e => e[c.field]);
        if (chartInstances[c.id]) {
            chartInstances[c.id].data.labels = labels;
            chartInstances[c.id].data.datasets[0].data = values;
            chartInstances[c.id].update('none');
        } else {
            chartInstances[c.id] = new Chart(document.getElementById(c.id).getContext('2d'), {
                type: 'line',
                data: { labels, datasets: [{ label: c.label, data: values, borderColor: c.border, fill: false, pointRadius: 0, pointHoverRadius: 5, tension: 0.25, spanGaps: true }] },
                options: createBaseChartOptions()
            });
        }
    });
}

function renderPortfolioHistoryChartWithData(rawData) {
    if (!window.Chart || !Array.isArray(rawData) || rawData.length === 0) return;
    const sorted = [...rawData].sort((a, b) => a.currentDay - b.currentDay).filter(e => e.currentDay >= historicalStartDay);
    if (sorted.length === 0) return;
    const portfolio = sorted.map(e => ({ day: e.currentDay, value: userTotalTShares * (e.tshareRateHEX * e.pricePulseX) + userLiquidHEX * e.pricePulseX }));
    const start = portfolio[0].value;
    const curr = portfolio[portfolio.length - 1].value;
    const ath = Math.max(...portfolio.map(d => d.value));
    const growth = curr - start;
    const pct = start > 0 ? (growth / start) * 100 : 0;
    const startLbl = document.getElementById('hist-start-day-label');
    if (startLbl) startLbl.textContent = historicalStartDay;
    const startVal = document.getElementById('hist-start-value');
    if (startVal) startVal.textContent = `$${formatWithCommas(start.toFixed(2))}`;
    const currVal = document.getElementById('hist-current-value');
    if (currVal) currVal.textContent = `$${formatWithCommas(curr.toFixed(2))}`;
    const athVal = document.getElementById('hist-ath');
    if (athVal) athVal.textContent = `$${formatWithCommas(ath.toFixed(2))}`;
    const gEl = document.getElementById('hist-growth');
    if (gEl) { gEl.textContent = `${growth >= 0 ? '+' : ''}$${formatWithCommas(growth.toFixed(2))}`; gEl.style.color = growth >= 0 ? 'var(--success)' : 'var(--danger)'; }
    const pEl = document.getElementById('hist-growth-pct');
    if (pEl) { pEl.textContent = `${growth >= 0 ? '+' : ''}${pct.toFixed(2)}%`; pEl.style.color = growth >= 0 ? 'var(--success)' : 'var(--danger)'; }
    const labels = portfolio.map(d => d.day);
    const values = portfolio.map(d => d.value);
    const canvas = document.getElementById('historicalValueChart');
    if (!canvas) return;
    if (chartInstances.historicalValueChart) {
        chartInstances.historicalValueChart.data.labels = labels;
        chartInstances.historicalValueChart.data.datasets[0].data = values;
        chartInstances.historicalValueChart.update('none');
    } else {
        const options = createBaseChartOptions();
        options.scales.x.title = { display: true, text: 'Day', color: '#9ca3af' };
        options.scales.y.title = { display: true, text: 'Portfolio Value (USD)', color: '#9ca3af' };
        options.scales.y.ticks.callback = v => '$' + formatWithCommas(Math.round(v));
        chartInstances.historicalValueChart = new Chart(canvas.getContext('2d'), {
            type: 'line',
            data: { labels, datasets: [{ label: 'Portfolio Value', data: values, borderColor: '#00b7eb', backgroundColor: 'rgba(0,183,235,0.15)', borderWidth: 3, tension: 0.25, fill: true, pointRadius: 0, pointHoverRadius: 5, spanGaps: true }] },
            options
        });
    }
    requestAnimationFrame(() => { if (chartInstances.historicalValueChart) chartInstances.historicalValueChart.resize(); });
}

// =============================================
// PROFILE & LIVE DATA
// =============================================

function updateProfileStats() {
    if (!liveDataCache) return;
    const d = liveDataCache;
    const price = Number(d.price_Pulsechain) || 0;
    const tsharePrice = Number(d.tsharePrice_Pulsechain) || 0;
    const payoutPerTshare = Number(d.payoutPerTshare_Pulsechain) || 0;
    
    if (d.liquidHEX !== undefined) {
        userLiquidHEX = Number(d.liquidHEX);
        const liquidInput = document.getElementById('liquid-hex');
        if (liquidInput && document.activeElement !== liquidInput) {
            liquidInput.value = userLiquidHEX;
        }
    }

    document.getElementById('total-value').textContent = `$${formatWithCommas((userTotalTShares * tsharePrice).toFixed(2))}`;

    const walletHexEl = document.getElementById('wallet-hex-value');
    if (walletHexEl) walletHexEl.textContent = `${formatWithCommas(userLiquidHEX.toFixed(0))} HEX`;
    
    const walletUsdEl = document.getElementById('wallet-usd-value');
    if (walletUsdEl) walletUsdEl.textContent = `$${formatWithCommas((userLiquidHEX * price).toFixed(2))}`;

    const iHex = userTotalTShares * payoutPerTshare;
    document.getElementById('interest-hex').textContent = `${formatWithCommas(iHex.toFixed(2))} HEX`;
    document.getElementById('interest-usd').textContent = `$${formatWithCommas((iHex * price).toFixed(2))}`;
}

async function fetchLiveDataAndRender() {
    const generation = ++liveFetchGeneration;
    if (!hasConnectedOnce) setConnectionStatus('connecting');
    try {
        const res = await fetch('/api/live-data');
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();
        if (!isValidLiveData(data)) throw new Error('Live data invalid or zero price');
        updateLiveDataUI(data);
        hasConnectedOnce = true;
        setConnectionStatus('online');
    } catch (e) {
        console.error('Live data fetch failed', e);
        try {
            const publicRes = await fetch('https://hexdailystats.com/livedata');
            if (publicRes.ok) {
                const publicData = await publicRes.json();
                if (isValidLiveData(publicData)) { updateLiveDataUI(publicData); hasConnectedOnce = true; setConnectionStatus('online'); } 
                else { setConnectionStatus('offline'); }
            } else { setConnectionStatus('offline'); }
        } catch (fbErr) { console.error('Public fallback also failed', fbErr); setConnectionStatus('offline'); }
    } finally {
        if (generation === liveFetchGeneration) {
            nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
            updateCountdown();
            scheduledNextFetch = setTimeout(fetchLiveDataAndRender, currentFrequency * 60 * 1000);
        }
    }
}

function updateLiveDataUI(data) {
    liveDataCache = data;
    const price = Number(data.price_Pulsechain) || 0;
    const tsharePrice = Number(data.tsharePrice_Pulsechain) || 0;
    const tshareRate = Number(data.tshareRateHEX_Pulsechain) || 0;
    const payout = Number(data.payoutPerTshare_Pulsechain) || 0;
    const penalties = Number(data.penaltiesHEX_Pulsechain) || 0;
    const beat = Number(data.beat) || 0;

    document.getElementById('price').textContent = `$${price.toFixed(5)}`;
    document.getElementById('tshare-price').textContent = `$${tsharePrice.toFixed(2)}`;
    document.getElementById('tshare-rate').textContent = `${formatWithCommas(Math.floor(tshareRate.toFixed(0)))} HEX`;
    document.getElementById('payout').textContent = `${payout.toFixed(2)} HEX`;
    document.getElementById('penalties').textContent = `${formatWithCommas(Math.floor(penalties.toFixed(0)))} HEX`;
    document.getElementById('beat').textContent = `${formatWithCommas(Math.floor(beat.toFixed(0)))}`;
    document.getElementById('last-updated').textContent = `Last updated: ${new Date().toLocaleTimeString()}`;
    document.title = `HEXTRACK - $${price.toFixed(5)}`;

    nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
    updateCountdown();
    updateProfileStats();
}

function updateCountdown() {
    const totalMs = currentFrequency * 60 * 1000;
    const ms = Math.max(0, nextRefreshTime - Date.now());
    const s = Math.floor(ms / 1000);
    const el = document.getElementById('countdown');
    if (el) el.textContent = `${Math.floor(s / 60)}:${(s % 60).toString().padStart(2, '0')}`;
    const ring = document.getElementById('ring-progress');
    if (ring) {
        const fraction = totalMs > 0 ? ms / totalMs : 0;
        ring.style.strokeDashoffset = (RING_CIRCUMFERENCE * (1 - fraction)).toFixed(2);
        ring.classList.toggle('urgent', ms > 0 && ms <= 10000);
    }
}

function setupLiveDataInterval(freq) {
    currentFrequency = Number(freq) > 0 ? Number(freq) : 15;
    liveFetchGeneration++;
    nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
    updateCountdown();
    if (countdownIntervalId) clearInterval(countdownIntervalId);
    countdownIntervalId = setInterval(updateCountdown, 1000);
    if (scheduledNextFetch) clearTimeout(scheduledNextFetch);
    fetchLiveDataAndRender();
}

function setConnectionStatus(status) {
    const pill = document.getElementById('connection-status');
    const label = document.getElementById('connection-label');
    if (!pill || !label) return;
    if (pill.dataset.status === status) return;
    pill.dataset.status = status;
    label.textContent = status === 'online' ? 'Live' : status === 'offline' ? 'Off' : 'Conn';
}

function initTickerToggle() {
    const btn = document.getElementById('ticker-toggle');
    const wrap = document.getElementById('live-ticker-wrap');
    if (!btn || !wrap) return;
    const collapsed = localStorage.getItem('hex-ticker-collapsed') === '1';
    wrap.classList.toggle('collapsed', collapsed);
    btn.setAttribute('aria-expanded', String(!collapsed));
    btn.addEventListener('click', () => {
        const nowCollapsed = wrap.classList.toggle('collapsed');
        btn.setAttribute('aria-expanded', String(!nowCollapsed));
        localStorage.setItem('hex-ticker-collapsed', nowCollapsed ? '1' : '0');
    });
}

// =============================================
// WALLET ADDRESS
// =============================================

function saveWalletSettings() {
    const walletAddrs = document.getElementById('wallet-addresses').value || '';
    const addrs = walletAddrs.split(',').map(s => s.trim()).filter(s => s.length > 0);
    for (let addr of addrs) {
        if (!addr.startsWith('0x') || addr.length < 10) {
            showNotification('Invalid address format: ' + addr, 'danger');
            return;
        }
    }
    debouncedSaveConfig();
}

function updateWalletAddressesUI(addressesStr) {
    const container = document.getElementById('saved-wallet-addresses');
    if (!container) return;
    container.innerHTML = '';
    if (!addressesStr) return;
    const addresses = addressesStr.split(',').map(s => s.trim()).filter(s => s.length > 0);
    addresses.forEach(addr => {
        if (!addr.startsWith('0x') || addr.length < 10) return;
        const masked = addr.substring(0, 4) + '....' + addr.substring(addr.length - 4);
        const item = document.createElement('div');
        item.className = 'wallet-address-item';
        const span = document.createElement('span');
        span.className = 'wallet-address-masked';
        span.textContent = masked;
        const copyBtn = document.createElement('button');
        copyBtn.className = 'copy-address-btn';
        copyBtn.innerHTML = 'Copy';
        copyBtn.title = 'Copy full address';
        copyBtn.setAttribute('aria-label', `Copy address ${masked}`);
        copyBtn.addEventListener('click', () => {
            navigator.clipboard.writeText(addr).then(() => {
                showNotification('Address copied!', 'success');
                const originalText = copyBtn.innerHTML;
                copyBtn.innerHTML = 'Copied!';
                copyBtn.classList.add('copied');
                setTimeout(() => {
                    copyBtn.innerHTML = originalText;
                    copyBtn.classList.remove('copied');
                }, 2000);
            }).catch(() => showNotification('Failed to copy', 'danger'));
        });
        item.appendChild(span);
        item.appendChild(copyBtn);
        container.appendChild(item);
    });
}

// =============================================
// MINERS
// =============================================

function showCompletedMiners() {
    fetch('/api/miners').then(r => r.json()).then(miners => {
        const list = document.getElementById('completed-miners-list');
        if (!list) return;
        list.innerHTML = '';
        const completed = miners.filter(m => m.status === 'completed');
        if (!completed.length) {
            const li = document.createElement('li');
            li.style.color = 'var(--text-muted)';
            li.textContent = 'No completed miners.';
            list.appendChild(li);
        } else {
            completed.forEach(m => {
                const li = document.createElement('li');
                const tShares = Number(m.tShares) || 0;
                li.textContent = `${m.startDate} - ${m.endDate} • T-Shares: ${tShares.toFixed(2)}`;
                list.appendChild(li);
            });
        }
        const modal = document.getElementById('completed-miners-modal');
        if (modal) modal.showModal();
    }).catch(() => showNotification('Failed to load miners', 'danger'));
}

async function endMiner(id) {
    const minerId = Number(id);
    if (!Number.isFinite(minerId)) { showNotification('Invalid miner ID', 'danger'); return; }
    const confirmed = await showConfirmModal('End Miner', 'Have you ended the mining contract and minted HEX?');
    if (!confirmed) return;
    try {
        const res = await fetch('/api/end-miner', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: minerId }) });
        showNotification(res.ok ? 'Miner ended successfully' : 'Error ending miner', res.ok ? 'success' : 'danger');
        if (res.ok) initialLoad();
    } catch { showNotification('Network error', 'danger'); }
}

async function deleteMiner(id) {
    const minerId = Number(id);
    if (!Number.isFinite(minerId)) { showNotification('Invalid miner ID', 'danger'); return; }
    const confirmed = await showConfirmModal('Delete Miner', 'Delete this HEX miner?');
    if (!confirmed) return;
    try {
        const res = await fetch('/api/delete-miner', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: minerId }) });
        showNotification(res.ok ? 'Miner deleted' : 'Error deleting miner', res.ok ? 'success' : 'danger');
        if (res.ok) initialLoad();
    } catch { showNotification('Network error', 'danger'); }
}

window.endMiner = endMiner;
window.deleteMiner = deleteMiner;

// =============================================
// INITIAL LOAD
// =============================================

async function initialLoad() {
    try {
        const [minersRes, cfgRes, hexData] = await Promise.all([
            fetch('/api/miners'), fetch('/api/config'), fetchHexJsonWithCache()
        ]);
        const miners = await minersRes.json();
        const cfg = await cfgRes.json();
        window.HEXJSON_CACHE = Array.isArray(hexData) ? hexData : [];
        if (window.HEXJSON_CACHE.length === 0) console.warn('⚠️ No hexjson data yet.');

        let totalTS = 0;
        const activeMiners = [];
        miners.forEach(m => {
            if (m.status !== 'completed') { totalTS += Number(m.tShares) || 0; activeMiners.push(m); }
        });
        activeMiners.sort((a, b) => {
            const endA = parseDateDDMMYYYY(a.endDate); const endB = parseDateDDMMYYYY(b.endDate);
            const startA = parseDateDDMMYYYY(a.startDate); const startB = parseDateDDMMYYYY(b.startDate);
            if (endA && endB && endA.getTime() !== endB.getTime()) return endA.getTime() - endB.getTime();
            if (startA && startB) return startA.getTime() - startB.getTime();
            return 0;
        });

        userTotalTShares = totalTS;
        userLiquidHEX = Number(cfg.liquidHEX) || 0;
        document.getElementById('total-tshares').textContent = totalTS.toFixed(2);
        updateProfileStats();

        const activeDiv = document.getElementById('active-miners');
        activeDiv.innerHTML = '';
        document.getElementById('profile-message').textContent = activeMiners.length ? '' : 'Empty profile. Please add HEX miners in Settings.';

        activeMiners.forEach(m => {
            const startDate = parseDateDDMMYYYY(m.startDate);
            const endDate = parseDateDDMMYYYY(m.endDate);
            if (!startDate || !endDate) return;
            const startUTC = Date.UTC(startDate.getFullYear(), startDate.getMonth(), startDate.getDate());
            const endUTC = Date.UTC(endDate.getFullYear(), endDate.getMonth(), endDate.getDate());
            const now = Date.now();
            const matured = endUTC <= now;
            const totalDays = Math.ceil((endUTC - startUTC) / (1000 * 60 * 60 * 24));
            const daysElapsed = Math.max(0, Math.ceil((now - startUTC) / (1000 * 60 * 60 * 24)));
            const daysLeft = matured ? 0 : Math.ceil((endUTC - now) / (1000 * 60 * 60 * 24));
            const percentage = totalDays > 0 ? Math.min(100, (daysElapsed / totalDays) * 100) : 0;

            const card = document.createElement('div'); card.className = 'miner-card';
            const header = document.createElement('div'); header.className = 'miner-card-header';
            const dates = document.createElement('div'); dates.className = 'miner-dates';
            
            const startBlock = document.createElement('div');
            const startLabel = document.createElement('span'); startLabel.className = 'miner-date-label'; startLabel.textContent = 'Start Date';
            const startValue = document.createElement('div'); startValue.className = 'miner-date-value'; startValue.textContent = m.startDate;
            startBlock.appendChild(startLabel); startBlock.appendChild(startValue);
            
            const endBlock = document.createElement('div');
            const endLabel = document.createElement('span'); endLabel.className = 'miner-date-label'; endLabel.textContent = 'End Date';
            const endValue = document.createElement('div'); endValue.className = 'miner-date-value'; endValue.textContent = m.endDate;
            endBlock.appendChild(endLabel); endBlock.appendChild(endValue);
            
            dates.appendChild(startBlock); dates.appendChild(endBlock);
            
            const tshares = document.createElement('div'); tshares.className = 'miner-tshares';
            const tsharesLabel = document.createElement('div'); tsharesLabel.className = 'miner-tshares-label'; tsharesLabel.textContent = 'T-Shares';
            const tsharesValue = document.createElement('div'); tsharesValue.className = 'miner-tshares-value'; tsharesValue.textContent = (Number(m.tShares) || 0).toFixed(2);
            tshares.appendChild(tsharesLabel); tshares.appendChild(tsharesValue);
            
            header.appendChild(dates); header.appendChild(tshares);
            
            const progressContainer = document.createElement('div'); progressContainer.className = 'miner-progress-container';
            const progressInfo = document.createElement('div'); progressInfo.className = 'miner-progress-info';
            const daysLeftSpan = document.createElement('span'); daysLeftSpan.className = 'miner-days-left';
            
            if (matured) {
                const strong = document.createElement('strong'); strong.textContent = 'Matured'; daysLeftSpan.appendChild(strong);
            } else {
                const strong = document.createElement('strong'); strong.textContent = String(daysLeft);
                daysLeftSpan.appendChild(strong); daysLeftSpan.appendChild(document.createTextNode(' days remaining'));
            }
            
            const percentageSpan = document.createElement('span'); percentageSpan.className = 'miner-percentage'; percentageSpan.textContent = `${percentage.toFixed(1)}%`;
            progressInfo.appendChild(daysLeftSpan); progressInfo.appendChild(percentageSpan);
            
            const progressBar = document.createElement('div'); progressBar.className = 'miner-progress-bar';
            const progressFill = document.createElement('div'); 
            progressFill.className = `miner-progress-fill ${matured ? 'matured' : ''}`; 
            progressFill.style.width = `${percentage}%`;
            progressFill.setAttribute('role', 'progressbar');
            progressFill.setAttribute('aria-valuenow', percentage.toFixed(0));
            progressFill.setAttribute('aria-valuemin', '0');
            progressFill.setAttribute('aria-valuemax', '100');
            progressFill.setAttribute('aria-label', `Mining progress: ${percentage.toFixed(1)}% complete`);
            progressBar.appendChild(progressFill);
            progressContainer.appendChild(progressInfo); progressContainer.appendChild(progressBar);
            
            const footer = document.createElement('div'); footer.className = 'miner-card-footer';
            if (matured) {
                const endBtn = document.createElement('button'); endBtn.className = 'btn btn-sm btn-danger'; endBtn.textContent = 'End Miner';
                endBtn.setAttribute('aria-label', `End miner starting on ${m.startDate}`);
                endBtn.addEventListener('click', () => endMiner(m.id));
                footer.appendChild(endBtn);
            } else {
                const badge = document.createElement('span'); badge.className = 'miner-status-badge active'; badge.textContent = 'Active';
                footer.appendChild(badge);
            }
            
            card.appendChild(header); card.appendChild(progressContainer); card.appendChild(footer);
            activeDiv.appendChild(card);
        });

        document.getElementById('frequency').value = cfg.liveDataFrequency;
        document.getElementById('liquid-hex').value = cfg.liquidHEX || '';
        document.getElementById('hist-start-day').value = cfg.historicalStartDay || 1260;
        
        const walletAddrEl = document.getElementById('wallet-addresses');
        if (walletAddrEl) walletAddrEl.value = cfg.walletAddresses || '';
        updateWalletAddressesUI(cfg.walletAddresses || '');

        historicalStartDay = Number(cfg.historicalStartDay) || 1260;

        const existingDiv = document.getElementById('existing-miners');
        existingDiv.innerHTML = '';
        const sortedMiners = [...miners].sort((a, b) => {
            const endA = parseDateDDMMYYYY(a.endDate); const endB = parseDateDDMMYYYY(b.endDate);
            const startA = parseDateDDMMYYYY(a.startDate); const startB = parseDateDDMMYYYY(b.startDate);
            if (endA && endB && endA.getTime() !== endB.getTime()) return endA.getTime() - endB.getTime();
            if (startA && startB) return startA.getTime() - startB.getTime();
            return 0;
        });
        
        sortedMiners.forEach(m => {
            const div = document.createElement('div'); div.className = 'list-item';
            const span = document.createElement('span');
            span.textContent = `${m.startDate} - ${m.endDate} • T-Shares: ${(Number(m.tShares) || 0).toFixed(2)}`;
            const deleteBtn = document.createElement('button'); deleteBtn.className = 'btn btn-sm btn-danger'; deleteBtn.textContent = 'Delete';
            deleteBtn.setAttribute('aria-label', `Delete miner starting on ${m.startDate}`);
            deleteBtn.addEventListener('click', () => deleteMiner(m.id));
            div.appendChild(span); div.appendChild(deleteBtn);
            existingDiv.appendChild(div);
        });

        setupLiveDataInterval(cfg.liveDataFrequency);
        if (window.HEXJSON_CACHE.length > 0) requestAnimationFrame(() => renderPortfolioHistoryChartWithData(window.HEXJSON_CACHE));
        if (document.querySelector('.tab-btn.active')) renderAllCharts();
        if (typeof AirDatepicker !== 'undefined') initAirDatepickers();
    } catch (err) {
        console.error('Initial load error:', err);
        showNotification('Failed to load data.', 'danger');
    }
}

// =============================================
// ACTIONS & CONFIG
// =============================================

document.getElementById('add-miner-btn').addEventListener('click', () => {
    const sd = document.getElementById('start-date').value;
    const ed = document.getElementById('end-date').value;
    const ts = parseFloat(document.getElementById('tshares').value);
    if (!sd || !ed || isNaN(ts) || ts <= 0) return showNotification('Fill all fields with valid data', 'danger');
    const startDate = parseDateDDMMYYYY(sd);
    const endDate = parseDateDDMMYYYY(ed);
    if (!startDate || !endDate) return showNotification('Dates must be DD-MM-YYYY format', 'danger');
    if (endDate < startDate) return showNotification('End date must be after start date', 'danger');
    
    const btn = document.getElementById('add-miner-btn');
    btn.disabled = true;
    fetch('/api/add-miner', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ startDate: sd, endDate: ed, tShares: ts })
    }).then(r => {
        if (r.ok) {
            showNotification('Miner added', 'success');
            document.getElementById('start-date').value = '';
            document.getElementById('end-date').value = '';
            document.getElementById('tshares').value = '';
            initialLoad();
        } else { showNotification('Error adding miner', 'danger'); }
    }).catch(() => showNotification('Network error', 'danger'))
    .finally(() => btn.disabled = false);
});

function debouncedSaveConfig() {
    if (saveInProgress) return;
    saveInProgress = true;
    
    const freq = parseInt(document.getElementById('frequency').value) || 15;
    const liquid = parseFloat(document.getElementById('liquid-hex').value) || 0;
    const hist = parseInt(document.getElementById('hist-start-day').value) || 1260;
    const walletAddrs = document.getElementById('wallet-addresses')?.value || '';

    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            liveDataFrequency: freq,
            liquidHEX: liquid,
            historicalStartDay: hist,
            walletAddresses: walletAddrs
        })
    })
    .then(r => {
        showNotification(r.ok ? 'Settings saved' : 'Error saving', r.ok ? 'success' : 'danger');
        if (r.ok) initialLoad();
    })
    .catch(() => showNotification('Network error', 'danger'))
    .finally(() => saveInProgress = false);
}

function saveFrequency() {
    if (parseInt(document.getElementById('frequency').value) <= 0) return showNotification('Frequency must be > 0', 'danger');
    debouncedSaveConfig();
}

function saveLiquidHEX() {
    if (parseFloat(document.getElementById('liquid-hex').value) < 0) return showNotification('Liquid HEX must be >= 0', 'danger');
    debouncedSaveConfig();
}

function saveHistoricalStartDay() {
    if (parseInt(document.getElementById('hist-start-day').value) < 1) return showNotification('Start day must be > 0', 'danger');
    debouncedSaveConfig();
}

// =============================================
// INIT
// =============================================

document.addEventListener('DOMContentLoaded', () => {
    initTickerToggle();
    if (typeof AirDatepicker !== 'undefined') {
        initAirDatepickers();
    } else {
        const checkLoaded = setInterval(() => {
            if (typeof AirDatepicker !== 'undefined') { clearInterval(checkLoaded); initAirDatepickers(); }
        }, 100);
        setTimeout(() => clearInterval(checkLoaded), 5000);
    }

    const tabButtons = document.querySelectorAll('.tab-btn');
    const tabPanels = document.querySelectorAll('.tab-content');

    function activateTab(btn) {
        tabButtons.forEach(b => {
            b.classList.remove('active');
            b.setAttribute('aria-selected', 'false');
            b.setAttribute('tabindex', '-1');
        });
        tabPanels.forEach(c => c.classList.remove('active'));
        
        btn.classList.add('active');
        btn.setAttribute('aria-selected', 'true');
        btn.setAttribute('tabindex', '0');
        
        const targetTab = document.getElementById(btn.dataset.tab);
        targetTab.classList.add('active');
        
        if (btn.dataset.tab === 'charts') renderAllCharts();
        else if (btn.dataset.tab === 'profile') {
            if (window.HEXJSON_CACHE && window.HEXJSON_CACHE.length > 0) renderPortfolioHistoryChartWithData(window.HEXJSON_CACHE);
        }
    }

    tabButtons.forEach((btn, index) => {
        btn.addEventListener('click', () => activateTab(btn));
        btn.addEventListener('keydown', (e) => {
            let newIndex = index;
            if (e.key === 'ArrowRight') newIndex = (index + 1) % tabButtons.length;
            else if (e.key === 'ArrowLeft') newIndex = (index - 1 + tabButtons.length) % tabButtons.length;
            else if (e.key === 'Home') newIndex = 0;
            else if (e.key === 'End') newIndex = tabButtons.length - 1;
            else return;
            
            e.preventDefault();
            tabButtons[newIndex].focus();
            activateTab(tabButtons[newIndex]);
        });
    });

    document.getElementById('show-completed-btn')?.addEventListener('click', showCompletedMiners);
    document.querySelectorAll('.modal-close-btn, .btn-close').forEach(btn => {
        btn.addEventListener('click', () => { const d = btn.closest('dialog'); if (d) d.close(); });
    });

    initialLoad();
});
