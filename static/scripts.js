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

// Live data timer variables
let liveDataIntervalId = null;
let countdownIntervalId = null;
let nextRefreshTime = Date.now();
let currentFrequency = 15;

// Show custom notification
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

    // Auto-dismiss after 3 seconds
    setTimeout(() => {
        notification.classList.remove('show');
        notification.classList.add('fade');
        setTimeout(() => notification.remove(), 150); // Wait for fade animation
    }, 3000);
}

// Show custom confirmation modal
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

        confirmBtn.addEventListener('click', () => {
            cleanup();
            resolve(true);
        });

        cancelBtn.addEventListener('click', () => {
            cleanup();
            resolve(false);
        });

        closeBtn.addEventListener('click', () => {
            cleanup();
            resolve(false);
        });

        modal.addEventListener('hidden.bs.modal', () => {
            cleanup();
            resolve(false);
        });
    });
}

function formatWithCommas(num) {
    return num.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}


function fetchLiveData() {
    fetch('/api/live-data')
        .then(response => {
            if (!response.ok) {
                throw new Error(`HTTP error! status: ${response.status}`);
            }
            return response.json();
        })
        .then(data => {
            document.getElementById('price').textContent = data.price_Pulsechain.toFixed(5);
            document.getElementById('tshare-price').textContent = data.tsharePrice_Pulsechain.toFixed(2);
            document.getElementById('tshare-rate').textContent = formatWithCommas(Math.floor(data.tshareRateHEX_Pulsechain));
            document.getElementById('payout').textContent = data.payoutPerTshare_Pulsechain.toFixed(1);
            document.getElementById('penalties').textContent = formatWithCommas(Math.floor(data.penaltiesHEX_Pulsechain));
            document.getElementById('beat').textContent = formatWithCommas(data.beat);
            // Update timestamp to confirm refresh
            const timestamp = new Date().toLocaleTimeString();
            document.getElementById('last-updated').textContent = `Last updated: ${timestamp}`;
            // Update document title with price
            document.title = `HEX Stats - $${data.price_Pulsechain.toFixed(5)}`;
            console.log(`Live data updated at ${timestamp}, title set to: ${document.title}`);
            // Reset countdown on successful fetch
            nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
            updateCountdown();
        })
        .catch(error => {
            console.error('Error fetching live data:', error);
            document.getElementById('last-updated').textContent = `Error updating data: ${error.message}`;
            // Revert title on error
            document.title = 'HEX Stats';
        });
}

function updateCountdown() {
    const remainingMs = Math.max(0, nextRefreshTime - Date.now());
    const totalSec = Math.floor(remainingMs / 1000);
    const mins = Math.floor(totalSec / 60);
    const secs = totalSec % 60;
    document.getElementById('countdown').textContent = `${mins}:${secs.toString().padStart(2, '0')}`;
}

// Set up live data interval based on frequency (in minutes)
function setupLiveDataInterval(frequencyInMinutes) {
    // Clear existing interval if it exists
    if (liveDataIntervalId) {
        clearInterval(liveDataIntervalId);
    }
    if (countdownIntervalId) {
        clearInterval(countdownIntervalId);
    }
    // Convert frequency from minutes to milliseconds
    const intervalMs = frequencyInMinutes * 60 * 1000;
    currentFrequency = frequencyInMinutes;
    nextRefreshTime = Date.now() + intervalMs;
    // Set countdown timer
    countdownIntervalId = setInterval(updateCountdown, 1000);
    updateCountdown();
    // Set new interval
    liveDataIntervalId = setInterval(() => {
        console.log(`Attempting to fetch live data every ${frequencyInMinutes} minute(s)...`);
        fetchLiveData();
    }, intervalMs);
}

function renderPortfolioHistoryChart() {
    fetch('/api/hexjson')
        .then(response => {
            if (!response.ok) {
                throw new Error(`HTTP error! status: ${response.status}`);
            }
            return response.json();
        })
        .then(rawData => {
            // Sort data by currentDay ascending (earliest to latest)
            const sortedData = [...rawData].sort((a, b) => a.currentDay - b.currentDay);
            // Filter from user selected start day
            const filteredData = sortedData.filter(entry => entry.currentDay >= historicalStartDay);
            if (filteredData.length === 0) {
                console.warn('No historical data available for the selected starting day');
                return;
            }

            // Calculate combined portfolio value (T-Shares + Liquid HEX) for every day
            const portfolioData = filteredData.map(entry => {
                const hexPrice = entry.pricePulseX || 0;
                const tsharePrice = entry.tshareRateHEX * hexPrice;
                const portfolioValue = (userTotalTShares * tsharePrice) + (userLiquidHEX * hexPrice);
                return {
                    day: entry.currentDay,
                    value: portfolioValue
                };
            });

            // Calculate stats
            const startVal = portfolioData[0].value;
            const currVal = portfolioData[portfolioData.length - 1].value;
            const athVal = Math.max(...portfolioData.map(d => d.value));
            const growth = currVal - startVal;
            const growthPct = startVal > 0 ? (growth / startVal) * 100 : 0;

            // Update stats UI
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

            // Destroy previous chart instance
            if (chartInstances.historicalValueChart) {
                chartInstances.historicalValueChart.destroy();
            }

            const ctx = document.getElementById('historicalValueChart').getContext('2d');
            chartInstances.historicalValueChart = new Chart(ctx, {
                type: 'line',
                data: {
                    labels: portfolioData.map(d => d.day),
                    datasets: [{
                        label: 'Portfolio Value (USD)',
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
                    interaction: {
                        intersect: false,
                        mode: 'index'
                    },
                    scales: {
                        x: {
                            title: {
                                display: true,
                                text: 'PulseChain Day',
                                color: isDarkTheme ? '#ffffff' : '#000000'
                            },
                            ticks: {
                                color: isDarkTheme ? '#ffffff' : '#000000',
                                maxTicksLimit: 15
                            }
                        },
                        y: {
                            title: {
                                display: true,
                                text: 'Portfolio Value (USD)',
                                color: isDarkTheme ? '#ffffff' : '#000000'
                            },
                            ticks: {
                                color: isDarkTheme ? '#ffffff' : '#000000',
                                callback: function(value) {
                                    return '$' + formatWithCommas(Math.round(value));
                                }
                            }
                        }
                    },
                    plugins: {
                        legend: {
                            display: false
                        },
                        tooltip: {
                            displayColors: false,
                            backgroundColor: isDarkTheme ? '#343a40' : '#ffffff',
                            titleColor: isDarkTheme ? '#ffffff' : '#000000',
                            bodyColor: isDarkTheme ? '#ffffff' : '#000000',
                            borderColor: isDarkTheme ? '#6c757d' : '#dee2e6',
                            borderWidth: 1,
                            callbacks: {
                                title: function(tooltipItems) {
                                    return 'Day ' + tooltipItems[0].label;
                                },
                                label: function(context) {
                                    return 'Portfolio Value: $' + formatWithCommas(context.raw.toFixed(2));
                                }
                            }
                        }
                    }
                }
            });
        })
        .catch(error => {
            console.error('Error rendering historical portfolio chart:', error);
        });
}

function fetchProfile() {
    fetch('/api/miners')
        .then(response => response.json())
        .then(miners => {
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

            fetch('/api/live-data')
                .then(response => response.json())
                .then(data => {
                    const totalValue = totalTShares * data.tsharePrice_Pulsechain;
                    document.getElementById('total-value').textContent = formatWithCommas(totalValue.toFixed(2));
                });

            fetch('/api/config')
                .then(response => response.json())
                .then(config => {
                    userLiquidHEX = config.liquidHEX || 0;
                    fetch('/api/live-data')
                        .then(response => response.json())
                        .then(data => {
                            const liquidHEXValue = userLiquidHEX * data.price_Pulsechain;
                            document.getElementById('liquid-hex-value').textContent = formatWithCommas(liquidHEXValue.toFixed(2));
                        });
                    renderPortfolioHistoryChart();
                });

            const activeMinersDiv = document.getElementById('active-miners');
            activeMinersDiv.innerHTML = '';
            if (activeMiners.length === 0) {
                document.getElementById('profile-message').textContent = 'Empty profile. Please add HEX miners in Settings.';
                return;
            }
            document.getElementById('profile-message').textContent = '';
            activeMiners.forEach((miner, index) => {
                const isMatured = new Date(miner.endDate.split('-').reverse().join('-')) <= new Date();
                const daysLeft = isMatured ? 0 : Math.ceil((new Date(miner.endDate.split('-').reverse().join('-')) - new Date()) / (1000 * 60 * 60 * 24));
                const minerDiv = document.createElement('div');
                minerDiv.className = 'miner-item';
                minerDiv.innerHTML = `
                    <span>${miner.startDate} to ${miner.endDate}, T-Shares: ${miner.tShares.toFixed(2)} ${isMatured ? '(Matured)' : `(${daysLeft} days left)`}</span>
                    ${isMatured ? `<button class="btn btn-sm btn-danger" onclick="endMiner(${minerIndices[index]})">End</button>` : ''}
                `;
                activeMinersDiv.appendChild(minerDiv);
            });
            renderPortfolioHistoryChart();
        })
        .catch(error => {
            console.error('Error fetching profile:', error);
        });
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
    const confirmed = await showConfirmModal(
        'End Miner',
        'Have you ended the mining contract and minted HEX?'
    );
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

function renderCharts() {
    fetch('/api/hexjson')
        .then(response => {
            if (!response.ok) {
                throw new Error(`HTTP error! status: ${response.status}`);
            }
            return response.json();
        })
        .then(data => {
            // Sort data by currentDay ascending (earliest to latest)
            const sortedData = [...data].sort((a, b) => a.currentDay - b.currentDay);
            // Filter data for Price PulseX to start at currentDay 1260
            const priceFilteredData = sortedData.filter(entry => entry.currentDay >= 1260);

            // Get the latest values from sortedData (last entry)
            const latestData = sortedData[sortedData.length - 1] || {};

            // Update chart value displays
            document.getElementById('price-value').textContent = latestData.pricePulseX
                ? `$${latestData.pricePulseX.toFixed(4)}`
                : '$0.0000';
            document.getElementById('tshare-rate-value').textContent = latestData.tshareRateHEX
                ? `${formatWithCommas(latestData.tshareRateHEX.toFixed(2))} HEX`
                : '0.00 HEX';
            document.getElementById('payout-per-tshare-value').textContent = latestData.payoutPerTshareHEX
                ? `${formatWithCommas(latestData.payoutPerTshareHEX.toFixed(2))} HEX`
                : '0.00 HEX';
            document.getElementById('daily-payout-value').textContent = latestData.dailyPayoutHEX
                ? `${formatWithCommas(latestData.dailyPayoutHEX.toFixed(2))} HEX`
                : '0.00 HEX';

            const isDarkTheme = true;
            const chartConfigs = [
                {
                    id: 'priceChart',
                    label: 'HEX Price',
                    field: 'pricePulseX',
                    borderColor: isDarkTheme ? '#00b7eb' : '#007bff',
                    data: priceFilteredData
                },
                {
                    id: 'tshareRateChart',
                    label: 'T-Share Rate',
                    field: 'tshareRateHEX',
                    borderColor: isDarkTheme ? '#00cc99' : '#28a745',
                    data: sortedData
                },
                {
                    id: 'payoutPerTshareChart',
                    label: 'Payout Per T-Share',
                    field: 'payoutPerTshareHEX',
                    borderColor: isDarkTheme ? '#9966ff' : '#9900cc',
                    data: sortedData
                },
                {
                    id: 'dailyPayoutChart',
                    label: 'Daily Payout',
                    field: 'dailyPayoutHEX',
                    borderColor: isDarkTheme ? '#ff6f61' : '#dc3545',
                    data: sortedData
                }
            ];

            chartConfigs.forEach(config => {
                if (chartInstances[config.id]) {
                    chartInstances[config.id].destroy();
                }
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
                            pointRadius: 3,
                            tension: 0.1
                        }]
                    },
                    options: {
                        responsive: true,
                        maintainAspectRatio: false,
                        scales: {
                            x: {
                                title: {
                                    display: true,
                                    text: 'Current Day',
                                    color: isDarkTheme ? '#ffffff' : '#000000'
                                },
                                ticks: {
                                    color: isDarkTheme ? '#ffffff' : '#000000'
                                }
                            },
                            y: {
                                title: {
                                    display: true,
                                    text: config.label,
                                    color: isDarkTheme ? '#ffffff' : '#000000'
                                },
                                ticks: {
                                    color: isDarkTheme ? '#ffffff' : '#000000'
                                }
                            }
                        },
                        plugins: {
                            legend: {
                                labels: {
                                    color: isDarkTheme ? '#ffffff' : '#000000'
                                }
                            }
                        }
                    }
                });
            });
            console.log('Charts rendered: HEX Price from day 1260, others from earliest day');
        })
        .catch(error => {
            console.error('Error rendering charts:', error);
            // Display error in UI
            const chartContainers = document.querySelectorAll('.chart-container');
            chartContainers.forEach(container => {
                container.innerHTML = `<p style="color: var(--text-color); text-align: center;">Error loading chart: ${error.message}</p>`;
            });
            // Set default values on error
            document.getElementById('price-value').textContent = '$0.0000';
            document.getElementById('tshare-rate-value').textContent = '0.00 HEX';
            document.getElementById('payout-per-tshare-value').textContent = '0.00 HEX';
            document.getElementById('daily-payout-value').textContent = '0.00 HEX';
        });
}

function fetchSettings() {
    fetch('/api/config')
        .then(response => response.json())
        .then(config => {
            document.getElementById('frequency').value = config.liveDataFrequency;
            document.getElementById('liquid-hex').value = config.liquidHEX || '';
            document.getElementById('hist-start-day').value = config.historicalStartDay || 1260;
            historicalStartDay = config.historicalStartDay || 1260;
            // Set up live data interval with the fetched frequency
            setupLiveDataInterval(config.liveDataFrequency);
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
                // Update the live data interval
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
                fetchProfile(); // Refresh Profile tab
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
                } else {
                    showNotification('Error deleting miner', 'danger');
                }
            });
    }
}

// Navbar row detection
function checkNavbarRows() {
    const navbarNav = document.querySelector('#navbarNav');
    const navItems = document.querySelectorAll('.navbar-nav .nav-item');
    const toggler = document.querySelector('.navbar-toggler');
    if (!navbarNav || !toggler || navItems.length === 0) return;

    // Get the top position of the first and last nav items
    const firstItem = navItems[0];
    const lastItem = navItems[navItems.length - 1];
    const firstTop = firstItem.getBoundingClientRect().top;
    const lastTop = lastItem.getBoundingClientRect().top;

    // If tops differ significantly, nav bar is wrapping
    const isWrapping = Math.abs(lastTop - firstTop) > 10;

    // Toggle navbar classes
    if (isWrapping && window.innerWidth >= 576) {
        navbarNav.classList.add('collapse', 'navbar-collapse');
        toggler.style.display = 'block';
    } else if (window.innerWidth >= 576) {
        navbarNav.classList.remove('collapse', 'navbar-collapse');
        toggler.style.display = 'none';
    }
}

// Initialize
document.addEventListener('DOMContentLoaded', () => {
    // Initialize datepickers
    $('.datepicker').datepicker({
        format: 'dd-mm-yyyy',
        autoclose: true,
        todayHighlight: true
    });

    // Set initial document title
    document.title = 'HEX Stats';

    // Initial fetches
    fetchProfile();
    fetchLiveData(); // Initial fetch
    fetchSettings(); // This will set up the live data interval
    renderCharts();
    checkNavbarRows();

    // Periodic updates
    setInterval(fetchProfile, 600000); // Update profile every 10 minutes
    setInterval(() => {
        console.log('Attempting to update charts...');
        renderCharts();
    }, 86400000); // Update charts every day
    setInterval(renderPortfolioHistoryChart, 86400000); // Update portfolio chart every day

    // Navbar row detection on resize
    window.addEventListener('resize', checkNavbarRows);
});
