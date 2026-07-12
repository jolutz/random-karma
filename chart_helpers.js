// Chart.js bridge used by the Rust/WASM application.
const CONFIG = {
	MAX_RETRY_ATTEMPTS: 20,
	RETRY_DELAY_MS: 50,
	FONT: 'Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif',
};

let chart = null;
let currentLapCount = null;
let currentPlayerCount = null;
let resizeHandler = null;
let themeWatcher = null;
let pendingUpdate = 0;

function ready(callback, tries = 0) {
	if (typeof window.Chart !== "undefined") callback(window.Chart);
	else if (tries < CONFIG.MAX_RETRY_ATTEMPTS) setTimeout(() => ready(callback, tries + 1), CONFIG.RETRY_DELAY_MS);
	else console.error("Chart.js did not load in time.");
}

function cssColor(name, fallback) {
	const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
	return value || fallback;
}

function theme() {
	return {
		primary: cssColor('--primary', '#6d5dfc'),
		danger: cssColor('--danger', '#dc4c64'),
		grid: cssColor('--grid', 'rgba(80,94,121,.13)'),
		text: cssColor('--text-muted', '#667085'),
		surface: cssColor('--surface-solid', '#fff'),
		tooltip: cssColor('--text', '#172033'),
	};
}

function formatMsToMinSec(ms) {
	const totalSeconds = Math.floor(ms / 1000);
	return `${Math.floor(totalSeconds / 60)}m ${totalSeconds % 60}s`;
}

function formatMsCompact(ms) {
	const seconds = Math.floor(ms / 1000);
	return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

function scheduleUpdate() {
	if (!chart || pendingUpdate) return;
	pendingUpdate = requestAnimationFrame(() => {
		pendingUpdate = 0;
		chart?.update('none');
	});
}

function syncSliderWithChart() {
	const canvas = document.getElementById('similarityChart');
	const slider = document.querySelector('.target-slider-container input[type="range"]');
	if (canvas && slider) slider.style.width = `${Math.max(0, canvas.getBoundingClientRect().width)}px`;
}

function applyTheme() {
	if (!chart) return;
	const colors = theme();
	const main = chart.data.datasets[0];
	main.borderColor = colors.primary;
	main.backgroundColor = colors.primary;
	main.pointBackgroundColor = colors.surface;
	const failed = chart.data.datasets.find(dataset => dataset.label === 'Failed');
	if (failed) failed.backgroundColor = failed.borderColor = colors.danger;
	chart.options.scales.x.grid.color = colors.grid;
	chart.options.scales.x.ticks.color = colors.text;
	chart.options.scales.y.grid.color = colors.grid;
	chart.options.plugins.tooltip.backgroundColor = colors.tooltip;
	scheduleUpdate();
}

function createChartConfig(min, max) {
	const colors = theme();
	return {
		type: 'line',
		data: { datasets: [{
			label: 'Jaccard similarity', data: [], borderColor: colors.primary,
			backgroundColor: colors.primary, pointBackgroundColor: colors.surface,
			pointBorderColor: colors.primary, pointHoverBorderColor: colors.primary,
			pointHoverBackgroundColor: colors.surface, borderWidth: 2.5,
			pointBorderWidth: 2, pointHoverBorderWidth: 2,
			pointRadius: 3, pointHoverRadius: 6, pointHitRadius: 10, tension: .32, fill: false,
		}] },
		options: {
			animation: false, responsive: true, maintainAspectRatio: false, normalized: true,
			parsing: false, spanGaps: true, interaction: { mode: 'nearest', intersect: false },
			onClick: event => {
				const activePoint = chart?.getActiveElements()[0];
				const point = activePoint
					|| chart?.getElementsAtEventForMode(event, 'nearest', { intersect: false }, false)?.[0];
				if (!point) return;
				const target = chart.data.datasets[point.datasetIndex].data[point.index]?.x;
				const slider = document.querySelector('.target-slider-container input[type="range"]');
				if (slider && target != null) {
					slider.value = target;
					slider.dispatchEvent(new Event('input', { bubbles: true }));
				}
			},
			layout: { padding: { top: 30, right: 10, bottom: 4, left: 8 } },
			scales: {
				x: {
					type: 'linear', min, max, border: { display: false },
					grid: { color: colors.grid, tickLength: 0 },
					ticks: { color: colors.text, font: { family: CONFIG.FONT, size: 11 }, maxRotation: 0, maxTicksLimit: 7, padding: 10, callback: formatMsCompact },
				},
				y: {
					min: 0, max: 100, border: { display: false }, grid: { color: colors.grid, tickLength: 0 },
					ticks: { color: colors.text, font: { family: CONFIG.FONT, size: 10 }, maxTicksLimit: 5, padding: 8, callback: value => `${value}%` },
				},
			},
			plugins: {
				legend: { display: false },
				tooltip: {
					backgroundColor: colors.tooltip, titleColor: colors.surface, bodyColor: colors.surface,
					displayColors: false, cornerRadius: 10, padding: 12, caretSize: 6,
					titleFont: { family: CONFIG.FONT, weight: '600' }, bodyFont: { family: CONFIG.FONT },
					callbacks: { title: items => formatMsToMinSec(items[0].parsed.x), label: item => `Similarity  ${item.parsed.y.toFixed(1)}%` },
				},
			},
		},
	};
}

function setupObservers() {
	if (resizeHandler) window.removeEventListener('resize', resizeHandler);
	resizeHandler = () => requestAnimationFrame(syncSliderWithChart);
	window.addEventListener('resize', resizeHandler, { passive: true });
	if (!themeWatcher && window.matchMedia) {
		themeWatcher = window.matchMedia('(prefers-color-scheme: dark)');
		themeWatcher.addEventListener?.('change', applyTheme);
	}
}

export function initSimilarityChart(min, max, lapCount, playerCount) {
	ready(Chart => {
		if (pendingUpdate) cancelAnimationFrame(pendingUpdate);
		pendingUpdate = 0;
		chart?.destroy();
		const canvas = document.getElementById('similarityChart');
		if (!canvas) return;
		currentLapCount = lapCount;
		currentPlayerCount = playerCount;
		chart = new Chart(canvas.getContext('2d'), createChartConfig(min, max));
		requestAnimationFrame(syncSliderWithChart);
		setupObservers();
	});
}

export function addSimilarityData(target, similarity, lapCount, playerCount) {
	if (!chart || lapCount !== currentLapCount || playerCount !== currentPlayerCount) return;
	const data = chart.data.datasets[0].data;
	let low = 0, high = data.length;
	while (low < high) {
		const middle = (low + high) >> 1;
		if (data[middle].x < target) low = middle + 1;
		else high = middle;
	}
	if (data[low]?.x === target) data[low].y = similarity;
	else data.splice(low, 0, { x: target, y: similarity });
	scheduleUpdate();
}

export function chartAddFailedTargetMarker(target, lapCount, playerCount) {
	if (!chart || lapCount !== currentLapCount || playerCount !== currentPlayerCount) return;
	let failed = chart.data.datasets.find(dataset => dataset.label === 'Failed');
	if (!failed) {
		const colors = theme();
		failed = { label: 'Failed', type: 'scatter', data: [], backgroundColor: colors.danger, borderColor: colors.danger, pointStyle: 'crossRot', pointRadius: 7, pointHoverRadius: 9 };
		chart.data.datasets.push(failed);
	}
	if (!failed.data.some(point => point.x === target)) {
		failed.data.push({ x: target, y: 0 });
		scheduleUpdate();
	}
}
