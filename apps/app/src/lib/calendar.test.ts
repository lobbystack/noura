import { describe, expect, test } from 'bun:test';
import type { CalendarEntry } from '@noura/workspace';
import {
	calendarRange,
	calendarDayRange,
	calendarWeekRange,
	formatCalendarBoundary,
	isoDay,
	projectCalendarWeek,
	projectCalendarDay,
} from './calendar';

function entry(
	start: string,
	end: string | null,
	allDay: boolean,
	title = start,
): CalendarEntry {
	return {
		sourceId: `task-${title}`,
		sourceType: 'task',
		title,
		property: 'start',
		start,
		end,
		allDay,
		revision: 'revision',
	};
}

describe('calendar ranges', () => {
	test('builds the fixed six-week month range from Sunday', () => {
		const range = calendarRange(new Date(2026, 8, 16));
		expect(range.start.slice(0, 10)).toBe('2026-08-30');
		expect(range.end.slice(0, 10)).toBe('2026-10-11');
	});

	test('formats local midnight with the offset at that boundary', () => {
		const value = new Date(2026, 2, 8);
		expect(formatCalendarBoundary(value).slice(0, 10)).toBe('2026-03-08');
		expect(isoDay(value)).toBe('2026-03-08');
	});

	test('uses the offset on each side of a daylight-saving transition', () => {
		const child = Bun.spawnSync({
			cmd: [
				process.execPath,
				'-e',
				'import { formatCalendarBoundary } from "./apps/app/src/lib/calendar.ts"; console.log(JSON.stringify([formatCalendarBoundary(new Date(2026, 2, 8)), formatCalendarBoundary(new Date(2026, 2, 9))]));',
			],
			env: { ...process.env, TZ: 'America/Toronto' },
		});
		expect(child.exitCode).toBe(0);
		expect(JSON.parse(child.stdout.toString())).toEqual([
			'2026-03-08T00:00:00-05:00',
			'2026-03-09T00:00:00-04:00',
		]);
	});

	test('builds a Sunday-through-Saturday week range', () => {
		const range = calendarWeekRange(new Date(2026, 8, 4));
		expect(range.start.slice(0, 10)).toBe('2026-08-30');
		expect(range.end.slice(0, 10)).toBe('2026-09-06');
	});

	test('builds one local civil day and projects only that day', () => {
		const focus = new Date(2026, 8, 4);
		const range = calendarDayRange(focus);
		expect(range.start.slice(0, 10)).toBe('2026-09-04');
		expect(range.end.slice(0, 10)).toBe('2026-09-05');

		const day = projectCalendarDay(
			[
				entry('2026-09-03', null, true, 'Yesterday'),
				entry('2026-09-04', null, true, 'Today'),
			],
			focus,
		);
		expect(isoDay(day.date)).toBe('2026-09-04');
		expect(day.allDayEntries.map(({ title }) => title)).toEqual(['Today']);
	});

	test('expands all-day spans and clips timed spans to each visible day', () => {
		const days = projectCalendarWeek(
			[
				entry('2026-09-01', '2026-09-04', true, 'Retreat'),
				entry('2026-09-02T23:00:00', '2026-09-03T02:00:00', false, 'Deploy'),
			],
			new Date(2026, 8, 2),
		);

		expect(days.map((day) => day.allDayEntries.length)).toEqual([
			0, 0, 1, 1, 1, 0, 0,
		]);
		expect(days[3].timedSegments[0].startMinutes).toBe(23 * 60);
		expect(days[3].timedSegments[0].endMinutes).toBe(24 * 60);
		expect(days[4].timedSegments[0].startMinutes).toBe(0);
		expect(days[4].timedSegments[0].endMinutes).toBe(2 * 60);
	});

	test('places overlapping timed entries in separate lanes', () => {
		const days = projectCalendarWeek(
			[
				entry('2026-09-02T09:00:00', '2026-09-02T11:00:00', false, 'First'),
				entry('2026-09-02T10:00:00', '2026-09-02T12:00:00', false, 'Second'),
			],
			new Date(2026, 8, 2),
		);

		expect(days[3].timedSegments.map(({ lane }) => lane)).toEqual([0, 1]);
		expect(
			days[3].timedSegments.every(({ laneCount }) => laneCount === 2),
		).toBe(true);
	});
});
