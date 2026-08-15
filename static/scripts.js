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

window.HEXJSON_CACHE = [];
window.MINERS_CACHE = [];

let datepickers = {
    startDate: null,
    endDate: null
};

const RING_CIRCUMFERENCE = 2 * Math.PI * 16;

// =============================================
// SAFE STORAGE / HTML HELPERS
// =============================================

function safeGetLocalStorage(key) {
    try {
        return localStorage.getItem(key);
    } catch (e) {
        return null;
    }
}

function safeSetLocalStorage(key, value) {
    try {
        localStorage.setItem(key, value);
    } catch (e) {
        console.warn('localStorage write failed:', e);
    }
}

function escapeHtml(value) {
    return String(value).replace(/[&<>"']/g, c => ({
        '&': '&amp;',
        '<': '&lt;',
        '>': '&gt;',
        '"': '&quot;',
        "'": '&#39;'
    }[c]));
}

function formatWithCommas(num) {
    const n = Number(num);
    if (!Number.isFinite(n)) return '0';
    return n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

// =============================================
// AIR-DATEPICKER
// =============================================

function initAirDatepickers() {
    const localeEn = {
        days: ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'],
        daysShort: ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'],
        daysMin: ['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa'],
        months: ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'],
        monthsShort: ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'],
        today: 'Today',
        clear: 'Clear',
        dateFormat: 'dd-MM-yyyy',
        firstDay: 1
    };

    const commonOpts = {
        locale: localeEn,
        dateFormat: 'dd-MM-yyyy',
        autoClose: true,
        classes: 'air-datepicker-dark',
        onShow: (isFinished, dp) => {
            if (isFinished && dp?.$datepicker) {
                dp.$datepicker.style.setProperty('z-index', '1050', 'important');
            }
        },
        onSelect: ({ datepicker }) => {
            datepicker.$el.dispatchEvent(new Event('change', { bubbles: true }));
        }
    };

    const startDateEl = document.getElementById('start-date');

    if (startDateEl && typeof AirDatepicker !== 'undefined') {
        if (datepickers.startDate) datepickers.startDate.destroy();

        datepickers.startDate = new AirDatepicker(startDateEl, {
            ...commonOpts,
            minDate: new Date(2021, 0, 1),
            maxDate: new Date(),
            onSelect: ({ datepicker, date }) => {
                commonOpts.onSelect?.({ datepicker, formattedDate: datepicker.$el.value });

                if (datepickers.endDate && date instanceof Date && !isNaN(date)) {
                    datepickers.endDate.update({ minDate: date });
                }
            }
        });
    }

    const endDateEl = document.getElementById('end-date');

    if (endDateEl && typeof AirDatepicker !== 'undefined') {
        if (datepickers.endDate) datepickers.endDate.destroy();

        let initialMinDate = undefined;

        if (startDateEl?.value) {
            const parsed = parseDateDDMMYYYY(startDateEl.value);
            if (parsed instanceof Date && !isNaN(parsed)) {
                initialMinDate = parsed;
            }
        }

        const endDateOpts = { ...commonOpts };

        if (initialMinDate) {
            endDateOpts.minDate = initialMinDate;
        }

        datepickers.endDate = new AirDatepicker(endDateEl, endDateOpts);
    }
}

function parseDateDDMMYYYY(str) {
    if (!str || typeof str !== 'string') return undefined;

    const trimmed = str.trim();

    if (!/^\d{2}-\d{2}-\d{4}$/.test(trimmed)) return undefined;

    const [d, m, y] = trimmed.split('-').map(Number);

    if (d < 1 || d > 31 || m < 1 || m > 12 || y < 2000 || y > 2100) return undefined;

    const date = new Date(y, m - 1, d);

    if (isNaN(date.getTime()) || date.getDate() !== d || date.getMonth() !== m - 1) {
        return undefined;
    }

    return date;
}

function formatDateDDMMYYYY(date) {
    if (!(date instanceof Date) || isNaN(date)) return '';

    const d = String(date.getDate()).padStart(2, '0');
    const m = String(date.getMonth() + 1).padStart(2, '0');
    const y = date.getFullYear();

    return `${d}-${m}-${y}`;
}

// =============================================
// UI HELPERS
// =============================================

function showNotification(message, type = 'success') {
    const container = document.getElementById('notification-container');
    if (!container) return;

    const toast = document.createElement('div');
    toast.className = `toast ${type}`;
    toast.textContent = message;

    container.appendChild(toast);

    setTimeout(() => {
        toast.style.opacity = '0';
        toast.style.transform = 'translateX(100%)';
        setTimeout(() => toast.remove(), 300);
    }, 3000);
}

function showConfirmModal(title, message) {
    return new Promise(resolve => {
        const dialog = document.createElement('dialog');
        dialog.className = 'modal';

        dialog.innerHTML = `
            <header class="modal-header">
                <h3>${escapeHtml(title)}</h3>
                <button class="btn-close" aria-label="Close">×</button>
            </header>
            <p class="modal-body">${escapeHtml(message)}</p>
            <footer class="modal-footer">
                <button class="btn btn-secondary confirm-cancel">Cancel</button>
                <button class="btn btn-danger confirm-action">Confirm</button>
            </footer>
        `;

        document.body.appendChild(dialog);
        dialog.showModal();

        const cleanup = result => {
            dialog.close();
            dialog.remove();
            resolve(result);
        };

        dialog.querySelector('.btn-close').onclick = () => cleanup(false);
        dialog.querySelector('.confirm-cancel').onclick = () => cleanup(false);
        dialog.querySelector('.confirm-action').onclick = () => cleanup(true);
        dialog.addEventListener('cancel', () => cleanup(false));
    });
}

function showCompletedMiners() {
    fetchJson('/api/miners')
        .then(miners => {
            const list = document.getElementById('completed-miners-list');
            if (!list) return;

            list.innerHTML = '';

            const completed = miners.filter(m => m.status === 'completed');

            if (!completed.length) {
                list.innerHTML = '<li style="color:var(--text-muted)">No completed miners.</li>';
            } else {
                completed.forEach(m => {
                    const li = document.createElement('li');
                    li.textContent = `${m.startDate} - ${m.endDate} • T-Shares: ${(Number(m.tShares) || 0).toFixed(2)}`;
                    list.appendChild(li);
                });
            }

            const modal = document.getElementById('completed-miners-modal');
            if (modal) modal.showModal();
        })
        .catch(() => showNotification('Failed to load miners', 'danger'));
}

// =============================================
// HEX DAY / DATE HELPERS
// =============================================

function getUtcMidnightMs(date) {
    return Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate());
}

function getLatestHexDay() {
    if (!Array.isArray(window.HEXJSON_CACHE) || window.HEXJSON_CACHE.length === 0) {
        return 0;
    }

    return window.HEXJSON_CACHE.reduce((max, entry) => {
        const day = Number(entry.currentDay) || 0;
        return Math.max(max, day);
    }, 0);
}

function hexDayToDate(day) {
    const latestDay = getLatestHexDay();

    if (!latestDay || !day) return null;

    const now = new Date();
    const todayUtc = getUtcMidnightMs(now);

    const diffDays = latestDay - day;

    return new Date(todayUtc - diffDays * 86400000);
}

function dateToHexDay(dateStr) {
    const d = parseDateDDMMYYYY(dateStr);
    if (!d) return null;

    const latestDay = getLatestHexDay();
    if (!latestDay) return null;

    const now = new Date();
    const todayUtc = getUtcMidnightMs(now);
    const dateUtc = Date.UTC(d.getFullYear(), d.getMonth(), d.getDate());

    return latestDay + Math.round((dateUtc - todayUtc) / 86400000);
}

function formatHexDayLabel(day) {
    const date = hexDayToDate(day);

    if (!date) return String(day);

    return date.toLocaleDateString(undefined, {
        year: '2-digit',
        month: 'short',
        day: 'numeric'
    });
}

function getMinerDayRanges() {
    const miners = Array.isArray(window.MINERS_CACHE) ? window.MINERS_CACHE : [];

    return miners
        .map(m => {
            const start = dateToHexDay(m.startDate);
            const end = dateToHexDay(m.endDate);
            const tShares = Number(m.tShares);

            if (start == null || end == null || end < start || !Number.isFinite(tShares)) {
                return null;
            }

            return { start, end, tShares };
        })
        .filter(Boolean);
}

function activeTSharesOnDay(day, ranges) {
    return ranges.reduce((sum, r) => {
        return (day >= r.start && day <= r.end) ? sum + r.tShares : sum;
    }, 0);
}

// =============================================
// TAB & CHART MANAGEMENT
// =============================================

function renderAllCharts() {
    if (!window.Chart || !window.HEXJSON_CACHE || window.HEXJSON_CACHE.length === 0) return;

    renderChartsWithData(window.HEXJSON_CACHE);
    renderPortfolioHistoryChartWithData(window.HEXJSON_CACHE);

    requestAnimationFrame(() => {
        Object.values(chartInstances).forEach(c => {
            if (c && typeof c.resize === 'function') {
                c.resize();
            }
        });
    });
}

function bindStaticEvents() {
    document.querySelectorAll('.tab-btn').forEach(btn => {
        btn.addEventListener('click', () => {
            document.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
            document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));

            btn.classList.add('active');

            const targetTab = document.getElementById(btn.dataset.tab);
            if (targetTab) targetTab.classList.add('active');

            if (btn.dataset.tab === 'charts') {
                renderAllCharts();
            } else if (btn.dataset.tab === 'profile') {
                if (window.HEXJSON_CACHE && window.HEXJSON_CACHE.length > 0) {
                    renderPortfolioHistoryChartWithData(window.HEXJSON_CACHE);
                }
            }
        });
    });

    const showCompletedBtn = document.getElementById('show-completed-btn');
    if (showCompletedBtn) {
        showCompletedBtn.addEventListener('click', showCompletedMiners);
    }

    document.querySelectorAll('.modal-close-btn, .btn-close').forEach(btn => {
        btn.addEventListener('click', () => {
            const d = btn.closest('dialog');
            if (d) d.close();
        });
    });

    const addMinerBtn = document.getElementById('add-miner-btn');

    if (addMinerBtn) {
        addMinerBtn.addEventListener('click', () => {
            const sd = document.getElementById('start-date').value;
            const ed = document.getElementById('end-date').value;
            const ts = parseFloat(document.getElementById('tshares').value);

            if (!sd || !ed || isNaN(ts) || ts <= 0) {
                return showNotification('Fill all fields with valid data', 'danger');
            }

            if (!parseDateDDMMYYYY(sd) || !parseDateDDMMYYYY(ed)) {
                return showNotification('Dates must be DD-MM-YYYY format', 'danger');
            }

            const startDate = parseDateDDMMYYYY(sd);
            const endDate = parseDateDDMMYYYY(ed);

            if (endDate < startDate) {
                return showNotification('End date must be after start date', 'danger');
            }

            const btn = document.getElementById('add-miner-btn');
            btn.disabled = true;

            fetch('/api/add-miner', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    startDate: sd,
                    endDate: ed,
                    tShares: ts
                })
            })
                .then(r => {
                    if (r.ok) {
                        showNotification('Miner added', 'success');
                        document.getElementById('start-date').value = '';
                        document.getElementById('end-date').value = '';
                        document.getElementById('tshares').value = '';
                        initialLoad();
                    } else {
                        showNotification('Error adding miner', 'danger');
                    }
                })
                .catch(() => showNotification('Network error', 'danger'))
                .finally(() => {
                    btn.disabled = false;
                });
        });
    }
}

// =============================================
// FETCH HELPERS
// =============================================

async function fetchJson(url) {
    const res = await fetch(url);

    if (!res.ok) {
        throw new Error(`HTTP ${res.status}`);
    }

    return res.json();
}

async function fetchHexJson() {
    const cachedEtag = safeGetLocalStorage('hexjson-etag');
    const headers = {};

    if (cachedEtag) {
        headers['If-None-Match'] = cachedEtag;
    }

    try {
        let res = await fetch('/api/hexjson', {
            headers,
            cache: 'no-cache'
        });

        if (res.status === 304) {
            const cached = safeGetLocalStorage('hexjson-cache');

            if (cached) {
                try {
                    const parsed = JSON.parse(cached);
                    if (Array.isArray(parsed)) {
                        return parsed;
                    }
                } catch (e) {
                    console.warn('Cached hexjson parse failed, forcing refetch.');
                }
            }

            res = await fetch('/api/hexjson');
        }

        if (!res.ok) {
            throw new Error(`HTTP ${res.status}`);
        }

        const data = await res.json();
        const arr = Array.isArray(data) ? data : [];

        const etag = res.headers.get('ETag');
        if (etag) {
            safeSetLocalStorage('hexjson-etag', etag);
        }

        safeSetLocalStorage('hexjson-cache', JSON.stringify(arr));

        return arr;
    } catch (e) {
        const cached = safeGetLocalStorage('hexjson-cache');

        if (cached) {
            try {
                const parsed = JSON.parse(cached);
                if (Array.isArray(parsed)) {
                    console.warn('Using cached hexjson due to fetch failure.');
                    return parsed;
                }
            } catch (parseErr) {
                console.warn('Cached hexjson parse failed:', parseErr);
            }
        }

        throw e;
    }
}

// =============================================
// PROFILE & LIVE DATA
// =============================================

function updateProfileStats() {
    if (!liveDataCache) return;

    const d = liveDataCache;

    const totalValue = userTotalTShares * (Number(d.tsharePrice_Pulsechain) || 0);
    const liquidValue = userLiquidHEX * (Number(d.price_Pulsechain) || 0);
    const interestHex = userTotalTShares * (Number(d.payoutPerTshare_Pulsechain) || 0);
    const interestUsd = interestHex * (Number(d.price_Pulsechain) || 0);

    const totalEl = document.getElementById('total-value');
    const liquidEl = document.getElementById('liquid-hex-value');
    const interestHexEl = document.getElementById('interest-hex');
    const interestUsdEl = document.getElementById('interest-usd');

    if (totalEl) totalEl.textContent = `$${formatWithCommas(totalValue.toFixed(2))}`;
    if (liquidEl) liquidEl.textContent = `$${formatWithCommas(liquidValue.toFixed(2))}`;
    if (interestHexEl) interestHexEl.textContent = `${formatWithCommas(interestHex.toFixed(2))} HEX`;
    if (interestUsdEl) interestUsdEl.textContent = `$${formatWithCommas(interestUsd.toFixed(2))}`;
}

async function fetchLiveDataAndRender() {
    const generation = ++liveFetchGeneration;

    if (!hasConnectedOnce) {
        setConnectionStatus('connecting');
    }

    try {
        const res = await fetch('/api/live-data');

        if (!res.ok) {
            throw new Error(`HTTP ${res.status}`);
        }

        const data = await res.json();

        if (!data || !(Number(data.price_Pulsechain) > 0)) {
            throw new Error('Live data price missing or zero');
        }

        updateLiveDataUI(data);
        hasConnectedOnce = true;
        setConnectionStatus('online');
    } catch (e) {
        console.error('Live data fetch failed', e);

        try {
            const publicRes = await fetch('https://hexdailystats.com/livedata');

            if (publicRes.ok) {
                const publicData = await publicRes.json();

                if (publicData && Number(publicData.price_Pulsechain) > 0) {
                    updateLiveDataUI(publicData);
                    hasConnectedOnce = true;
                    setConnectionStatus('online');
                } else {
                    setConnectionStatus('offline');
                }
            } else {
                setConnectionStatus('offline');
            }
        } catch (fbErr) {
            console.error('Public fallback also failed', fbErr);
            setConnectionStatus('offline');
        }
    } finally {
        if (generation === liveFetchGeneration) {
            nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
            updateCountdown();

            scheduledNextFetch = setTimeout(
                fetchLiveDataAndRender,
                currentFrequency * 60 * 1000
            );
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

    document.title = `HEX Stats - $${price.toFixed(5)}`;

    nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
    updateCountdown();
    updateProfileStats();
}

function updateCountdown() {
    const totalMs = currentFrequency * 60 * 1000;
    const ms = Math.max(0, nextRefreshTime - Date.now());
    const s = Math.floor(ms / 1000);

    const el = document.getElementById('countdown');
    if (el) {
        el.textContent = `${Math.floor(s / 60)}:${(s % 60).toString().padStart(2, '0')}`;
    }

    const ring = document.getElementById('ring-progress');

    if (ring) {
        const fraction = totalMs > 0 ? ms / totalMs : 0;
        ring.style.strokeDashoffset = (RING_CIRCUMFERENCE * (1 - fraction)).toFixed(2);
        ring.classList.toggle('urgent', ms > 0 && ms <= 10000);
    }
}

function setupLiveDataInterval(freq) {
    currentFrequency = freq || 15;

    // Invalidate any in-flight scheduling.
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

    const collapsed = safeGetLocalStorage('hex-ticker-collapsed') === '1';

    wrap.classList.toggle('collapsed', collapsed);
    btn.setAttribute('aria-expanded', String(!collapsed));

    btn.addEventListener('click', () => {
        const nowCollapsed = wrap.classList.toggle('collapsed');
        btn.setAttribute('aria-expanded', String(!nowCollapsed));
        safeSetLocalStorage('hex-ticker-collapsed', nowCollapsed ? '1' : '0');
    });
}

// =============================================
// INITIAL LOAD
// =============================================

async function initialLoad() {
    try {
        const [miners, cfg, hexData] = await Promise.all([
            fetchJson('/api/miners'),
            fetchJson('/api/config'),
            fetchHexJson()
        ]);

        window.HEXJSON_CACHE = Array.isArray(hexData) ? hexData : [];
        window.MINERS_CACHE = Array.isArray(miners) ? miners : [];

        if (window.HEXJSON_CACHE.length === 0) {
            console.warn('⚠️ No hexjson data yet.');
        }

        let totalTS = 0;
        let activeMiners = [];

        window.MINERS_CACHE.forEach(m => {
            if (m.status !== 'completed') {
                totalTS += Number(m.tShares) || 0;
                activeMiners.push(m);
            }
        });

        userTotalTShares = totalTS;
        userLiquidHEX = Number(cfg.liquidHEX) || 0;
        historicalStartDay = Number(cfg.historicalStartDay) || 1260;

        const totalTSharesEl = document.getElementById('total-tshares');
        if (totalTSharesEl) totalTSharesEl.textContent = totalTS.toFixed(2);

        updateProfileStats();

        const activeDiv = document.getElementById('active-miners');
        if (activeDiv) {
            activeDiv.innerHTML = '';

            const profileMessage = document.getElementById('profile-message');
            if (profileMessage) {
                profileMessage.textContent = activeMiners.length
                    ? ''
                    : 'Empty profile. Please add HEX miners in Settings.';
            }

            activeMiners.forEach(m => {
                const [startD, startMo, startY] = m.startDate.split('-');
                const [endD, endMo, endY] = m.endDate.split('-');

                const startUTC = Date.UTC(startY, startMo - 1, startD);
                const endUTC = Date.UTC(endY, endMo - 1, endD);

                const now = Date.now();
                const matured = endUTC <= now;

                const totalDays = Math.ceil((endUTC - startUTC) / (1000 * 60 * 60 * 24));
                const daysElapsed = Math.max(0, Math.ceil((now - startUTC) / (1000 * 60 * 60 * 24)));
                const daysLeft = matured ? 0 : Math.ceil((endUTC - now) / (1000 * 60 * 60 * 24));

                const percentage = totalDays > 0
                    ? Math.min(100, (daysElapsed / totalDays) * 100)
                    : 0;

                const tSharesText = (Number(m.tShares) || 0).toFixed(2);
                const minerId = Number(m.id) || 0;

                const div = document.createElement('div');
                div.className = 'miner-card';

                div.innerHTML = `
                    <div class="miner-card-header">
                        <div class="miner-dates">
                            <div>
                                <span class="miner-date-label">Start Date</span>
                                <div class="miner-date-value">${escapeHtml(m.startDate)}</div>
                            </div>
                            <div>
                                <span class="miner-date-label">End Date</span>
                                <div class="miner-date-value">${escapeHtml(m.endDate)}</div>
                            </div>
                        </div>
                        <div class="miner-tshares">
                            <div class="miner-tshares-label">T-Shares</div>
                            <div class="miner-tshares-value">${tSharesText}</div>
                        </div>
                    </div>

                    <div class="miner-progress-container">
                        <div class="miner-progress-info">
                            <span class="miner-days-left">
                                ${matured ? '<strong>Matured</strong>' : `<strong>${daysLeft}</strong> days remaining`}
                            </span>
                            <span class="miner-percentage">${percentage.toFixed(1)}%</span>
                        </div>
                        <div class="miner-progress-bar">
                            <div class="miner-progress-fill ${matured ? 'matured' : ''}" 
                                style="width: ${percentage}%"></div>
                        </div>
                    </div>

                    <div class="miner-card-footer">
                        ${matured
                            ? `<button class="btn btn-sm btn-danger" onclick="endMiner(${minerId})">End Miner</button>`
                            : `<span class="miner-status-badge active">Active</span>`
                        }
                    </div>
                `;

                activeDiv.appendChild(div);
            });
        }

        const frequencyEl = document.getElementById('frequency');
        const liquidHexEl = document.getElementById('liquid-hex');
        const histStartDayEl = document.getElementById('hist-start-day');

        if (frequencyEl) frequencyEl.value = cfg.liveDataFrequency;
        if (liquidHexEl) liquidHexEl.value = cfg.liquidHEX || '';
        if (histStartDayEl) histStartDayEl.value = cfg.historicalStartDay || 1260;

        const existingDiv = document.getElementById('existing-miners');

        if (existingDiv) {
            existingDiv.innerHTML = '';

            window.MINERS_CACHE.forEach(m => {
                const div = document.createElement('div');
                div.className = 'list-item';

                const tSharesText = (Number(m.tShares) || 0).toFixed(2);
                const minerId = Number(m.id) || 0;

                div.innerHTML = `
                    <span>${escapeHtml(m.startDate)} - ${escapeHtml(m.endDate)} • T-Shares: ${tSharesText}</span>
                    <button class="btn btn-sm btn-danger" onclick="deleteMiner(${minerId})">Delete</button>
                `;

                existingDiv.appendChild(div);
            });
        }

        setupLiveDataInterval(cfg.liveDataFrequency);

        if (window.HEXJSON_CACHE.length > 0) {
            requestAnimationFrame(() => {
                renderPortfolioHistoryChartWithData(window.HEXJSON_CACHE);
            });
        }

        if (document.querySelector('.tab-btn.active')) {
            renderAllCharts();
        }

        if (typeof AirDatepicker !== 'undefined') {
            initAirDatepickers();
        }
    } catch (err) {
        console.error('Initial load error:', err);
        showNotification('Failed to load data.', 'danger');
    }
}

// =============================================
// CHARTS
// =============================================

function baseLineOptions(xTitle = null, yTitle = null, yTickCallback = null) {
    return {
        animation: false,
        normalized: true,
        spanGaps: true,
        responsive: true,
        maintainAspectRatio: false,
        interaction: {
            intersect: false,
            mode: 'index'
        },
        scales: {
            x: {
                title: xTitle ? { display: true, text: xTitle, color: '#9ca3af' } : undefined,
                ticks: {
                    color: '#9ca3af',
                    maxTicksLimit: 12,
                    callback: function (value) {
                        const label = this.getLabelForValue(value);
                        const day = Number(label);

                        if (Number.isFinite(day) && day > 0) {
                            return formatHexDayLabel(day);
                        }

                        return label;
                    }
                },
                grid: {
                    color: '#374151'
                }
            },
            y: {
                title: yTitle ? { display: true, text: yTitle, color: '#9ca3af' } : undefined,
                ticks: {
                    color: '#9ca3af',
                    callback: yTickCallback || undefined
                },
                grid: {
                    color: '#374151'
                }
            }
        },
        plugins: {
            legend: {
                display: false
            },
            tooltip: {
                backgroundColor: '#1a1d23',
                titleColor: '#fff',
                bodyColor: '#fff'
            }
        }
    };
}

function renderChartsWithData(data) {
    if (!window.Chart || !Array.isArray(data) || data.length === 0) return;

    const sorted = [...data].sort((a, b) => a.currentDay - b.currentDay);

    const priceData = sorted.filter(e => e.currentDay >= historicalStartDay);
    const latest = sorted[sorted.length - 1] || {};

    const priceEl = document.getElementById('price-value');
    if (priceEl) {
        priceEl.textContent = latest.pricePulseX
            ? `$${Number(latest.pricePulseX).toFixed(5)}`
            : '$0.0000';
    }

    const tshareEl = document.getElementById('tshare-rate-value');
    if (tshareEl) {
        tshareEl.textContent = latest.tshareRateHEX
            ? `${formatWithCommas(Number(latest.tshareRateHEX).toFixed(0))} HEX`
            : '0 HEX';
    }

    const payoutEl = document.getElementById('payout-per-tshare-value');
    if (payoutEl) {
        payoutEl.textContent = latest.payoutPerTshareHEX
            ? `${formatWithCommas(Number(latest.payoutPerTshareHEX).toFixed(2))} HEX`
            : '0.000 HEX';
    }

    const dailyEl = document.getElementById('daily-payout-value');
    if (dailyEl) {
        dailyEl.textContent = latest.dailyPayoutHEX
            ? `${formatWithCommas(Number(latest.dailyPayoutHEX).toFixed(0))} HEX`
            : '0 HEX';
    }

    const configs = [
        { id: 'priceChart', label: 'HEX Price', field: 'pricePulseX', border: '#00b7eb', data: priceData },
        { id: 'tshareRateChart', label: 'T-Share Rate', field: 'tshareRateHEX', border: '#00cc99', data: sorted },
        { id: 'payoutPerTshareChart', label: 'Payout Per T-Share', field: 'payoutPerTshareHEX', border: '#9966ff', data: sorted },
        { id: 'dailyPayoutChart', label: 'Daily Payout', field: 'dailyPayoutHEX', border: '#ff6f61', data: sorted }
    ];

    configs.forEach(c => {
        if (!Array.isArray(c.data)) return;

        const labels = c.data.map(e => e.currentDay);
        const values = c.data.map(e => Number.isFinite(e[c.field]) ? e[c.field] : 0);

        if (chartInstances[c.id]) {
            chartInstances[c.id].data.labels = labels;
            chartInstances[c.id].data.datasets[0].data = values;
            chartInstances[c.id].options = baseLineOptions();
            chartInstances[c.id].update('none');
        } else {
            chartInstances[c.id] = new Chart(
                document.getElementById(c.id).getContext('2d'),
                {
                    type: 'line',
                    data: {
                        labels,
                        datasets: [{
                            label: c.label,
                            data: values,
                            borderColor: c.border,
                            fill: false,
                            pointRadius: 0,
                            pointHoverRadius: 5,
                            tension: 0.25,
                            spanGaps: true
                        }]
                    },
                    options: baseLineOptions()
                }
            );
        }
    });
}

function renderPortfolioHistoryChartWithData(rawData) {
    if (!window.Chart || !Array.isArray(rawData) || rawData.length === 0) return;

    const sorted = [...rawData]
        .sort((a, b) => a.currentDay - b.currentDay)
        .filter(e => e.currentDay >= historicalStartDay);

    if (sorted.length === 0) return;

    const minerRanges = getMinerDayRanges();
    const useDynamicMinerHistory = minerRanges.length > 0 && getLatestHexDay() > 0;

    const portfolio = sorted.map(e => {
        const tSharesForDay = useDynamicMinerHistory
            ? activeTSharesOnDay(e.currentDay, minerRanges)
            : userTotalTShares;

        const tshareRate = Number(e.tshareRateHEX) || 0;
        const price = Number(e.pricePulseX) || 0;

        const minerHex = tSharesForDay * tshareRate;
        const minerUsd = minerHex * price;
        const liquidUsd = userLiquidHEX * price;

        return {
            day: e.currentDay,
            value: minerUsd + liquidUsd
        };
    });

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

    if (gEl) {
        gEl.textContent = `${growth >= 0 ? '+' : ''}$${formatWithCommas(growth.toFixed(2))}`;
        gEl.style.color = growth >= 0 ? 'var(--success)' : 'var(--danger)';
    }

    const pEl = document.getElementById('hist-growth-pct');

    if (pEl) {
        pEl.textContent = `${growth >= 0 ? '+' : ''}${pct.toFixed(2)}%`;
        pEl.style.color = growth >= 0 ? 'var(--success)' : 'var(--danger)';
    }

    const labels = portfolio.map(d => d.day);
    const values = portfolio.map(d => d.value);

    const canvas = document.getElementById('historicalValueChart');
    if (!canvas) return;

    const options = baseLineOptions(
        'Date',
        'Portfolio Value (USD)',
        v => '$' + formatWithCommas(Math.round(v))
    );

    if (chartInstances.historicalValueChart) {
        chartInstances.historicalValueChart.data.labels = labels;
        chartInstances.historicalValueChart.data.datasets[0].data = values;
        chartInstances.historicalValueChart.options = options;
        chartInstances.historicalValueChart.update('none');
    } else {
        chartInstances.historicalValueChart = new Chart(canvas.getContext('2d'), {
            type: 'line',
            data: {
                labels,
                datasets: [{
                    label: 'Portfolio Value',
                    data: values,
                    borderColor: '#00b7eb',
                    backgroundColor: 'rgba(0,183,235,0.15)',
                    borderWidth: 3,
                    tension: 0.25,
                    fill: true,
                    pointRadius: 0,
                    pointHoverRadius: 5,
                    spanGaps: true
                }]
            },
            options
        });
    }

    requestAnimationFrame(() => {
        if (chartInstances.historicalValueChart) {
            chartInstances.historicalValueChart.resize();
        }
    });
}

// =============================================
// ACTIONS & CONFIG
// =============================================

async function endMiner(id) {
    if (await showConfirmModal('End Miner', 'Have you ended the mining contract and minted HEX?')) {
        try {
            const res = await fetch('/api/end-miner', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ id })
            });

            showNotification(
                res.ok ? 'Miner ended successfully' : 'Error ending miner',
                res.ok ? 'success' : 'danger'
            );

            if (res.ok) initialLoad();
        } catch (e) {
            showNotification('Network error', 'danger');
        }
    }
}

async function deleteMiner(id) {
    if (await showConfirmModal('Delete Miner', 'Delete this HEX miner?')) {
        try {
            const res = await fetch('/api/delete-miner', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ id })
            });

            showNotification(
                res.ok ? 'Miner deleted' : 'Error deleting',
                res.ok ? 'success' : 'danger'
            );

            if (res.ok) initialLoad();
        } catch (e) {
            showNotification('Network error', 'danger');
        }
    }
}

function debouncedSaveConfig() {
    if (saveInProgress) return;

    saveInProgress = true;

    const freq = parseInt(document.getElementById('frequency').value) || 15;
    const liquid = parseFloat(document.getElementById('liquid-hex').value) || 0;
    const hist = parseInt(document.getElementById('hist-start-day').value) || 1260;

    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            liveDataFrequency: freq,
            liquidHEX: liquid,
            historicalStartDay: hist
        })
    })
        .then(r => {
            showNotification(r.ok ? 'Settings saved' : 'Error saving', r.ok ? 'success' : 'danger');
            if (r.ok) initialLoad();
        })
        .catch(() => showNotification('Network error', 'danger'))
        .finally(() => {
            saveInProgress = false;
        });
}

function saveFrequency() {
    if (parseInt(document.getElementById('frequency').value) <= 0) {
        return showNotification('Frequency must be > 0', 'danger');
    }

    debouncedSaveConfig();
}

function saveLiquidHEX() {
    if (parseFloat(document.getElementById('liquid-hex').value) < 0) {
        return showNotification('Liquid HEX must be >= 0', 'danger');
    }

    debouncedSaveConfig();
}

function saveHistoricalStartDay() {
    if (parseInt(document.getElementById('hist-start-day').value) < 1) {
        return showNotification('Start day must be > 0', 'danger');
    }

    debouncedSaveConfig();
}

// =============================================
// INIT
// =============================================

document.addEventListener('DOMContentLoaded', () => {
    bindStaticEvents();
    initTickerToggle();

    if (typeof AirDatepicker !== 'undefined') {
        initAirDatepickers();
    } else {
        const checkLoaded = setInterval(() => {
            if (typeof AirDatepicker !== 'undefined') {
                clearInterval(checkLoaded);
                initAirDatepickers();
            }
        }, 100);

        setTimeout(() => clearInterval(checkLoaded), 5000);
    }

    initialLoad();
});
