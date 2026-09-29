import { beforeAll, describe, expect, test } from 'bun:test';
import { GlobalRegistrator } from '@happy-dom/global-registrator';
import { ImageWidget } from './widgets';

beforeAll(() => {
	if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register();
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('remote images', () => {
	test('stay unloaded until asked, then show what the loader returns', async () => {
		const requested: string[] = [];
		const widget = new ImageWidget(
			'Chart',
			'https://example.com/chart.png',
			{},
			undefined,
			async (url) => {
				requested.push(url);
				return 'data:image/png;base64,AAAA';
			},
		);
		const dom = widget.toDOM();
		expect(dom.querySelector('img')).toBeNull();
		expect(requested).toEqual([]);
		dom.querySelector('button')!.click();
		await settle();
		expect(requested).toEqual(['https://example.com/chart.png']);
		expect(dom.querySelector('img')?.getAttribute('src')).toBe(
			'data:image/png;base64,AAAA',
		);
	});

	test('offer a retry when the download fails', async () => {
		const widget = new ImageWidget(
			'Chart',
			'https://example.com/chart.png',
			{},
			undefined,
			async () => {
				throw new Error('offline');
			},
		);
		const dom = widget.toDOM();
		const button = dom.querySelector('button')!;
		button.click();
		await settle();
		expect(dom.querySelector('img')).toBeNull();
		expect(button.disabled).toBe(false);
		expect(button.textContent).toContain('Try again');
	});
});
