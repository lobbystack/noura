import { describe, expect, test } from 'bun:test';
import {
	daypartGreeting,
	dueLabel,
	isOverdue,
	plusDays,
} from './dashboard-dates';

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

	test('all-day values near today read as relative days', () => {
		const now = new Date(2026, 8, 2, 15, 0); // Wednesday
		expect(dueLabel('2026-09-03', now)).toBe('Tomorrow');
		expect(dueLabel('2026-09-01', now)).toBe('Yesterday');
		expect(dueLabel('2026-09-05', now)).toBe(
			new Date(2026, 8, 5).toLocaleDateString(undefined, { weekday: 'long' }),
		);
	});

	test('all-day values further out read as a short date', () => {
		const now = new Date(2026, 8, 2, 15, 0);
		const label = dueLabel('2026-10-15', now);
		expect(label).not.toContain('2026');
		expect(label).toMatch(/15/);
		expect(dueLabel('2027-01-04', now)).toMatch(/2027/);
	});

	test('unreadable values fall back to the stored text', () => {
		const now = new Date(2026, 8, 2, 15, 0);
		expect(dueLabel('someday', now)).toBe('someday');
	});

	test('overdue compares calendar days', () => {
		const now = new Date(2026, 8, 2, 15, 0);
		expect(isOverdue('2026-09-01', now)).toBe(true);
		expect(isOverdue('2026-09-02', now)).toBe(false);
		expect(isOverdue('2026-09-02T08:00:00', now)).toBe(false);
		expect(isOverdue('not a date', now)).toBe(false);
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
