import { describe, expect, test } from 'bun:test';
import { detectPlatform, getAppPlatform } from './platform';

describe('app platform detection', () => {
	test('a browser never inherits the native build target', () => {
		for (const target of [
			undefined,
			'',
			'darwin',
			'linux',
			'windows',
			'android',
			'ios',
		]) {
			expect(detectPlatform(false, target)).toBe('web');
		}
	});

	test('native targets distinguish mobile from desktop', () => {
		for (const target of ['android', 'ios'])
			expect(detectPlatform(true, target)).toBe('mobile');
		for (const target of ['darwin', 'linux', 'windows'])
			expect(detectPlatform(true, target)).toBe('desktop');
	});

	test('unknown native targets fail closed rather than defaulting to desktop', () => {
		for (const target of [undefined, '', 'unknown'])
			expect(detectPlatform(true, target)).toBeNull();
	});

	test('non-Tauri runtime is safe without window or native build metadata', () => {
		expect(getAppPlatform()).toBe('web');
	});
});
