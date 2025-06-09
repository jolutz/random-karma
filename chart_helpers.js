// Chart.js helper functions for Random Karma visualization

// Configuration constants
const CONFIG = {
	MAX_RETRY_ATTEMPTS: 20,
	RETRY_DELAY_MS: 50,
	CHART_ANIMATION: false,
	SLIDER_WIDTH_BUFFER: 10,

	// Theme colors matching CSS design system
	COLORS: {
		primary: '#4361ee',
		danger: '#f87171',
		grid: '#e5e7eb',
		text: '#4b5563',
		tooltipBg: 'rgba(31, 41, 55, 0.8)',
		tooltipText: '#fff'
	},

	// Chart layout settings
	LAYOUT: {
		padding: { top: 5, bottom: 5, left: 10, right: 10 },
		fontSize: {
			tick: 10,
			tooltip: 12
		},
		maxTicks: 6
	}
};

// Global state
let chart = null;
let currentLapCount = null;
let currentPlayerCount = null;
let resizeHandler = null;

// Utility functions ----------------------------------------------------------

/**
 * Wait for Chart.js to load with retry mechanism
 * @param {Function} cb - Callback to execute when Chart.js is available
 * @param {number} tries - Current retry attempt
 */
function ready(cb, tries = 0) {
	if (typeof window.Chart !== "undefined") {
		cb(window.Chart);
	} else if (tries < CONFIG.MAX_RETRY_ATTEMPTS) {
		setTimeout(() => ready(cb, tries + 1), CONFIG.RETRY_DELAY_MS);
	} else {
		console.error("Chart.js did not load in time.");
	}
}

/**
 * Format milliseconds to minute:second format
 * @param {number} ms - Time in milliseconds
 * @returns {string} Formatted time string
 */
function formatMsToMinSec(ms) {
	const totalSec = Math.floor(ms / 1000);
	const m = Math.floor(totalSec / 60);
	const s = totalSec % 60;
	return `${m}m ${s}s`;
}

/**
 * Format milliseconds to compact M:SS format for axis labels
 * @param {number} ms - Time in milliseconds
 * @returns {string} Formatted time string
 */
function formatMsCompact(ms) {
	const sec = Math.floor(ms / 1000);
	const m = Math.floor(sec / 60);
	const s = sec % 60;
	return `${m}:${s.toString().padStart(2, '0')}`;
}

/**
 * Synchronize slider width with chart width for visual alignment
 */
function syncSliderWithChart() {
	const chartCanvas = document.getElementById('similarityChart');
	const targetSlider = document.querySelector('.target-slider-container input[type="range"]');

	if (chartCanvas && targetSlider) {
		const chartWidth = chartCanvas.getBoundingClientRect().width;
		targetSlider.style.width = `${chartWidth - CONFIG.SLIDER_WIDTH_BUFFER}px`;
	}
}

// Chart management functions -------------------------------------------------

/**
 * Create chart configuration object
 * @param {number} min - Minimum x-axis value
 * @param {number} max - Maximum x-axis value
 * @returns {Object} Chart.js configuration
 */
function createChartConfig(min, max) {
	return {
		type: "line",
		data: {
			datasets: [
				{
					label: "Jaccard Similarity (%)",
					backgroundColor: CONFIG.COLORS.primary,
					borderColor: CONFIG.COLORS.primary,
					fill: false,
					tension: 0.35,
					pointRadius: 3,
					pointHoverRadius: 6,
					data: [],
				},
			],
		},
		options: {
			animation: CONFIG.CHART_ANIMATION,
			spanGaps: true,
			responsive: true,
			maintainAspectRatio: false,
			layout: {
				padding: CONFIG.LAYOUT.padding
			},
			scales: {
				x: {
					type: "linear",
					min,
					max,
					title: { display: false },
					grid: {
						display: true,
						drawBorder: false,
						color: CONFIG.COLORS.grid
					},
					ticks: {
						color: CONFIG.COLORS.text,
						font: {
							size: CONFIG.LAYOUT.fontSize.tick,
							family: "'Inter', sans-serif"
						},
						maxRotation: 0,
						autoSkip: true,
						maxTicksLimit: CONFIG.LAYOUT.maxTicks,
						padding: 0,
						callback: (value) => formatMsCompact(value)
					},
				},
				y: {
					min: 0,
					max: 100,
					display: false,
					grid: {
						color: CONFIG.COLORS.grid,
						drawBorder: false
					},
				},
			},
			plugins: {
				legend: { display: false },
				tooltip: {
					backgroundColor: CONFIG.COLORS.tooltipBg,
					titleColor: CONFIG.COLORS.tooltipText,
					bodyColor: CONFIG.COLORS.tooltipText,
					titleFont: { family: "'Inter', sans-serif" },
					bodyFont: { family: "'Inter', sans-serif" },
					cornerRadius: 6,
					padding: 10,
					callbacks: {
						title: (context) => formatMsToMinSec(context[0].parsed.x),
						label: (context) => `${context.dataset.label}: ${context.parsed.y.toFixed(1)}%`
					}
				}
			},
			elements: {
				line: { borderWidth: 2 },
				point: { backgroundColor: "#fff", borderWidth: 2 },
			},
		},
	};
}

/**
 * Setup resize event handler
 */
function setupResizeHandler() {
	if (resizeHandler) {
		window.removeEventListener('resize', resizeHandler);
	}

	resizeHandler = () => {
		requestAnimationFrame(() => {
			syncSliderWithChart();
		});
	};

	window.addEventListener('resize', resizeHandler);
}

// Exported functions ---------------------------------------------------------

/**
 * Initialize the similarity chart
 * @param {number} min - Minimum target value
 * @param {number} max - Maximum target value
 * @param {number} lapCount - Current lap count
 * @param {number} playerCount - Current player count
 */
export function initSimilarityChart(min, max, lapCount, playerCount) {
	ready((Chart) => {
		// Clean up existing chart
		if (chart) {
			chart.destroy();
		}

		// Update current parameters
		currentLapCount = lapCount;
		currentPlayerCount = playerCount;

		// Get canvas context
		const ctx = document.getElementById("similarityChart").getContext("2d");

		// Create new chart
		chart = new Chart(ctx, createChartConfig(min, max));

		// Setup slider synchronization
		setTimeout(() => {
			syncSliderWithChart();
		}, 100);

		setupResizeHandler();
	});
}

/**
 * Add or update a similarity data point
 * @param {number} target - Target value
 * @param {number} similarity - Similarity percentage (0-100)
 * @param {number} lapCount - Lap count for this data point
 * @param {number} playerCount - Player count for this data point
 */
export function addSimilarityData(target, similarity, lapCount, playerCount) {
	if (!chart || lapCount !== currentLapCount || playerCount !== currentPlayerCount) {
		return;
	}

	const data = chart.data.datasets[0].data;

	// Binary search to maintain sorted order
	let lo = 0, hi = data.length;
	while (lo < hi) {
		const mid = (lo + hi) >> 1;
		if (data[mid].x < target) {
			lo = mid + 1;
		} else {
			hi = mid;
		}
	}

	// Update existing or insert new
	if (data[lo]?.x === target) {
		data[lo].y = similarity;
	} else {
		data.splice(lo, 0, { x: target, y: similarity });
	}

	chart.update({ animation: false });
}

/**
 * Mark a target that failed to compute with a red marker
 * @param {number} target - Target value that failed
 * @param {number} lapCount - Lap count for this failure
 * @param {number} playerCount - Player count for this failure
 */
export function chartAddFailedTargetMarker(target, lapCount, playerCount) {
	if (!chart || lapCount !== currentLapCount || playerCount !== currentPlayerCount) {
		return;
	}

	// Find or create failed dataset
	let failedDs = chart.data.datasets.find(d => d.label === "Failed");
	if (!failedDs) {
		failedDs = {
			label: "Failed",
			type: "scatter",
			backgroundColor: CONFIG.COLORS.danger,
			borderColor: CONFIG.COLORS.danger,
			pointStyle: "crossRot",
			pointRadius: 8,
			pointHoverRadius: 10,
			data: [],
		};
		chart.data.datasets.push(failedDs);
	}

	// Add marker if not already present
	if (!failedDs.data.some(p => p.x === target)) {
		failedDs.data.push({ x: target, y: 0 });
		chart.update({ animation: false });
	}
}
