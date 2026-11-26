<script lang="ts" module>
	const paths = {
		inbox: 'M4 13.5 6.5 5h11l2.5 8.5V19H4v-5.5Zm0 0h5l1 2.5h4l1-2.5h5',
		sparkle:
			'M12 3.5 13.9 9 19.5 11 13.9 13 12 18.5 10.1 13 4.5 11 10.1 9 12 3.5Zm6 12 .8 2.2 2.2.8-2.2.8L18 21.5l-.8-2.2-2.2-.8 2.2-.8.8-2.2Z',
		note: 'M6 3.5h8.5L19 8v12.5H6v-17Zm8 0V8.5h5M9 13h7M9 16.5h5',
		checks: 'M3.5 7.5 5.5 9.5 9 6M3.5 15.5l2 2L9 14M12.5 8h8M12.5 16h8',
		calendar: 'M4.5 6h15v14h-15V6Zm0 4.5h15M8.5 3.5V8M15.5 3.5V8',
		folder: 'M3.5 6.5h6l2 2h9v11h-17v-13Z',
		search: 'M10.5 4a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13Zm4.8 11.3L20 20',
		gear: 'M12 9a3 3 0 1 1 0 6 3 3 0 0 1 0-6Zm0-5.5 1.6 2.4 2.8-.6.6 2.8 2.4 1.6-1.4 2.3 1.4 2.3-2.4 1.6-.6 2.8-2.8-.6L12 20.5l-1.6-2.4-2.8.6-.6-2.8L4.6 14.3 6 12 4.6 9.7 7 8.1l.6-2.8 2.8.6L12 3.5Z',
		file: 'M6.5 3.5h7l4 4v13h-11v-17Zm7 0v4h4',
		lock: 'M6 10.5h12v9.5H6v-9.5Zm2.5 0V8a3.5 3.5 0 0 1 7 0v2.5',
		arrow: 'M7 17 17 7M9 7h8v8',
		check: 'M5 12.5 10 17.5 19 7',
		// Primer Octicons `mark-github-24`, MIT. See docs/provenance/octicons-mark-github.md.
		github:
			'M10.226 17.284c-2.965-.36-5.054-2.493-5.054-5.256 0-1.123.404-2.336 1.078-3.144-.292-.741-.247-2.314.09-2.965.898-.112 2.111.36 2.83 1.01.853-.269 1.752-.404 2.853-.404 1.1 0 1.999.135 2.807.382.696-.629 1.932-1.1 2.83-.988.315.606.36 2.179.067 2.942.72.854 1.101 2 1.101 3.167 0 2.763-2.089 4.852-5.098 5.234.763.494 1.28 1.572 1.28 2.807v2.336c0 .674.561 1.056 1.235.786 4.066-1.55 7.255-5.615 7.255-10.646C23.5 6.188 18.334 1 11.978 1 5.62 1 .5 6.188.5 12.545c0 4.986 3.167 9.12 7.435 10.669.606.225 1.19-.18 1.19-.786V20.63a2.9 2.9 0 0 1-1.078.224c-1.483 0-2.359-.808-2.987-2.313-.247-.607-.517-.966-1.034-1.033-.27-.023-.359-.135-.359-.27 0-.27.45-.471.898-.471.652 0 1.213.404 1.797 1.235.45.651.921.943 1.483.943.561 0 .92-.202 1.437-.719.382-.381.674-.718.944-.943',
	} as const;

	export type IconName = keyof typeof paths;
</script>

<script lang="ts">
	let {
		name,
		size = 18,
		filled = false,
	}: { name: IconName; size?: number; filled?: boolean } = $props();
</script>

<svg
	class="icon"
	class:filled={filled || name === 'github'}
	width={size}
	height={size}
	viewBox="0 0 24 24"
	aria-hidden="true"
>
	<path d={paths[name]} />
</svg>

<style>
	.icon {
		flex: none;
	}
	path {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.6;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.filled path {
		fill: currentColor;
		stroke: none;
	}
</style>
