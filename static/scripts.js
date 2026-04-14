let chartInstances = {};
let userTotalTShares = 0;
let userLiquidHEX = 0;
let historicalStartDay = 1260;
let liveDataCache = null;

let liveDataIntervalId = null;
let countdownIntervalId = null;
let nextRefreshTime = Date.now();
let currentFrequency = 15;

// Notification helper
function showNotification(message, type = 'success') {
  const container = document.getElementById('notification-container');
  const notif = document.createElement('div');
  notif.className = `alert alert-${type} alert-dismissible fade show`;
  notif.innerHTML = `${message}<button type="button" class="btn-close" data-bs-dismiss="alert"></button>`;
  container.appendChild(notif);
  setTimeout(() => notif.remove(), 3500);
}

// Confirmation modal
function showConfirmModal(title, message) {
  return new Promise(resolve => {
    // ... (unchanged, identical to original)
  });
}

function formatWithCommas(num) {
  return num.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

// Unified live data fetch
async function fetchLiveData() {
  try {
    const res = await fetch('/api/live-data');
    if (!res.ok) throw new Error('Network error');
    const data = await res.json();
    liveDataCache = data;

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

    nextRefreshTime = Date.now() + currentFrequency * 60 * 1000;
    updateCountdown();
  } catch (e) {
    console.error(e);
    document.getElementById('last-updated').textContent = 'Error fetching live data';
  }
}

function updateCountdown() {
  const remaining = Math.max(0, nextRefreshTime - Date.now());
  const totalSec = Math.floor(remaining / 1000);
  const mins = Math.floor(totalSec / 60);
  const secs = totalSec % 60;
  document.getElementById('countdown').textContent = `${mins}:${secs.toString().padStart(2, '0')}`;
}

function setupLiveDataInterval(minutes) {
  if (liveDataIntervalId) clearInterval(liveDataIntervalId);
  if (countdownIntervalId) clearInterval(countdownIntervalId);

  currentFrequency = minutes;
  nextRefreshTime = Date.now() + minutes * 60 * 1000;

  countdownIntervalId = setInterval(updateCountdown, 1000);
  updateCountdown();

  liveDataIntervalId = setInterval(fetchLiveData, minutes * 60 * 1000);
}

// Optimized Profile fetch (Promise.all)
async function fetchProfile() {
  try {
    const [minersData, liveData, configData] = await Promise.all([
      fetch('/api/miners').then(r => r.json()),
      fetch('/api/live-data').then(r => r.json()),
      fetch('/api/config').then(r => r.json())
    ]);

    let totalTShares = 0;
    const activeMiners = [];
    const activeIndices = [];

    minersData.forEach((miner, i) => {
      if (miner.status !== 'completed') {
        totalTShares += miner.tShares;
        activeMiners.push(miner);
        activeIndices.push(i);
      }
    });

    userTotalTShares = totalTShares;
    document.getElementById('total-tshares').textContent = totalTShares.toFixed(2);

    // Use liveData from Promise.all
    const totalValue = totalTShares * liveData.tsharePrice_Pulsechain;
    document.getElementById('total-value').textContent = formatWithCommas(totalValue.toFixed(2));

    const dailyHEX = totalTShares * liveData.payoutPerTshare_Pulsechain;
    const dailyUSD = dailyHEX * liveData.price_Pulsechain;
    document.getElementById('interest-hex').textContent = formatWithCommas(dailyHEX.toFixed(2)) + ' HEX';
    document.getElementById('interest-usd').textContent = '$' + formatWithCommas(dailyUSD.toFixed(2));

    userLiquidHEX = configData.liquidHEX || 0;
    const liquidValue = userLiquidHEX * liveData.price_Pulsechain;
    document.getElementById('liquid-hex-value').textContent = formatWithCommas(liquidValue.toFixed(2));

    // Render active miners (unchanged logic)
    const container = document.getElementById('active-miners');
    container.innerHTML = activeMiners.length ? '' : '<p class="text-muted">No active miners. Add some in Settings.</p>';
    // ... rest of active miners rendering identical to original

    renderPortfolioHistoryChart();
  } catch (e) {
    console.error('Profile load failed', e);
  }
}

// Charts & Portfolio functions remain functionally identical but now benefit from the optimized profile fetch
// (renderCharts, renderPortfolioHistoryChart, saveFrequency, etc. unchanged except for minor cleanups)

document.addEventListener('DOMContentLoaded', () => {
  $('.datepicker').datepicker({ format: 'dd-mm-yyyy', autoclose: true });

  fetchProfile();
  fetchLiveData();
  fetch('/api/config').then(r => r.json()).then(cfg => {
    document.getElementById('frequency').value = cfg.liveDataFrequency;
    document.getElementById('liquid-hex').value = cfg.liquidHEX || 0;
    document.getElementById('hist-start-day').value = cfg.historicalStartDay || 1260;
    historicalStartDay = cfg.historicalStartDay || 1260;
    setupLiveDataInterval(cfg.liveDataFrequency);
  });

  renderCharts();

  // Periodic updates
  setInterval(fetchProfile, 30 * 60 * 1000);
  setInterval(renderCharts, 4 * 60 * 60 * 1000);
  setInterval(renderPortfolioHistoryChart, 4 * 60 * 60 * 1000);
});
