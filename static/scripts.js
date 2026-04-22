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
let liveDataCache = null;
let countdownIntervalId = null;
let nextRefreshTime = Date.now();
let currentFrequency = 15;
let wsConnection = null;

// =============================================
// PICO UI HANDLERS (Tabs, Modals, Notifications)
// =============================================

// Tab Switching Logic
document.addEventListener('DOMContentLoaded', () => {
    const tabLinks = document.querySelectorAll('.tab-link');
    const tabSections = document.querySelectorAll('.tab-section');

    tabLinks.forEach(link => {
        link.addEventListener('click', (e) => {
            e.preventDefault();
            
            // Remove active states
            tabLinks.forEach(l => l.classList.remove('active'));
            tabSections.forEach(s => s.style.display = 'none');
            
            // Set new active states
            link.classList.add('active');
            const targetId = link.getAttribute('data-target');
            document.getElementById(targetId).style.display = 'block';
        });
    });

    // Initialize Datepickers
    const startDateElem = document.getElementById('start-date');
    const endDateElem = document.getElementById('end-date');
    if (startDateElem) new Datepicker(startDateElem, { format: 'mm-dd-yyyy' });
    if (endDateElem) new Datepicker(endDateElem, { format: 'mm-dd-yyyy' });

    // Initial Data Load
    initialLoad();
});

// Notifications
function showNotification(message, type = 'success') {
    const container = document.getElementById('notification-container');
    const toast = document.createElement('article');
    toast.className = `notification-toast ${type === 'danger' ? 'danger' : ''}`;
    toast.innerHTML = `<strong>${type === 'success' ? '✅' : '❌'}</strong> ${message}`;
    
    container.appendChild(toast);

    setTimeout(() => {
        toast.style.opacity = '0';
        toast.style.transition = 'opacity 0.3s ease';
        setTimeout(() => toast.remove(), 300);
    }, 3000);
}

// Native <dialog> Modal for Confirmation
function showConfirmModal(title, message) {
    return new Promise((resolve) => {
        const dialog = document.createElement('dialog');
        dialog.innerHTML = `
            <article>
                <header>
                    <strong>${title}</strong>
                </header>
                <p>${message}</p>
                <footer>
                    <button class="secondary cancel-btn" style="width: auto; margin-right: 10px;">Cancel</button>
                    <button class="confirm-btn" style="width: auto;">Confirm</button>
                </footer>
            </article>
        `;
        document.body.appendChild(dialog);
        dialog.showModal();

        const cleanup = () => {
            dialog.close();
            dialog.remove();
        };

        dialog.querySelector('.confirm-btn').addEventListener('click', () => { cleanup(); resolve(true); });
        dialog.querySelector('.cancel-btn').addEventListener('click', () => { cleanup(); resolve(false); });
    });
}

function formatWithCommas(num) {
    return num.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

// =============================================
// LIVE DATA WEBSOCKET & TIMER
// =============================================

function updateLiveDataUI(data) {
    liveDataCache = data;

    document.getElementById('price').textContent = data.price_Pulsechain.toFixed(5);
    document.getElementById('tshare-price').textContent = data.tsharePrice_Pulsechain.toFixed(2);
    document.getElementById('tshare-rate').textContent = formatWithCommas(Math.floor(data.tshareRateHEX_Pulsechain));
    document.getElementById('payout').textContent = data.payoutPerTshare_Pulsechain.toFixed(1);
    document.getElementById('penalties').textContent = formatWithCommas(Math.floor(data.penaltiesHEX_Pulsechain));
    document.getElementById('beat').textContent = formatWithCommas(data.beat);

    const timestamp = new Date().toLocaleTimeString();
    document.getElementById('last-updated').textContent = `Last updated: ${timestamp}`;
    document.title = `HEX Stats - $${data.price_Pulsechain.toFixed(5)}`;

    nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
    updateCountdown();
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

    wsConnection.onopen = () => console.log('✅ WebSocket connected');
    wsConnection.onmessage = (event) => {
        try {
            updateLiveDataUI(JSON.parse(event.data));
        } catch (e) {
            console.error('WebSocket parse error', e);
        }
    };
    wsConnection.onclose = () => setTimeout(connectLiveWebSocket, 5000);
    wsConnection.onerror = (err) => console.error('WebSocket error', err);
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
// INITIAL LOAD & DOM POPULATION
// =============================================

async function initialLoad() {
    try {
        const [minersRes, liveRes, configRes, hexjsonRes] = await Promise.all([
            fetch('/api/miners'),
            fetch('/api/live-data'),
            fetch('/api/config'),
            fetch('/api/hexjson')
        ]);

        const miners = await minersRes.json();
        const liveData = await liveRes.json();
        const config = await configRes.json();
        const hexjsonData = await hexjsonRes.json();

        liveDataCache = liveData;
        updateLiveDataUI(liveData);

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

        const totalValue = totalTShares * liveData.tsharePrice_Pulsechain;
        document.getElementById('total-value').textContent = formatWithCommas(totalValue.toFixed(2));

        const dailyInterestHEX = totalTShares * liveData.payoutPerTshare_Pulsechain;
        const dailyInterestUSD = dailyInterestHEX * liveData.price_Pulsechain;
        document.getElementById('interest-hex').innerHTML = `<strong>${formatWithCommas(dailyInterestHEX.toFixed(2))} HEX</strong>`;
        document.getElementById('interest-usd').innerHTML = `<strong>$${formatWithCommas(dailyInterestUSD.toFixed(2))}</strong>`;

        userLiquidHEX = config.liquidHEX || 0;
        const liquidHEXValue = userLiquidHEX * liveData.price_Pulsechain;
        document.getElementById('liquid-hex-value').textContent = formatWithCommas(liquidHEXValue.toFixed(2));

        // Render active miners
        const activeMinersDiv = document.getElementById('active-miners');
        activeMinersDiv.innerHTML = '';
        if (activeMiners.length === 0) {
            document.getElementById('profile-message').textContent = 'Empty profile. Please add HEX miners in Settings.';
        } else {
            document.getElementById('profile-message').textContent = '';
            activeMiners.forEach((miner, index) => {
                const endDateObj = new Date(miner.endDate.split('-').reverse().join('-'));
                const isMatured = endDateObj <= new Date();
                const daysLeft = isMatured ? 0 : Math.ceil((endDateObj - new Date()) / (1000 * 60 * 60 * 24));

                const minerDiv = document.createElement('div');
                minerDiv.className = 'miner-item';
                minerDiv.innerHTML = `
                    <span>${miner.startDate} to ${miner.endDate}, T-Shares: ${miner.tShares.toFixed(2)} ${isMatured ? '(Matured)' : `(${daysLeft} days left)`}</span>
                    ${isMatured ? `<button class="secondary outline" onclick="endMiner(${minerIndices[index]})">End</button>` : ''}
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
                <button class="secondary outline" onclick="deleteMiner(${index})">Delete</button>
            `;
            existingMinersDiv.appendChild(minerDiv);
        });

        setupLiveDataInterval(config.liveDataFrequency);
        renderChartsWithData(hexjsonData);
        renderPortfolioHistoryChartWithData(hexjsonData);

    } catch (error) {
        console.error('Error during initial load:', error);
        showNotification('Failed to load initial data. Please refresh the page.', 'danger');
    }
}

// =============================================
// CHARTS
// =============================================

function renderChartsWithData(data) {
    const sortedData = [...data].sort((a, b) => a.currentDay - b.currentDay);
    const priceFilteredData = sortedData.filter(entry => entry.currentDay >= 1260);
    const latestData = sortedData[sortedData.length - 1] || {};

    document.getElementById('price-value').textContent = latestData.pricePulseX ? `$${latestData.pricePulseX.toFixed(4)}` : '$0.0000';
    document.getElementById('tshare-rate-value').textContent = latestData.tshareRateHEX ? `${formatWithCommas(latestData.tshareRateHEX.toFixed(2))} HEX` : '0.00 HEX';
    document.getElementById('payout-per-tshare-value').textContent = latestData.payoutPerTshareHEX ? `${formatWithCommas(latestData.payoutPerTshareHEX.toFixed(2))} HEX` : '0.00 HEX';
    document.getElementById('daily-payout-value').textContent = latestData.dailyPayoutHEX ? `${formatWithCommas(latestData.dailyPayoutHEX.toFixed(2))} HEX` : '0.00 HEX';

    const chartConfigs = [
        { id: 'priceChart', label: 'HEX Price', field: 'pricePulseX', borderColor: '#00b7eb', data: priceFilteredData },
        { id: 'tshareRateChart', label: 'T-Share Rate', field: 'tshareRateHEX', borderColor: '#00cc99', data: sortedData },
        { id: 'payoutPerTshareChart', label: 'Payout Per T-Share', field: 'payoutPerTshareHEX', borderColor: '#9966ff', data: sortedData },
        { id: 'dailyPayoutChart', label: 'Daily Payout', field: 'dailyPayoutHEX', borderColor: '#ff6f61', data: sortedData }
    ];

    // Read Pico's computed text color for Charts styling
    const textColor = getComputedStyle(document.documentElement).getPropertyValue('--pico-color').trim() || '#ffffff';

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
                    x: { title: { display: true, text: 'Current Day', color: textColor }, ticks: { color: textColor } },
                    y: { title: { display: true, text: config.label, color: textColor }, ticks: { color: textColor } }
                },
                plugins: {
                    legend: { labels: { color: textColor } }
                }
            }
        });
    });
}

function renderPortfolioHistoryChartWithData(rawData) {
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
    document.getElementById('hist-start-value').innerHTML = `<strong>$${formatWithCommas(startVal.toFixed(2))}</strong>`;
    document.getElementById('hist-current-value').innerHTML = `<strong>$${formatWithCommas(currVal.toFixed(2))}</strong>`;
    document.getElementById('hist-ath').innerHTML = `<strong>$${formatWithCommas(athVal.toFixed(2))}</strong>`;

    const growthEl = document.getElementById('hist-growth');
    growthEl.innerHTML = `<strong>${growth >= 0 ? '+' : ''}$${formatWithCommas(growth.toFixed(2))}</strong>`;
    growthEl.className = growth >= 0 ? 'text-success' : 'text-danger';

    const pctEl = document.getElementById('hist-growth-pct');
    pctEl.textContent = (growth >= 0 ? '+' : '') + growthPct.toFixed(2) + '%';
    pctEl.className = growth >= 0 ? 'text-success' : 'text-danger';

    const textColor = getComputedStyle(document.documentElement).getPropertyValue('--pico-color').trim() || '#ffffff';
    const bgColor = getComputedStyle(document.documentElement).getPropertyValue('--pico-background-color').trim() || '#11191f';

    if (chartInstances.historicalValueChart) chartInstances.historicalValueChart.destroy();

    const ctx = document.getElementById('historicalValueChart').getContext('2d');
    chartInstances.historicalValueChart = new Chart(ctx, {
        type: 'line',
        data: {
            labels: portfolioData.map(d => d.day),
            datasets: [{
                label: 'Value',
                data: portfolioData.map(d => d.value),
                borderColor: '#00b7eb',
                backgroundColor: 'rgba(0,183,235,0.2)',
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
                x: { title: { display: true, text: 'Day', color: textColor }, ticks: { color: textColor, maxTicksLimit: 15 } },
                y: { title: { display: true, text: 'Portfolio Value (USD)', color: textColor }, ticks: { color: textColor, callback: v => '$' + formatWithCommas(Math.round(v)) } }
            },
            plugins: {
                legend: { display: false },
                tooltip: {
                    displayColors: false,
                    backgroundColor: bgColor,
                    titleColor: textColor,
                    bodyColor: textColor,
                    borderColor: '#6c757d',
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

function renderCharts() {
    fetch('/api/hexjson').then(r => r.json()).then(renderChartsWithData).catch(console.error);
}

function renderPortfolioHistoryChart() {
    fetch('/api/hexjson').then(r => r.json()).then(renderPortfolioHistoryChartWithData).catch(console.error);
}

// =============================================
// MINERS AND SETTINGS ACTIONS
// =============================================

function showCompletedMiners() {
    fetch('/api/miners')
        .then(response => response.json())
        .then(miners => {
            const completedMiners = miners.filter(miner => miner.status === 'completed');
            const dialog = document.createElement('dialog');
            
            const minersHtml = completedMiners.length === 0 
                ? '<p>No completed miners.</p>' 
                : completedMiners.map(miner => `<p>${miner.startDate} to ${miner.endDate}, T-Shares: ${miner.tShares.toFixed(2)}</p>`).join('');

            dialog.innerHTML = `
                <article>
                    <header>
                        <button aria-label="Close" class="close" id="close-modal-x"></button>
                        <strong>Completed Miners</strong>
                    </header>
                    ${minersHtml}
                    <footer>
                        <button class="secondary cancel-btn" style="width: auto;">Close</button>
                    </footer>
                </article>
            `;
            document.body.appendChild(dialog);
            dialog.showModal();

            const cleanup = () => { dialog.close(); dialog.remove(); };
            
            dialog.querySelector('.cancel-btn').addEventListener('click', cleanup);
            dialog.querySelector('#close-modal-x').addEventListener('click', cleanup);
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
                initialLoad();   
            } else {
                showNotification('Error ending miner', 'danger');
            }
        });
    }
}

async function deleteMiner(index) {
    const confirmed = await showConfirmModal('Delete Miner', 'Are you sure you want to permanently delete this miner?');
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

function addMiner() {
    const startDate = document.getElementById('start-date').value;
    const endDate = document.getElementById('end-date').value;
    const tShares = parseFloat(document.getElementById('tshares').value);

    if (!startDate || !endDate || isNaN(tShares) || tShares <= 0) {
        showNotification('Please fill in all fields correctly.', 'danger');
        return;
    }

    const newMiner = {
        startDate: startDate,
        endDate: endDate,
        tShares: tShares,
        status: 'active'
    };

    fetch('/api/add-miner', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(newMiner)
    })
    .then(response => {
        if (response.ok) {
            showNotification('Miner added successfully!', 'success');
            document.getElementById('start-date').value = '';
            document.getElementById('end-date').value = '';
            document.getElementById('tshares').value = '';
            initialLoad(); 
        } else {
            showNotification('Failed to add miner.', 'danger');
        }
    })
    .catch(error => {
        console.error('Error:', error);
        showNotification('An error occurred.', 'danger');
    });
}

function saveFrequency() {
    const frequency = parseInt(document.getElementById('frequency').value, 10);
    if (isNaN(frequency) || frequency < 1) {
        showNotification('Please enter a valid frequency greater than 0.', 'danger');
        return;
    }

    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ liveDataFrequency: frequency })
    })
    .then(response => {
        if (response.ok) {
            showNotification('Update frequency saved.', 'success');
            setupLiveDataInterval(frequency);
        } else {
            showNotification('Failed to save frequency.', 'danger');
        }
    });
}

function saveLiquidHEX() {
    const liquidHex = parseFloat(document.getElementById('liquid-hex').value);
    if (isNaN(liquidHex) || liquidHex < 0) {
        showNotification('Please enter a valid amount.', 'danger');
        return;
    }

    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ liquidHEX: liquidHex })
    })
    .then(response => {
        if (response.ok) {
            showNotification('Liquid HEX saved.', 'success');
            initialLoad();
        } else {
            showNotification('Failed to save Liquid HEX.', 'danger');
        }
    });
}

function saveHistoricalStartDay() {
    const day = parseInt(document.getElementById('hist-start-day').value, 10);
    if (isNaN(day) || day < 1) {
        showNotification('Please enter a valid starting day.', 'danger');
        return;
    }

    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ historicalStartDay: day })
    })
    .then(response => {
        if (response.ok) {
            showNotification('Historical start day saved.', 'success');
            historicalStartDay = day;
            renderPortfolioHistoryChart(); 
        } else {
            showNotification('Failed to save historical start day.', 'danger');
        }
    });
}
