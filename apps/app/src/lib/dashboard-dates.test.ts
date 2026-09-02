import { describe, expect, test } from 'bun:test';
import { daypartGreeting, dueLabel, plusDays } from './dashboard-dates';

describe('dashboard dates', () => {
	test('plusDays keeps the clock time and adds calendar days', () => {
		const base = new Date(2026, 8, 2, 9, 30); // Sep 2 2026, 09:30 local
		const next = plusDays(base, 7);
		expect(next.getFullYear()).toBe(2026);
		expect(next.getMonth()).toBe(8);
		expect(next.getDate()).toBe(9);
		expect(next.getHours()).toBe(9);
		expect(next.getMinutes()).toBe(30);
	});

	test('all-day values due today read as Today', () => {
		const now = new Date(2026, 8, 2, 15, 0);
		expect(dueLabel('2026-09-02', now)).toBe('Today');
	});

	test('all-day values on another day render the stored date', () => {
		const now = new Date(2026, 8, 2, 15, 0);
		expect(dueLabel('2026-09-05', now)).toBe('2026-09-05');
	});

	test('timed values render their local clock time', () => {
		const now = new Date(2026, 8, 2, 15, 0);
		const label = dueLabel('2026-09-02T17:30:00', now);
		expect(label).toMatch(/17:30|05:30/);
	});

	test('timed values on another day keep their weekday', () => {
		const now = new Date(2026, 8, 2, 15, 0);
		const sameDayLabel = dueLabel('2026-09-02T17:30:00', now);
		const laterDayLabel = dueLabel('2026-09-04T17:30:00', now);
		// Both labels render in the same runtime locale, so a later-day
		// label that stayed time-only would equal the same-day label.
		expect(laterDayLabel).not.toBe(sameDayLabel);
		expect(laterDayLabel).toMatch(/17:30|05:30/);
	});

	test('greeting follows the local hour', () => {
		expect(daypartGreeting(new Date(2026, 8, 2, 8, 0))).toBe('Good morning');
		expect(daypartGreeting(new Date(2026, 8, 2, 14, 0))).toBe('Good afternoon');
		expect(daypartGreeting(new Date(2026, 8, 2, 20, 0))).toBe('Good evening');
		expect(daypartGreeting(new Date(2026, 8, 2, 2, 0))).toBe('Working late');
	});
});
