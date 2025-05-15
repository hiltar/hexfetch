let chartInstances = {
    priceChart: null,
    tshareRateChart: null,
    payoutPerTshareChart: null,
    dailyPayoutChart: null,
    cpuUsageChart: null,
    memoryUsageChart: null,
    diskUsageChart: null
};

let systemMetricsHistory = [];

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

function setTheme(theme) {
    document.documentElement.setAttribute('data-theme', theme);
    localStorage.setItem('theme', theme);
    const toggle = document.getElementById('themeToggle');
    const icon = document.getElementById('themeIcon');
    toggle.checked = theme === 'dark';
    icon.classList.remove('bi-sun-fill', 'bi-moon-fill');
    icon.classList.add(theme === 'dark' ? 'bi-moon-fill' : 'bi-sun-fill');
    renderCharts();
    renderSystemCharts();
}

function toggleTheme() {
    const currentTheme = document.documentElement.getAttribute('data-theme') || 'light';
    setTheme(currentTheme === 'light' ? 'dark' : 'light');
}

function initTheme() {
    const savedTheme = localStorage.getItem('theme');
    if (savedTheme) {
        setTheme(savedTheme);
    } else {
        const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
        setTheme(prefersDark ? 'dark' : 'light');
    }
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
            const timestamp = new Date().toLocaleTimeString();
            document.getElementById('last-updated').textContent = `Last updated: ${timestamp}`;
            document.title = `HEX Stats - $${data.price_Pulsechain.toFixed(5)}`;
            console.log(`Live data updated at ${timestamp}, title set to: ${document.title}`);
        })
        .catch(error => {
            console.error('Error fetching live data:', error);
            document.getElementById('last-updated').textContent = `Error updating data: ${error.message}`;
            document.title = 'HEX Stats';
        });
}

function fetchProfile() {
    fetch('/api/miners')
        .then(response => response.json())
        .then(miners => {
            let totalTShares = 0;
            let activeMiners = [];
            miners.forEach(miner => {
                if (miner.status !== 'completed') {
                    totalTShares += miner.tShares;
                    activeMiners.push(miner);
                }
            });
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
                    const liquidHEX = config.liquidHEX || 0;
                    fetch('/api/live-data')
                        .then(response => response.json())
                        .then(data => {
                            const liquidHEXValue = liquidHEX * data.price_Pulsechain;
                            document.getElementById('liquid-hex-value').textContent = formatWithCommas(liquidHEXValue.toFixed(2));
                        });
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
                    ${isMatured ? `<button class="btn btn-sm btn-danger" onclick="endMiner(${index})">End</button>` : ''}
                `;
                activeMinersDiv.appendChild(minerDiv);
            });
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

function fetchSystemMetrics() {
    fetch('/api/system')
        .then(response => {
            if (!response.ok) {
                throw new Error(`HTTP error! status: ${response.status}`);
            }
            return response.json();
        })
        .then(data => {
            systemMetricsHistory.push(data);
            if (systemMetricsHistory.length > 60) {
                systemMetricsHistory.shift();
            }
            renderSystemCharts();
        })
        .catch(error => {
            console.error('Error fetching system metrics:', error);
            showNotification('Failed to load system metrics', 'danger');
        });
}

function renderSystemCharts() {
    if (systemMetricsHistory.length === 0) {
        return;
    }

    const isDarkTheme = document.documentElement.getAttribute('data-theme') === 'dark';
    const latestMetrics = systemMetricsHistory[systemMetricsHistory.length - 1];

    document.getElementById('cpu-usage-value').textContent = `${latestMetrics.cpuUsagePercent.toFixed(2)}%`;
    document.getElementById('memory-usage-value').textContent = `${latestMetrics.memoryUsedPercent.toFixed(2)}% (${latestMetrics.memoryUsedGB.toFixed(2)} GB / ${latestMetrics.memoryTotalGB.toFixed(2)} GB)`;
    document.getElementById('disk-usage-value').textContent = `${latestMetrics.diskUsedPercent.toFixed(2)}% (${latestMetrics.diskUsedGB.toFixed(2)} GB / ${latestMetrics.diskTotalGB.toFixed(2)} GB)`;

    const chartConfigs = [
        {
            id: 'cpuUsageChart',
            label: 'CPU Usage (%)',
            field: 'cpuUsagePercent',
            borderColor: isDarkTheme ? '#00b7eb' : '#007bff',
            yAxisLabel: 'Percentage (%)'
        },
        {
            id: 'memoryUsageChart',
            label: 'Memory Usage (%)',
            field: 'memoryUsedPercent',
            borderColor: isDarkTheme ? '#00cc99' : '#28a745',
            yAxisLabel: 'Percentage (%)'
        },
        {
            id: 'diskUsageChart',
            label: 'Disk Usage (%)',
            field: 'diskUsedPercent',
            borderColor: isDarkTheme ? '#9966ff' : '#9900cc',
            yAxisLabel: 'Percentage (%)'
        }
    ];

    chartConfigs.forEach(config => {
        if (chartInstances[config.id]) {
            chartInstances[config.id].destroy();
        }
        const ctx = document.getElementById(config.id).getContext('2d');
        const datasets = config.datasets || [{
            label: config.label,
            data: systemMetricsHistory.map(entry => entry[config.field]),
            borderColor: config.borderColor,
            fill: false,
            pointRadius: 3,
            tension: 0.1
        }];

        chartInstances[config.id] = new Chart(ctx, {
            type: 'line',
            data: {
                labels: systemMetricsHistory.map(entry => new Date(entry.timestamp * 1000).toLocaleTimeString()),
                datasets: datasets
            },
            options: {
                responsive: true,
                maintainAspectRatio: false,
                scales: {
                    x: {
                        title: {
                            display: true,
                            text: 'Time',
                            color: isDarkTheme ? '#ffffff' : '#000000'
                        },
                        ticks: {
                            color: isDarkTheme ? '#ffffff' : '#000000'
                        }
                    },
                    y: {
                        title: {
                            display: true,
                            text: config.yAxisLabel,
                            color: isDarkTheme ? '#ffffff' : '#000000'
                        },
                        ticks: {
                            color: isDarkTheme ? '#ffffff' : '#000000'
                        },
                        beginAtZero: true
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
            if (!Array.isArray(data) || data.length === 0) {
                throw new Error('No chart data available');
            }
            const sortedData = [...data].sort((a, b) => a.currentDay - b.currentDay);
            const latestData = sortedData[sortedData.length - 1];
            if (!latestData || typeof latestData !== 'object') {
                throw new Error('Invalid latest chart data');
            }
            document.getElementById('price-value').textContent = (latestData.pricePulseX != null && !isNaN(latestData.pricePulseX))
                ? `$${latestData.pricePulseX.toFixed(4)}`
                : '$0.0000';
            document.getElementById('tshare-rate-value').textContent = (latestData.tshareRateHEX != null && !isNaN(latestData.tshareRateHEX))
                ? `${formatWithCommas(Number(latestData.tshareRateHEX).toFixed(2))} HEX`
                : '0.00 HEX';
            document.getElementById('payout-per-tshare-value').textContent = (latestData.payoutPerTshareHEX != null && !isNaN(latestData.payoutPerTshareHEX))
                ? `${formatWithCommas(Number(latestData.payoutPerTshareHEX).toFixed(2))} HEX`
                : '0.00 HEX';
            document.getElementById('daily-payout-value').textContent = (latestData.dailyPayoutHEX != null && !isNaN(latestData.dailyPayoutHEX))
                ? `${formatWithCommas(Number(latestData.dailyPayoutHEX).toFixed(2))} HEX`
                : '0.00 HEX';
            const priceFilteredData = sortedData.filter(entry => entry.currentDay >= 1260);
            const isDarkTheme = document.documentElement.getAttribute('data-theme') === 'dark';
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
            renderSystemCharts();
        })
        .catch(error => {
            console.error('Error rendering charts:', error);
            const chartContainers = document.querySelectorAll('.chart-container');
            chartContainers.forEach(container => {
                container.innerHTML = `<p style="color: var(--text-color); text-align: center;">Error loading chart: ${error.message}</p>`;
            });
            document.getElementById('price-value').textContent = '$0.0000';
            document.getElementById('tshare-rate-value').textContent = '0.00 HEX';
            document.getElementById('payout-per-tshare-value').textContent = '0.00 HEX';
            document.getElementById('daily-payout-value').textContent = '0.00 HEX';
            document.getElementById('cpu-usage-value').textContent = '0.00%';
            document.getElementById('memory-usage-value').textContent = '0.00% (0.00 GB / 0.00 GB)';
            document.getElementById('disk-usage-value').textContent = '0.00% (0.00 GB / 0.00 GB)';
            showNotification('Failed to load chart data', 'danger');
        });
}

function fetchSettings() {
    fetch('/api/config')
        .then(response => response.json())
        .then(config => {
            document.getElementById('frequency').value = config.liveDataFrequency;
            document.getElementById('liquid-hex').value = config.liquidHEX || '';
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
    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ liveDataFrequency: frequency, liquidHEX: parseFloat(document.getElementById('liquid-hex').value) || 0 })
    })
        .then(response => {
            if (response.ok) {
                showNotification(`Live data update frequency set to ${frequency} minutes`, 'success');
            } else {
                showNotification('Error saving frequency', 'danger');
            }
        });
}

function saveLiquidHEX() {
    const frequency = parseInt(document.getElementById('frequency').value);
    const liquidHEX = parseFloat(document.getElementById('liquid-hex').value);
    if (isNaN(liquidHEX) || liquidHEX < 0) {
        showNotification('Liquid HEX must be a non-negative number', 'danger');
        return;
    }
    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ liveDataFrequency: frequency, liquidHEX })
    })
        .then(response => {
            if (response.ok) {
                showNotification('Liquid HEX saved successfully', 'success');
                fetchProfile();
            } else {
                showNotification('Error saving Liquid HEX', 'danger');
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
                } else {
                    showNotification('Error deleting miner', 'danger');
                }
            });
    }
}

function checkNavbarRows() {
    const navbarNav = document.querySelector('#navbarNav');
    const navItems = document.querySelectorAll('.navbar-nav .nav-item');
    const toggler = document.querySelector('.navbar-toggler');
    if (!navbarNav || !toggler || navItems.length === 0) return;
    const firstItem = navItems[0];
    const lastItem = navItems[navItems.length - 1];
    const firstTop = firstItem.getBoundingClientRect().top;
    const lastTop = lastItem.getBoundingClientRect().top;
    const isWrapping = Math.abs(lastTop - firstTop) > 10;
    if (isWrapping && window.innerWidth >= 576) {
        navbarNav.classList.add('collapse', 'navbar-collapse');
        toggler.style.display = 'block';
    } else if (window.innerWidth >= 576) {
        navbarNav.classList.remove('collapse', 'navbar-collapse');
        toggler.style.display = 'none';
    }
}

document.addEventListener('DOMContentLoaded', () => {
    $('.datepicker').datepicker({
        format: 'dd-mm-yyyy',
        autoclose: true,
        todayHighlight: true
    });
    initTheme();
    document.title = 'HEX Stats';
    fetchProfile();
    fetchLiveData();
    fetchSettings();
    fetchSystemMetrics();
    renderCharts();
    checkNavbarRows();
    setInterval(() => {
        console.log('Attempting to fetch live data...');
        fetchLiveData();
    }, 60000);
    setInterval(fetchProfile, 60000);
    setInterval(() => {
        console.log('Attempting to fetch system metrics...');
        fetchSystemMetrics();
    }, 60000);
    setInterval(() => {
        console.log('Attempting to update charts...');
        renderCharts();
    }, 86400000);
    window.addEventListener('resize', checkNavbarRows);
});
