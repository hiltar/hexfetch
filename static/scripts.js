// =============================================
// GLOBAL STATE
// =============================================
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
let liveDataCache = null;           // ← Optimized: cache latest live data

// Live data timer variables
let liveDataIntervalId = null;
let countdownIntervalId = null;
let nextRefreshTime = Date.now();
let currentFrequency = 15;

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
// LIVE DATA (Optimized)
// =============================================
async function fetchLiveData() {
    try {
        const response = await fetch('/api/live-data');
        if (!response.ok) throw new Error(`HTTP error! status: ${response.status}`);
        const data = await response.json();
        liveDataCache = data;   // Cache for other functions

        // Update Live Data tab
        document.getElementById('price').textContent = data.price_Pulsechain.toFixed(5);
        document.getElementById('tshare-price').textContent = data.tsharePrice_Pulsechain.toFixed(2);
        document.getElementById('tshare-rate').textContent = formatWithCommas(Math.floor(data.tshareRateHEX_Pulsechain));
        document.getElementById('payout').textContent = data.payoutPerTshare_Pulsechain.toFixed(1);
        document.getElementById('penalties').textContent = formatWithCommas(Math.floor(data.penaltiesHEX_Pulsechain));
        document.getElementById('beat').textContent = formatWithCommas(data.beat);

        const timestamp = new Date().toLocaleTimeString();
        document.getElementById('last-updated').textContent = `Last updated: ${timestamp}`;
        document.title = `HEX Stats - $${data.price_Pulsechain.toFixed(5)}`;

        // Reset countdown
        nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
        updateCountdown();
    } catch (error) {
        console.error('Error fetching live data:', error);
        document.getElementById('last-updated').textContent = `Error updating data: ${error.message}`;
        document.title = 'HEX Stats';
    }
}

function updateCountdown() {
    const remainingMs = Math.max(0, nextRefreshTime - Date.now());
    const totalSec = Math.floor(remainingMs / 1000);
    const mins = Math.floor(totalSec / 60);
    const secs = totalSec % 60;
    document.getElementById('countdown').textContent = `${mins}:${secs.toString().padStart(2, '0')}`;
}

function setupLiveDataInterval(frequencyInMinutes) {
    if (liveDataIntervalId) clearInterval(liveDataIntervalId);
    if (countdownIntervalId) clearInterval(countdownIntervalId);

    currentFrequency = frequencyInMinutes;
    nextRefreshTime = Date.now() + frequencyInMinutes * 60 * 1000;

    countdownIntervalId = setInterval(updateCountdown, 1000);
    updateCountdown();

    liveDataIntervalId = setInterval(() => {
        console.log(`Fetching live data every ${frequencyInMinutes} minute(s)...`);
        fetchLiveData();
    }, frequencyInMinutes * 60 * 1000);
}

// =============================================
// PROFILE (Optimized with Promise.all)
// =============================================
async function fetchProfile() {
    try {
        const [minersRes, liveRes, configRes] = await Promise.all([
            fetch('/api/miners'),
            fetch('/api/live-data'),
            fetch('/api/config')
        ]);

        const miners = await minersRes.json();
        const liveData = await liveRes.json();
        const config = await configRes.json();

        // Cache live data
        liveDataCache = liveData;

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

        // Use cached live data
        const totalValue = totalTShares * liveData.tsharePrice_Pulsechain;
        document.getElementById('total-value').textContent = formatWithCommas(totalValue.toFixed(2));

        const dailyInterestHEX = totalTShares * liveData.payoutPerTshare_Pulsechain;
        const dailyInterestUSD = dailyInterestHEX * liveData.price_Pulsechain;
        document.getElementById('interest-hex').textContent = formatWithCommas(dailyInterestHEX.toFixed(2)) + ' HEX';
        document.getElementById('interest-usd').textContent = '$' + formatWithCommas(dailyInterestUSD.toFixed(2));

        // Liquid HEX
        userLiquidHEX = config.liquidHEX || 0;
        const liquidHEXValue = userLiquidHEX * liveData.price_Pulsechain;
        document.getElementById('liquid-hex-value').textContent = formatWithCommas(liquidHEXValue.toFixed(2));

        // Render active miners
        const activeMinersDiv = document.getElementById('active-miners');
        activeMinersDiv.innerHTML = '';
        if (activeMiners.length === 0) {
            document.getElementById('profile-message').textContent = 'Empty profile. Please add HEX miners in Settings.';
            return;
        }
        document.getElementById('profile-message').textContent = '';

        activeMiners.forEach((miner, index) => {
            const endDateObj = new Date(miner.endDate.split('-').reverse().join('-'));
            const isMatured = endDateObj <= new Date();
            const daysLeft = isMatured ? 0 : Math.ceil((endDateObj - new Date()) / (1000 * 60 * 60 * 24));

            const minerDiv = document.createElement('div');
            minerDiv.className = 'miner-item';
            minerDiv.innerHTML = `
                <span>${miner.startDate} to ${miner.endDate}, T-Shares: ${miner.tShares.toFixed(2)} ${isMatured ? '(Matured)' : `(${daysLeft} days left)`}</span>
                ${isMatured ? `<button class="btn btn-sm btn-danger" onclick="endMiner(${minerIndices[index]})">End</button>` : ''}
            `;
            activeMinersDiv.appendChild(minerDiv);
        });

        renderPortfolioHistoryChart();
    } catch (error) {
        console.error('Error fetching profile:', error);
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
                fetchProfile();
            } else {
                showNotification('Error ending miner', 'danger');
            }
        });
    }
}

// =============================================
// CHARTS
// =============================================
function renderPortfolioHistoryChart() {
    fetch('/api/hexjson')
        .then(response => {
            if (!response.ok) throw new Error(`HTTP error! status: ${response.status}`);
            return response.json();
        })
        .then(rawData => {
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
            if (chartInstances.historicalValueChart) chartInstances.historicalValueChart.destroy();

            const ctx = document.getElementById('historicalValueChart').getContext('2d');
            chartInstances.historicalValueChart = new Chart(ctx, {
                type: 'line',
                data: {
                    labels: portfolioData.map(d => d.day),
                    datasets: [{
                        label: 'Value',
                        data: portfolioData.map(d => d.value),
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
        })
        .catch(error => console.error('Error rendering historical portfolio chart:', error));
}

function renderCharts() {
    fetch('/api/hexjson')
        .then(response => {
            if (!response.ok) throw new Error(`HTTP error! status: ${response.status}`);
            return response.json();
        })
        .then(data => {
            const sortedData = [...data].sort((a, b) => a.currentDay - b.currentDay);
            const priceFilteredData = sortedData.filter(entry => entry.currentDay >= 1260);
            const latestData = sortedData[sortedData.length - 1] || {};

            // Update chart value displays
            document.getElementById('price-value').textContent = latestData.pricePulseX ? `$${latestData.pricePulseX.toFixed(4)}` : '$0.0000';
            document.getElementById('tshare-rate-value').textContent = latestData.tshareRateHEX ? `${formatWithCommas(latestData.tshareRateHEX.toFixed(2))} HEX` : '0.00 HEX';
            document.getElementById('payout-per-tshare-value').textContent = latestData.payoutPerTshareHEX ? `${formatWithCommas(latestData.payoutPerTshareHEX.toFixed(2))} HEX` : '0.00 HEX';
            document.getElementById('daily-payout-value').textContent = latestData.dailyPayoutHEX ? `${formatWithCommas(latestData.dailyPayoutHEX.toFixed(2))} HEX` : '0.00 HEX';

            const isDarkTheme = true;
            const chartConfigs = [
                { id: 'priceChart', label: 'HEX Price', field: 'pricePulseX', borderColor: isDarkTheme ? '#00b7eb' : '#007bff', data: priceFilteredData },
                { id: 'tshareRateChart', label: 'T-Share Rate', field: 'tshareRateHEX', borderColor: isDarkTheme ? '#00cc99' : '#28a745', data: sortedData },
                { id: 'payoutPerTshareChart', label: 'Payout Per T-Share', field: 'payoutPerTshareHEX', borderColor: isDarkTheme ? '#9966ff' : '#9900cc', data: sortedData },
                { id: 'dailyPayoutChart', label: 'Daily Payout', field: 'dailyPayoutHEX', borderColor: isDarkTheme ? '#ff6f61' : '#dc3545', data: sortedData }
            ];

            chartConfigs.forEach(config => {
                if (chartInstances[config.id]) chartInstances[config.id].destroy();
                const ctx = document.getElementById(config.id).getContext('2d');
                chartInstances[config.id] = new Chart(ctx, {
                    type: 'line',
                    data: {
                        labels: config.data.map(entry => entry.currentDay),
                        datasets: [{
                            label: config.label,
                            data: config.data.map(entry => entry[config.field]),
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
                        scales: {
                            x: { title: { display: true, text: 'Current Day', color: isDarkTheme ? '#ffffff' : '#000000' }, ticks: { color: isDarkTheme ? '#ffffff' : '#000000' } },
                            y: { title: { display: true, text: config.label, color: isDarkTheme ? '#ffffff' : '#000000' }, ticks: { color: isDarkTheme ? '#ffffff' : '#000000' } }
                        },
                        plugins: {
                            legend: { labels: { color: isDarkTheme ? '#ffffff' : '#000000' } }
                        }
                    }
                });
            });
        })
        .catch(error => {
            console.error('Error rendering charts:', error);
            document.querySelectorAll('.chart-container').forEach(c => c.innerHTML = `<p style="color: var(--text-color); text-align: center;">Error loading chart: ${error.message}</p>`);
        });
}

// =============================================
// SETTINGS & MINERS
// =============================================
function fetchSettings() {
    fetch('/api/config')
        .then(response => response.json())
        .then(config => {
            document.getElementById('frequency').value = config.liveDataFrequency;
            document.getElementById('liquid-hex').value = config.liquidHEX || '';
            document.getElementById('hist-start-day').value = config.historicalStartDay || 1260;
            historicalStartDay = config.historicalStartDay || 1260;
        });

    fetch('/api/miners')
        .then(response => response.json())
        .then(miners => {
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
            setupLiveDataInterval(frequency);
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
            fetchProfile();
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
            renderPortfolioHistoryChart();
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
            document.getElementById('start-date').value = '';
            document.getElementById('end-date').value = '';
            document.getElementById('tshares').value = '';
        } else {
            showNotification('Error adding miner', 'danger');
        }
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
                fetchSettings();
                fetchProfile();
            } else {
                showNotification('Error deleting miner', 'danger');
            }
        });
    }
}

// =============================================
// NAVBAR RESPONSIVENESS
// =============================================
function checkNavbarRows() {
    const navbarNav = document.querySelector('#navbarNav');
    const navItems = document.querySelectorAll('.navbar-nav .nav-item');
    const toggler = document.querySelector('.navbar-toggler');
    if (!navbarNav || !toggler || navItems.length === 0) return;

    const firstTop = navItems[0].getBoundingClientRect().top;
    const lastTop = navItems[navItems.length - 1].getBoundingClientRect().top;
    const isWrapping = Math.abs(lastTop - firstTop) > 10;

    if (isWrapping && window.innerWidth >= 576) {
        navbarNav.classList.add('collapse', 'navbar-collapse');
        toggler.style.display = 'block';
    } else if (window.innerWidth >= 576) {
        navbarNav.classList.remove('collapse', 'navbar-collapse');
        toggler.style.display = 'none';
    }
}

// =============================================
// INITIALIZATION
// =============================================
document.addEventListener('DOMContentLoaded', () => {
    // Initialize datepickers
    $('.datepicker').datepicker({
        format: 'dd-mm-yyyy',
        autoclose: true,
        todayHighlight: true
    });

    document.title = 'HEX Stats';

    // Initial data load
    fetchProfile();
    fetchLiveData();
    fetchSettings();           // Loads frequency + miners list
    renderCharts();

    // Periodic updates
    setInterval(fetchProfile, 30 * 60 * 1000);           // every 30 min
    setInterval(renderCharts, 4 * 60 * 60 * 1000);       // every 4 hours
    setInterval(renderPortfolioHistoryChart, 4 * 60 * 60 * 1000);

    // Navbar responsiveness
    window.addEventListener('resize', checkNavbarRows);
    checkNavbarRows();
});
