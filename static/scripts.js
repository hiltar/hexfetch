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
let wsConnection = null;
let wsActive = false;               // true when WebSocket is live
let saveInProgress = false;         // debounce flag for config saves

// =============================================
// HELPERS
// =============================================
function showNotification(message, type = 'success') {
    const container = document.getElementById('notification-container');
    const notification = document.createElement('div');
    notification.className = `notification alert alert-${type === 'success' ? 'success' : 'danger'} alert-dismissible fade show`;
    notification.setAttribute('role', 'alert');
    notification.innerHTML = `
        ${message}
        <button type="button" class="btn-close" data-bs-dismiss="alert" aria-label="Close"></button>
    `;
    container.appendChild(notification);

    setTimeout(() => {
        notification.classList.remove('show');
        notification.classList.add('fade');
        setTimeout(() => notification.remove(), 150);
    }, 3000);
}

function showConfirmModal(title, message) {
    return new Promise((resolve) => {
        const modal = document.createElement('div');
        modal.className = 'modal fade';
        modal.innerHTML = `
            <div class="modal-dialog modal-dialog-centered">
                <div class="modal-content">
                    <div class="modal-header">
                        <h5 class="modal-title">${title}</h5>
                        <button type="button" class="btn-close" data-bs-dismiss="modal" aria-label="Close"></button>
                    </div>
                    <div class="modal-body">
                        <p>${message}</p>
                    </div>
                    <div class="modal-footer">
                        <button type="button" class="btn btn-secondary" data-bs-dismiss="modal">Cancel</button>
                        <button type="button" class="btn btn-primary confirm-btn">Confirm</button>
                    </div>
                </div>
            </div>
        `;
        document.body.appendChild(modal);

        const bsModal = new bootstrap.Modal(modal, { backdrop: 'static', keyboard: false });
        bsModal.show();

        const confirmBtn = modal.querySelector('.confirm-btn');
        const cancelBtn = modal.querySelector('.btn-secondary');
        const closeBtn = modal.querySelector('.btn-close');

        const cleanup = () => {
            bsModal.hide();
            modal.remove();
        };

        confirmBtn.addEventListener('click', () => { cleanup(); resolve(true); });
        cancelBtn.addEventListener('click', () => { cleanup(); resolve(false); });
        closeBtn.addEventListener('click', () => { cleanup(); resolve(false); });
        modal.addEventListener('hidden.bs.modal', () => { cleanup(); resolve(false); });
    });
}

function formatWithCommas(num) {
    return num.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

// =============================================
// PROFILE STATS HELPER
// =============================================
function updateProfileStats() {
    if (!liveDataCache) {
        // If no live data yet, zero out values
        document.getElementById('total-value').textContent = '0.00';
        document.getElementById('liquid-hex-value').textContent = '0.00';
        document.getElementById('interest-hex').textContent = '0.00 HEX';
        document.getElementById('interest-usd').textContent = '$0.00';
        return;
    }
    const data = liveDataCache;
    const totalValue = userTotalTShares * data.tsharePrice_Pulsechain;
    document.getElementById('total-value').textContent = formatWithCommas(totalValue.toFixed(2));

    const dailyInterestHEX = userTotalTShares * data.payoutPerTshare_Pulsechain;
    const dailyInterestUSD = dailyInterestHEX * data.price_Pulsechain;
    document.getElementById('interest-hex').textContent = formatWithCommas(dailyInterestHEX.toFixed(2)) + ' HEX';
    document.getElementById('interest-usd').textContent = '$' + formatWithCommas(dailyInterestUSD.toFixed(2));

    const liquidHEXValue = userLiquidHEX * data.price_Pulsechain;
    document.getElementById('liquid-hex-value').textContent = formatWithCommas(liquidHEXValue.toFixed(2));
}

// =============================================
// LIVE DATA
// =============================================
function updateLiveDataUI(data) {
    liveDataCache = data;

    document.getElementById('price').textContent = data.price_Pulsechain.toFixed(5);
    document.getElementById('tshare-price').textContent = data.tsharePrice_Pulsechain.toFixed(2);
    document.getElementById('tshare-rate').textContent = formatWithCommas(Math.floor(data.tshareRateHEX_Pulsechain));
    document.getElementById('payout').textContent = data.payoutPerTshare_Pulsechain.toFixed(3);
    document.getElementById('penalties').textContent = formatWithCommas(Math.floor(data.penaltiesHEX_Pulsechain));
    document.getElementById('beat').textContent = formatWithCommas(data.beat);

    const timestamp = new Date().toLocaleTimeString();
    document.getElementById('last-updated').textContent = `Last updated: ${timestamp}`;
    document.title = `HEX Stats - $${data.price_Pulsechain.toFixed(5)}`;

    nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
    updateCountdown();

    // Update profile tab values as well
    updateProfileStats();
}

function updateCountdown() {
    const remainingMs = Math.max(0, nextRefreshTime - Date.now());
    const totalSec = Math.floor(remainingMs / 1000);
    const mins = Math.floor(totalSec / 60);
    const secs = totalSec % 60;
    document.getElementById('countdown').textContent = `${mins}:${secs.toString().padStart(2, '0')}`;
}

function connectLiveWebSocket() {
    if (wsConnection) wsConnection.close();

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    wsConnection = new WebSocket(`${protocol}//${window.location.host}/ws/live-data`);

    wsConnection.onopen = () => {
        console.log('✅ WebSocket connected');
        wsActive = true;
    };
    wsConnection.onmessage = (event) => {
        try {
            updateLiveDataUI(JSON.parse(event.data));
        } catch (e) {
            console.error('WebSocket parse error', e);
        }
    };
    wsConnection.onclose = () => {
        wsActive = false;
        setTimeout(connectLiveWebSocket, 5000);
    };
    wsConnection.onerror = (err) => {
        console.error('WebSocket error', err);
        // onclose will fire after error, no need to force reconnect here
    };
}

function setupLiveDataInterval(frequencyInMinutes) {
    currentFrequency = frequencyInMinutes;
    nextRefreshTime = Date.now() + frequencyInMinutes * 60 * 1000;

    if (countdownIntervalId) clearInterval(countdownIntervalId);
    countdownIntervalId = setInterval(updateCountdown, 1000);
    updateCountdown();

    if (!wsConnection || wsConnection.readyState !== WebSocket.OPEN) {
        connectLiveWebSocket();
    }
}

// =============================================
// DATA RETRIEVAL & UI UPDATE
// =============================================
async function initialLoad() {
    try {
        // Always fetch live data if cache is empty; otherwise skip HTTP call.
        const promises = [fetch('/api/miners'), fetch('/api/config'), fetch('/api/hexjson')];
        if (!liveDataCache) {
            promises.push(fetch('/api/live-data'));
        }
        const results = await Promise.all(promises);
        let i = 0;
        const miners = await results[i++].json();
        const config = await results[i++].json();
        const hexjsonData = await results[i++].json();
        let liveData = null;
        if (!liveDataCache) {
            liveData = await results[i++].json();
            updateLiveDataUI(liveData);
        }

        // Profile calculations
        let totalTShares = 0;
        let activeMiners = [];
        let minerIndices = [];

        miners.forEach((miner, originalIndex) => {
            if (miner.status !== 'completed') {
                totalTShares += miner.tShares;
                activeMiners.push(miner);
                minerIndices.push(originalIndex);
            }
        });

        userTotalTShares = totalTShares;
        document.getElementById('total-tshares').textContent = totalTShares.toFixed(2);

        userLiquidHEX = config.liquidHEX || 0;
        updateProfileStats();  // this will use liveDataCache (may be from WS or just fetched)

        // Active miners
        const activeMinersDiv = document.getElementById('active-miners');
        activeMinersDiv.innerHTML = '';
        if (activeMiners.length === 0) {
            document.getElementById('profile-message').textContent = 'Empty profile. Please add HEX miners in Settings.';
        } else {
            document.getElementById('profile-message').textContent = '';
            activeMiners.forEach((miner, index) => {
                // Use UTC maturity check to align with server date parsing
                const [d, m, y] = miner.endDate.split('-');
                const endDateUTC = Date.UTC(y, m - 1, d);
                const nowUTC = Date.now();
                const isMatured = endDateUTC <= nowUTC;
                const daysLeft = isMatured ? 0 : Math.ceil((endDateUTC - nowUTC) / (1000 * 60 * 60 * 24));

                const minerDiv = document.createElement('div');
                minerDiv.className = 'miner-item';
                minerDiv.innerHTML = `
                    <span>${miner.startDate} to ${miner.endDate}, T-Shares: ${miner.tShares.toFixed(2)} ${isMatured ? '(Matured)' : `(${daysLeft} days left)`}</span>
                    ${isMatured ? `<button class="btn btn-sm btn-danger" onclick="endMiner(${minerIndices[index]})">End</button>` : ''}
                `;
                activeMinersDiv.appendChild(minerDiv);
            });
        }

        // Settings fields
        document.getElementById('frequency').value = config.liveDataFrequency;
        document.getElementById('liquid-hex').value = config.liquidHEX || '';
        document.getElementById('hist-start-day').value = config.historicalStartDay || 1260;
        historicalStartDay = config.historicalStartDay || 1260;

        // Existing miners list in Settings
        const existingMinersDiv = document.getElementById('existing-miners');
        existingMinersDiv.innerHTML = '';
        miners.forEach((miner, index) => {
            const minerDiv = document.createElement('div');
            minerDiv.className = 'miner-item';
            minerDiv.innerHTML = `
                <span>${miner.startDate} to ${miner.endDate}, T-Shares: ${miner.tShares.toFixed(2)}</span>
                <button class="btn btn-sm btn-danger" onclick="deleteMiner(${index})">Delete</button>
            `;
            existingMinersDiv.appendChild(minerDiv);
        });

        setupLiveDataInterval(config.liveDataFrequency);
        // Render charts only if Chart.js is loaded (i.e. user visited Charts tab)
        if (window.Chart) {
            renderChartsWithData(hexjsonData, true); // update, not recreate
            renderPortfolioHistoryChartWithData(hexjsonData, true);
        }

    } catch (error) {
        console.error('Error during initial load:', error);
        showNotification('Failed to load initial data. Please refresh the page.', 'danger');
    }
}

// =============================================
// CHARTS (with update optimisation)
// =============================================

// Lazy-load Chart.js when the Charts tab is activated
function loadChartJS() {
    return new Promise((resolve, reject) => {
        if (window.Chart) {
            resolve();
            return;
        }
        const script = document.createElement('script');
        script.src = 'https://cdn.jsdelivr.net/npm/chart.js@4.5.1/dist/chart.umd.min.js';
        script.onload = resolve;
        script.onerror = () => reject(new Error('Failed to load Chart.js'));
        document.head.appendChild(script);
    });
}

function renderChartsWithData(data, updateOnly = false) {
    if (!window.Chart) return;

    const sortedData = [...data].sort((a, b) => a.currentDay - b.currentDay);
    const priceFilteredData = sortedData.filter(entry => entry.currentDay >= 1260);
    const latestData = sortedData[sortedData.length - 1] || {};

    document.getElementById('price-value').textContent = latestData.pricePulseX ? `$${latestData.pricePulseX.toFixed(4)}` : '$0.0000';
    document.getElementById('tshare-rate-value').textContent = latestData.tshareRateHEX ? `${formatWithCommas(latestData.tshareRateHEX)} HEX` : '0 HEX';
    document.getElementById('payout-per-tshare-value').textContent = latestData.payoutPerTshareHEX ? `${formatWithCommas(latestData.payoutPerTshareHEX.toFixed(3))} HEX` : '0.000 HEX';
    document.getElementById('daily-payout-value').textContent = latestData.dailyPayoutHEX ? `${formatWithCommas(latestData.dailyPayoutHEX)} HEX` : '0 HEX';

    const isDarkTheme = true;
    const chartConfigs = [
        { id: 'priceChart', label: 'HEX Price', field: 'pricePulseX', borderColor: isDarkTheme ? '#00b7eb' : '#007bff', data: priceFilteredData },
        { id: 'tshareRateChart', label: 'T-Share Rate', field: 'tshareRateHEX', borderColor: isDarkTheme ? '#00cc99' : '#28a745', data: sortedData },
        { id: 'payoutPerTshareChart', label: 'Payout Per T-Share', field: 'payoutPerTshareHEX', borderColor: isDarkTheme ? '#9966ff' : '#9900cc', data: sortedData },
        { id: 'dailyPayoutChart', label: 'Daily Payout', field: 'dailyPayoutHEX', borderColor: isDarkTheme ? '#ff6f61' : '#dc3545', data: sortedData }
    ];

    chartConfigs.forEach(config => {
        const labels = config.data.map(e => e.currentDay);
        const values = config.data.map(e => e[config.field]);

        if (chartInstances[config.id]) {
            const chart = chartInstances[config.id];
            chart.data.labels = labels;
            chart.data.datasets[0].data = values;
            chart.update('none');
        } else {
            const ctx = document.getElementById(config.id).getContext('2d');
            chartInstances[config.id] = new Chart(ctx, {
                type: 'line',
                data: {
                    labels: labels,
                    datasets: [{
                        label: config.label,
                        data: values,
                        borderColor: config.borderColor,
                        fill: false,
                        pointRadius: 0,
                        pointHoverRadius: 5,
                        tension: 0.25
                    }]
                },
                options: {
                    responsive: true,
                    maintainAspectRatio: false,
                    interaction: {
                        intersect: false,
                        mode: 'index'
                    },
                    scales: {
                        x: { title: { display: true, text: 'Current Day', color: isDarkTheme ? '#ffffff' : '#000000' }, ticks: { color: isDarkTheme ? '#ffffff' : '#000000' } },
                        y: { title: { display: true, text: config.label, color: isDarkTheme ? '#ffffff' : '#000000' }, ticks: { color: isDarkTheme ? '#ffffff' : '#000000' } }
                    },
                    plugins: {
                        legend: { labels: { color: isDarkTheme ? '#ffffff' : '#000000' } },
                        tooltip: {
                            callbacks: {
                                title: (tooltipItems) => `Day ${tooltipItems[0].label}`,
                                label: (tooltipItem) => `${config.label}: ${tooltipItem.raw.toFixed(2)}`
                            }
                        }
                    }
                }
            });
        }
    });
}

function renderPortfolioHistoryChartWithData(rawData, updateOnly = false) {
    if (!window.Chart) return;

    const sortedData = [...rawData].sort((a, b) => a.currentDay - b.currentDay);
    const filteredData = sortedData.filter(entry => entry.currentDay >= historicalStartDay);
    if (filteredData.length === 0) return;

    const portfolioData = filteredData.map(entry => {
        const hexPrice = entry.pricePulseX || 0;
        const tsharePrice = entry.tshareRateHEX * hexPrice;
        const portfolioValue = (userTotalTShares * tsharePrice) + (userLiquidHEX * hexPrice);
        return { day: entry.currentDay, value: portfolioValue };
    });

    const startVal = portfolioData[0].value;
    const currVal = portfolioData[portfolioData.length - 1].value;
    const athVal = Math.max(...portfolioData.map(d => d.value));
    const growth = currVal - startVal;
    const growthPct = startVal > 0 ? (growth / startVal) * 100 : 0;

    document.getElementById('hist-start-day-label').textContent = historicalStartDay;
    document.getElementById('hist-start-value').textContent = '$' + formatWithCommas(startVal.toFixed(2));
    document.getElementById('hist-current-value').textContent = '$' + formatWithCommas(currVal.toFixed(2));
    document.getElementById('hist-ath').textContent = '$' + formatWithCommas(athVal.toFixed(2));

    const growthEl = document.getElementById('hist-growth');
    growthEl.textContent = (growth >= 0 ? '+' : '') + '$' + formatWithCommas(growth.toFixed(2));
    growthEl.className = growth >= 0 ? 'h5 fw-bold text-success' : 'h5 fw-bold text-danger';

    const pctEl = document.getElementById('hist-growth-pct');
    pctEl.textContent = (growth >= 0 ? '+' : '') + growthPct.toFixed(2) + '%';
    pctEl.className = growth >= 0 ? 'small fw-bold text-success' : 'small fw-bold text-danger';

    const isDarkTheme = true;
    const labels = portfolioData.map(d => d.day);
    const values = portfolioData.map(d => d.value);

    if (chartInstances.historicalValueChart) {
        const chart = chartInstances.historicalValueChart;
        chart.data.labels = labels;
        chart.data.datasets[0].data = values;
        chart.update('none');
    } else {
        const ctx = document.getElementById('historicalValueChart').getContext('2d');
        chartInstances.historicalValueChart = new Chart(ctx, {
            type: 'line',
            data: {
                labels: labels,
                datasets: [{
                    label: 'Value',
                    data: values,
                    borderColor: isDarkTheme ? '#00b7eb' : '#007bff',
                    backgroundColor: isDarkTheme ? 'rgba(0,183,235,0.2)' : 'rgba(0,123,255,0.15)',
                    borderWidth: 3,
                    tension: 0.25,
                    fill: true,
                    pointRadius: 0,
                    pointHoverRadius: 5
                }]
            },
            options: {
                responsive: true,
                maintainAspectRatio: false,
                interaction: { intersect: false, mode: 'index' },
                scales: {
                    x: { title: { display: true, text: 'Day', color: isDarkTheme ? '#ffffff' : '#000000' }, ticks: { color: isDarkTheme ? '#ffffff' : '#000000', maxTicksLimit: 15 } },
                    y: { title: { display: true, text: 'Portfolio Value (USD)', color: isDarkTheme ? '#ffffff' : '#000000' }, ticks: { color: isDarkTheme ? '#ffffff' : '#000000', callback: v => '$' + formatWithCommas(Math.round(v)) } }
                },
                plugins: {
                    legend: { display: false },
                    tooltip: {
                        displayColors: false,
                        backgroundColor: isDarkTheme ? '#343a40' : '#ffffff',
                        titleColor: isDarkTheme ? '#ffffff' : '#000000',
                        bodyColor: isDarkTheme ? '#ffffff' : '#000000',
                        borderColor: isDarkTheme ? '#6c757d' : '#dee2e6',
                        borderWidth: 1,
                        callbacks: {
                            title: items => 'Day ' + items[0].label,
                            label: ctx => 'Value: $' + formatWithCommas(ctx.raw.toFixed(2))
                        }
                    }
                }
            }
        });
    }
}

// =============================================
// MINER ACTIONS
// =============================================
async function endMiner(index) {
    const confirmed = await showConfirmModal('End Miner', 'Have you ended the mining contract and minted HEX?');
    if (confirmed) {
        fetch('/api/end-miner', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ index })
        })
        .then(response => {
            if (response.ok) {
                showNotification('Miner ended successfully', 'success');
                initialLoad();
            } else {
                showNotification('Error ending miner', 'danger');
            }
        });
    }
}

function showCompletedMiners() {
    fetch('/api/miners')
        .then(response => response.json())
        .then(miners => {
            const completedMiners = miners.filter(miner => miner.status === 'completed');
            const modal = document.createElement('div');
            modal.className = 'modal fade';
            modal.innerHTML = `
                <div class="modal-dialog">
                    <div class="modal-content">
                        <div class="modal-header">
                            <h5 class="modal-title">Completed Miners</h5>
                            <button type="button" class="btn-close" data-bs-dismiss="modal"></button>
                        </div>
                        <div class="modal-body">
                            ${completedMiners.length === 0 ? '<p>No completed miners.</p>' : completedMiners.map(miner => `<p>${miner.startDate} to ${miner.endDate}, T-Shares: ${miner.tShares.toFixed(2)}</p>`).join('')}
                        </div>
                        <div class="modal-footer">
                            <button type="button" class="btn btn-secondary" data-bs-dismiss="modal">Close</button>
                        </div>
                    </div>
                </div>
            `;
            document.body.appendChild(modal);
            const bsModal = new bootstrap.Modal(modal);
            bsModal.show();
            modal.addEventListener('hidden.bs.modal', () => modal.remove());
        });
}

function addMiner() {
    const startDate = document.getElementById('start-date').value;
    const endDate = document.getElementById('end-date').value;
    const tShares = parseFloat(document.getElementById('tshares').value);

    if (!startDate || !endDate || isNaN(tShares) || tShares <= 0) {
        showNotification('Please fill all fields with valid data', 'danger');
        return;
    }
    const dateRegex = /^\d{2}-\d{2}-\d{4}$/;
    if (!dateRegex.test(startDate) || !dateRegex.test(endDate)) {
        showNotification('Dates must be in DD-MM-YYYY format', 'danger');
        return;
    }

    // Prevent accidental double clicks
    const btn = document.querySelector('button[onclick="addMiner()"]');
    if (btn) btn.disabled = true;

    fetch('/api/add-miner', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ startDate, endDate, tShares })
    })
    .then(response => {
        if (response.ok) {
            showNotification('Miner added successfully', 'success');
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
        if (btn) btn.disabled = false;
    });
}

async function deleteMiner(index) {
    const confirmed = await showConfirmModal('Delete Miner', 'Do you want to delete this HEX miner?');
    if (confirmed) {
        fetch('/api/delete-miner', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ index })
        })
        .then(response => {
            if (response.ok) {
                showNotification('Miner deleted successfully', 'success');
                initialLoad();
            } else {
                showNotification('Error deleting miner', 'danger');
            }
        });
    }
}

// =============================================
// DEBOUNCED CONFIG SAVE (used by all three save buttons)
// =============================================
function debouncedSaveConfig() {
    if (saveInProgress) return;
    saveInProgress = true;

    const frequency = parseInt(document.getElementById('frequency').value) || 15;
    const liquidHEX = parseFloat(document.getElementById('liquid-hex').value) || 0;
    const histStart = parseInt(document.getElementById('hist-start-day').value) || 1260;

    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ liveDataFrequency: frequency, liquidHEX: liquidHEX, historicalStartDay: histStart })
    })
    .then(response => {
        if (response.ok) {
            showNotification('Settings saved', 'success');
            initialLoad();
        } else {
            showNotification('Error saving settings', 'danger');
        }
    })
    .catch(() => showNotification('Network error', 'danger'))
    .finally(() => { saveInProgress = false; });
}

function saveFrequency() {
    const freq = parseInt(document.getElementById('frequency').value);
    if (isNaN(freq) || freq <= 0) {
        showNotification('Frequency must be a positive integer', 'danger');
        return;
    }
    debouncedSaveConfig();
}

function saveLiquidHEX() {
    const liquid = parseFloat(document.getElementById('liquid-hex').value);
    if (isNaN(liquid) || liquid < 0) {
        showNotification('Liquid HEX must be non-negative', 'danger');
        return;
    }
    debouncedSaveConfig();
}

function saveHistoricalStartDay() {
    const start = parseInt(document.getElementById('hist-start-day').value);
    if (isNaN(start) || start < 1) {
        showNotification('Starting day must be a positive integer', 'danger');
        return;
    }
    debouncedSaveConfig();
}

// =============================================
// INITIALIZATION & TAB LAZY LOADING
// =============================================
document.addEventListener('DOMContentLoaded', () => {
    document.title = 'HEX Stats';

    const datepickerOptions = {
        format: 'dd-mm-yyyy',
        autohide: true,
        buttonClass: 'btn',
        prevButton: '<i class="bi bi-chevron-left"></i>',
        nextButton: '<i class="bi bi-chevron-right"></i>'
    };

    const startDateInput = document.getElementById('start-date');
    const endDateInput   = document.getElementById('end-date');
    if (startDateInput) new Datepicker(startDateInput, datepickerOptions);
    if (endDateInput)   new Datepicker(endDateInput,   datepickerOptions);

    // Lazy-load Chart.js when the Charts tab is first shown
    const chartTab = document.getElementById('chart-tab');
    if (chartTab) {
        chartTab.addEventListener('shown.bs.tab', () => {
            loadChartJS().then(() => {
                fetch('/api/hexjson').then(r => r.json()).then(data => {
                    renderChartsWithData(data);
                    renderPortfolioHistoryChartWithData(data);
                });
            }).catch(err => console.error(err));
        });
    }

    // Initial load and periodic refresh every 10 minutes
    initialLoad();
    setInterval(initialLoad, 10 * 60 * 1000);
});
