let chartInstance = null;

function formatWithCommas(num) {
    return num.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

function fetchLiveData() {
    fetch('/api/live-data')
        .then(response => response.json())
        .then(data => {
            document.getElementById('price').textContent = data.price_Pulsechain.toFixed(4);
            document.getElementById('tshare-price').textContent = data.tsharePrice_Pulsechain.toFixed(2);
            document.getElementById('tshare-rate').textContent = formatWithCommas(Math.floor(data.tshareRateHEX_Pulsechain));
            document.getElementById('payout').textContent = data.payoutPerTshare_Pulsechain.toFixed(1);
            document.getElementById('penalties').textContent = formatWithCommas(Math.floor(data.penaltiesHEX_Pulsechain));
            document.getElementById('beat').textContent = formatWithCommas(data.beat);
        })
        .catch(error => console.error('Error fetching live data:', error));
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
                    document.getElementById('total-value').textContent = (totalTShares * data.tsharePrice_Pulsechain).toFixed(2);
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

function endMiner(index) {
    if (confirm('Have you ended the mining contract and minted HEX?')) {
        fetch('/api/end-miner', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ index })
        })
            .then(response => {
                if (response.ok) {
                    fetchProfile();
                } else {
                    alert('Error ending miner');
                }
            });
    }
}

function updateChart() {
    const field = document.getElementById('chart-field').value;
    fetch('/api/hexjson')
        .then(response => response.json())
        .then(data => {
            if (chartInstance) {
                chartInstance.destroy();
            }
            const ctx = document.getElementById('hexChart').getContext('2d');
            chartInstance = new Chart(ctx, {
                type: 'line',
                data: {
                    labels: data.map(entry => entry.currentDay),
                    datasets: [{
                        label: field,
                        data: data.map(entry => entry[field]),
                        borderColor: 'blue',
                        fill: false
                    }]
                },
                options: {
                    scales: {
                        x: { title: { display: true, text: 'Current Day' } },
                        y: { title: { display: true, text: field } }
                    }
                }
            });
        });
}

function fetchSettings() {
    fetch('/api/config')
        .then(response => response.json())
        .then(config => {
            document.getElementById('frequency').value = config.liveDataFrequency;
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
        alert('Frequency must be a positive integer');
        return;
    }
    fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ liveDataFrequency: frequency })
    })
        .then(response => {
            if (response.ok) {
                alert(`Live data update frequency set to ${frequency} minutes`);
            } else {
                alert('Error saving frequency');
            }
        });
}

function addMiner() {
    const startDate = document.getElementById('start-date').value;
    const endDate = document.getElementById('end-date').value;
    const tShares = parseFloat(document.getElementById('tshares').value);
    if (!startDate || !endDate || isNaN(tShares) || tShares <= 0) {
        alert('Please fill all fields with valid data');
        return;
    }
    const dateRegex = /^\d{2}-\d{2}-\d{4}$/;
    if (!dateRegex.test(startDate) || !dateRegex.test(endDate)) {
        alert('Dates must be in DD-MM-YYYY format');
        return;
    }
    fetch('/api/add-miner', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ startDate, endDate, tShares })
    })
        .then(response => {
            if (response.ok) {
                fetchSettings();
                document.getElementById('start-date').value = '';
                document.getElementById('end-date').value = '';
                document.getElementById('tshares').value = '';
            } else {
                alert('Error adding miner');
            }
        });
}

function deleteMiner(index) {
    if (confirm('Do you want to delete this HEX miner?')) {
        fetch('/api/delete-miner', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ index })
        })
            .then(response => {
                if (response.ok) {
                    fetchSettings();
                } else {
                    alert('Error deleting miner');
                }
            });
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

    fetchProfile();
    fetchLiveData();
    fetchSettings();
    updateChart();
    setInterval(fetchLiveData, 60000); // Update live data every minute
    setInterval(fetchProfile, 60000); // Update profile every minute
});
