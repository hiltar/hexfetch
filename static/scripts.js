// ====================== GLOBALS ======================
let chartInstances = { historicalValueChart: null };
let userTotalTShares = 0;
let userLiquidHEX = 0;
let historicalStartDay = 1260;
let currentFrequency = 15;
let nextRefreshTime = Date.now();
let liveDataIntervalId = null;
let countdownIntervalId = null;

// ====================== ANIMATED COUNTER ======================
function animateValue(id, start, end, duration = 1200) {
    const obj = document.getElementById(id);
    if (!obj) return;
    const range = end - start;
    const startTime = Date.now();
    const step = () => {
        const now = Date.now();
        const progress = Math.min((now - startTime) / duration, 1);
        const value = start + range * progress;
        obj.textContent = '$' + value.toFixed(2).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
        if (progress < 1) requestAnimationFrame(step);
    };
    step();
}

// ====================== NOTIFICATIONS & MODALS ======================
function showNotification(message, type = 'success') {
    const container = document.getElementById('notification-container');
    const notif = document.createElement('div');
    notif.className = `alert alert-${type === 'success' ? 'success' : 'danger'} alert-dismissible fade show glass-card`;
    notif.innerHTML = `${message}<button type="button" class="btn-close" data-bs-dismiss="alert"></button>`;
    container.appendChild(notif);
    setTimeout(() => notif.remove(), 4000);
}

function showConfirmModal(title, message) {
    return new Promise(resolve => {
        const modal = document.createElement('div');
        modal.className = 'modal fade';
        modal.innerHTML = `
            <div class="modal-dialog modal-dialog-centered">
                <div class="modal-content glass-card">
                    <div class="modal-header"><h5>${title}</h5><button class="btn-close" data-bs-dismiss="modal"></button></div>
                    <div class="modal-body"><p>${message}</p></div>
                    <div class="modal-footer">
                        <button class="btn btn-secondary" data-bs-dismiss="modal">Cancel</button>
                        <button class="btn btn-primary confirm-btn">Confirm</button>
                    </div>
                </div>
            </div>`;
        document.body.appendChild(modal);
        const bsModal = new bootstrap.Modal(modal);
        bsModal.show();
        modal.querySelector('.confirm-btn').onclick = () => { bsModal.hide(); resolve(true); };
        modal.addEventListener('hidden.bs.modal', () => { modal.remove(); resolve(false); });
    });
}

// ====================== THEME ======================
function setTheme(theme) {
    document.documentElement.setAttribute('data-theme', theme);
    localStorage.setItem('theme', theme);
    document.getElementById('themeIcon').className = theme === 'dark' ? 'bi bi-moon-fill' : 'bi bi-sun-fill';
    renderPortfolioHistoryChart();
}

function toggleTheme() {
    const current = document.documentElement.getAttribute('data-theme') || 'light';
    setTheme(current === 'light' ? 'dark' : 'light');
}

function initTheme() {
    const saved = localStorage.getItem('theme') || (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
    setTheme(saved);
}

function formatWithCommas(num) {
    return num.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

// ====================== LIVE DATA & TIMER RING ======================
function updateTimerRing() {
    const remaining = Math.max(0, nextRefreshTime - Date.now());
    const totalMs = currentFrequency * 60 * 1000;
    const progress = (remaining / totalMs) * 213; // circumference ≈ 213
    document.getElementById('timer-progress').setAttribute('stroke-dashoffset', progress);
}

function fetchLiveData() {
    fetch('/api/live-data')
        .then(r => r.json())
        .then(data => {
            // Update live data content (you can expand this)
            document.getElementById('live-data-content').innerHTML = `
                <p><strong>Price:</strong> $${data.price_Pulsechain.toFixed(5)}</p>
                <p><strong>T-Share Price:</strong> $${data.tsharePrice_Pulsechain.toFixed(2)}</p>
                <p><strong>T-Share Rate:</strong> HEX{data.tshareRateHEX_Pulsechain.toFixed(2)}</p>
                <p><strong>Payout:</strong> HEX{data.payoutPerTshare_Pulsechain.toFixed(1}</p>
                <p><strong>Beat:</strong> {data.beat}</p>
            `;
            nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
        });
}



// ====================== PORTFOLIO HISTORY CHART (with animation) ======================
function renderPortfolioHistoryChart() {
    fetch('/api/hexjson').then(r => r.json()).then(raw => {
        const sorted = [...raw].sort((a,b) => a.currentDay - b.currentDay);
        const filtered = sorted.filter(e => e.currentDay >= historicalStartDay);
        if (!filtered.length) return;

        const portfolioData = filtered.map(entry => {
            const hexPrice = entry.pricePulseX || 0;
            return userTotalTShares * (entry.tshareRateHEX * hexPrice) + userLiquidHEX * hexPrice;
        });

        const startVal = portfolioData[0];
        const currVal = portfolioData.at(-1);
        const athVal = Math.max(...portfolioData);
        const growth = currVal - startVal;
        const growthPct = startVal ? (growth / startVal) * 100 : 0;

        // Animate stats
        animateValue('hist-start-value', 0, startVal);
        animateValue('hist-current-value', 0, currVal);
        animateValue('hist-ath', 0, athVal);
        document.getElementById('hist-growth').textContent = (growth >= 0 ? '+' : '') + '$' + growth.toFixed(2);
        document.getElementById('hist-growth').className = growth >= 0 ? 'h5 text-success' : 'h5 text-danger';
        document.getElementById('hist-growth-pct').textContent = (growth >= 0 ? '+' : '') + growthPct.toFixed(2) + '%';
        document.getElementById('hist-growth-pct').className = growth >= 0 ? 'small fw-bold text-success' : 'small fw-bold text-danger';

        // Chart
        if (chartInstances.historicalValueChart) chartInstances.historicalValueChart.destroy();
        const ctx = document.getElementById('historicalValueChart').getContext('2d');
        chartInstances.historicalValueChart = new Chart(ctx, {
            type: 'line',
            data: {
                labels: filtered.map(e => e.currentDay),
                datasets: [{
                    label: 'Portfolio Value',
                    data: portfolioData,
                    borderColor: '#00d4ff',
                    backgroundColor: 'rgba(0,212,255,0.15)',
                    tension: 0.35,
                    borderWidth: 4,
                    fill: true
                }]
            },
            options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } } }
        });
    });
}

// ====================== PROFILE ======================
function fetchProfile() {
    fetch('/api/miners').then(r => r.json()).then(miners => {
        let total = 0;
        miners.forEach(m => { if (m.status !== 'completed') total += m.tShares; });
        userTotalTShares = total;

        fetch('/api/config').then(r => r.json()).then(config => {
            userLiquidHEX = config.liquidHEX || 0;
            historicalStartDay = config.historicalStartDay || 1260;
            document.getElementById('hist-start-day-label').textContent = historicalStartDay;
            renderPortfolioHistoryChart();
        });
    });
}

function saveFrequency() {
    const frequency = parseInt(document.getElementById('frequency').value);
    if (frequency <= 0) {
        showNotification('Frequency must be a positive integer', 'danger');
        return;
    }
    const liquidHEX = parseFloat(document.getElementById('liquid-hex').value) || 0;
    const histStart = parseInt(document.getElementById('hist-start-day').value) || 1260;
    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ liveDataFrequency: frequency, liquidHEX: liquidHEX, historicalStartDay: histStart })
    })
        .then(response => {
            if (response.ok) {
                showNotification(`Live data update frequency set to ${frequency} minutes`, 'success');
                // Update the live data interval
                setupLiveDataInterval(frequency);
                renderPortfolioHistoryChart();
            } else {
                showNotification('Error saving frequency', 'danger');
            }
        });
}

function saveLiquidHEX() {
    const liquidHEX = parseFloat(document.getElementById('liquid-hex').value);
    if (isNaN(liquidHEX) || liquidHEX < 0) {
        showNotification('Liquid HEX must be a non-negative number', 'danger');
        return;
    }
    const frequency = parseInt(document.getElementById('frequency').value) || 15;
    const histStart = parseInt(document.getElementById('hist-start-day').value) || 1260;
    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ liveDataFrequency: frequency, liquidHEX: liquidHEX, historicalStartDay: histStart })
    })
        .then(response => {
            if (response.ok) {
                showNotification('Liquid HEX saved successfully', 'success');
                userLiquidHEX = liquidHEX;
                fetchProfile(); // Refresh Profile tab
                renderPortfolioHistoryChart();
            } else {
                showNotification('Error saving Liquid HEX', 'danger');
            }
        });
}

function saveHistoricalStartDay() {
    const histStart = parseInt(document.getElementById('hist-start-day').value);
    if (isNaN(histStart) || histStart < 1) {
        showNotification('Starting day must be a positive integer', 'danger');
        return;
    }
    const frequency = parseInt(document.getElementById('frequency').value) || 15;
    const liquidHEX = parseFloat(document.getElementById('liquid-hex').value) || 0;
    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ liveDataFrequency: frequency, liquidHEX: liquidHEX, historicalStartDay: histStart })
    })
        .then(response => {
            if (response.ok) {
                showNotification(`Historical chart starting day set to ${histStart}`, 'success');
                historicalStartDay = histStart;
                renderPortfolioHistoryChart(); // Refresh chart
            } else {
                showNotification('Error saving historical start day', 'danger');
            }
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
    fetch('/api/add-miner', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ startDate, endDate, tShares })
    })
        .then(response => {
            if (response.ok) {
                showNotification('Miner added successfully', 'success');
                fetchProfile();
                fetchSettings();
                renderPortfolioHistoryChart();
                document.getElementById('start-date').value = '';
                document.getElementById('end-date').value = '';
                document.getElementById('tshares').value = '';
            } else {
                showNotification('Error adding miner', 'danger');
            }
        });
}

async function deleteMiner(index) {
    const confirmed = await showConfirmModal(
        'Delete Miner',
        'Do you want to delete this HEX miner?'
    );
    if (confirmed) {
        fetch('/api/delete-miner', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ index })
        })
            .then(response => {
                if (response.ok) {
                    showNotification('Miner deleted successfully', 'success');
                    fetchSettings();
                    fetchProfile();
                    renderPortfolioHistoryChart();
                } else {
                    showNotification('Error deleting miner', 'danger');
                }
            });
    }
}


// ====================== INITIALIZE ======================
document.addEventListener('DOMContentLoaded', () => {
    initTheme();
    fetchProfile();
    fetchLiveData();
    // Periodic updates
    setInterval(fetchProfile, 3 * 600000);
    setInterval(fetchLiveData, 60000);
    setInterval(updateTimerRing, 10000);
    // Initial render
    renderPortfolioHistoryChart();
});
