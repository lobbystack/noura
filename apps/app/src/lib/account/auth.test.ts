import { describe, expect, test } from 'bun:test';
import {
	accountCallbackPath,
	accountPath,
	devicePath,
	invitationPath,
	userCode,
} from './auth';

describe('device sign-in destinations', () => {
	test('preserves invitation callbacks and prioritizes device review without open redirects', () => {
		const token = 'a'.repeat(43);
		expect(accountCallbackPath('', token)).toBe(`/invite/${token}`);
		expect(accountCallbackPath('', `/invite/${token}`)).toBe(
			`/invite/${token}`,
		);
		expect(accountCallbackPath('abcd-1234', token)).toBe(
			'/account/device?user_code=ABCD1234',
		);
		for (const value of [
			'//evil.example',
			'https://evil.example',
			`/invite/${token}/extra`,
			`/invite/${token}?next=//evil`,
		])
			expect(accountCallbackPath('', value)).toBe('/account');
	});
	test('normalizes the displayed code without accepting arbitrary destinations', () => {
		expect(userCode('abcd-1234')).toBe('ABCD1234');
		expect(devicePath('abcd-1234')).toBe('/account/device?user_code=ABCD1234');
		for (const value of [
			'//evil.example',
			'https://evil.example',
			'ABCD1234&next=//evil',
			'ABCD123',
			'<script>',
		]) {
			expect(userCode(value)).toBe('');
			expect(accountPath(value)).toBe('/account');
			expect(devicePath(value)).toBe('/account/device');
		}
	});
	test('accepts only canonical invitation destinations', () => {
		const token = 'a'.repeat(43);
		expect(invitationPath(token)).toBe(`/invite/${token}`);
		for (const value of ['', '../account', 'a'.repeat(42), `${token}/other`])
			expect(invitationPath(value)).toBe('');
	});
});
