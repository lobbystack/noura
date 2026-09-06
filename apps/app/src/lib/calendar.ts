import type { CalendarEntry } from '@noura/workspace';

const POINT_DURATION_MINUTES = 30;

export function isoDay(value: Date): string {
	const year = value.getFullYear();
	const month = String(value.getMonth() + 1).padStart(2, '0');
	const day = String(value.getDate()).padStart(2, '0');
	return `${year}-${month}-${day}`;
}

export function addCalendarDays(value: Date, days: number): Date {
	return new Date(
		value.getFullYear(),
		value.getMonth(),
		value.getDate() + days,
	);
}

export function startOfWeek(value: Date): Date {
	return addCalendarDays(value, -value.getDay());
}

export function formatCalendarBoundary(value: Date): string {
	const offsetMinutes = -value.getTimezoneOffset();
	const sign = offsetMinutes >= 0 ? '+' : '-';
	const offsetHours = String(Math.floor(Math.abs(offsetMinutes) / 60)).padStart(
		2,
		'0',
	);
	const offsetRemainder = String(Math.abs(offsetMinutes) % 60).padStart(2, '0');
	return `${isoDay(value)}T00:00:00${sign}${offsetHours}:${offsetRemainder}`;
}

export function calendarRange(focus: Date) {
	const first = new Date(focus.getFullYear(), focus.getMonth(), 1);
	const start = startOfWeek(first);
	return {
		start: formatCalendarBoundary(start),
		end: formatCalendarBoundary(addCalendarDays(start, 42)),
	};
}

export function calendarWeekRange(focus: Date) {
	const start = startOfWeek(focus);
	return {
		start: formatCalendarBoundary(start),
		end: formatCalendarBoundary(addCalendarDays(start, 7)),
	};
}

export function calendarDayRange(focus: Date) {
	const start = new Date(
		focus.getFullYear(),
		focus.getMonth(),
		focus.getDate(),
	);
	return {
		start: formatCalendarBoundary(start),
		end: formatCalendarBoundary(addCalendarDays(start, 1)),
	};
}

export type CalendarTimedSegment = {
	entry: CalendarEntry;
	start: Date;
	end: Date;
	startMinutes: number;
	endMinutes: number;
	lane: number;
	laneCount: number;
};

export type CalendarWeekDay = {
	date: Date;
	allDayEntries: CalendarEntry[];
	timedSegments: CalendarTimedSegment[];
};

function parseCivilDate(value: string): Date | null {
	const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
	if (!match) return null;
	const date = new Date(
		Number(match[1]),
		Number(match[2]) - 1,
		Number(match[3]),
	);
	return isoDay(date) === value ? date : null;
}

function compareEntries(a: CalendarEntry, b: CalendarEntry): number {
	return (
		a.start.localeCompare(b.start) ||
		a.title.localeCompare(b.title) ||
		a.sourceId.localeCompare(b.sourceId) ||
		a.property.localeCompare(b.property)
	);
}

function timedInterval(
	entry: CalendarEntry,
): { start: Date; end: Date } | null {
	const start = new Date(entry.start);
	if (Number.isNaN(start.getTime())) return null;
	const candidate = entry.end ? new Date(entry.end) : null;
	const end =
		candidate &&
		!Number.isNaN(candidate.getTime()) &&
		candidate.getTime() > start.getTime()
			? candidate
			: new Date(start.getTime() + POINT_DURATION_MINUTES * 60_000);
	return { start, end };
}

function assignLanes(segments: CalendarTimedSegment[]): CalendarTimedSegment[] {
	let groupStart = 0;
	while (groupStart < segments.length) {
		let groupEnd = groupStart + 1;
		let latestEnd = segments[groupStart].end.getTime();
		while (
			groupEnd < segments.length &&
			segments[groupEnd].start.getTime() < latestEnd
		) {
			latestEnd = Math.max(latestEnd, segments[groupEnd].end.getTime());
			groupEnd += 1;
		}

		const laneEnds: number[] = [];
		for (const segment of segments.slice(groupStart, groupEnd)) {
			const available = laneEnds.findIndex(
				(end) => end <= segment.start.getTime(),
			);
			segment.lane = available === -1 ? laneEnds.length : available;
			laneEnds[segment.lane] = segment.end.getTime();
		}
		for (let index = groupStart; index < groupEnd; index += 1) {
			segments[index].laneCount = laneEnds.length;
		}
		groupStart = groupEnd;
	}
	return segments;
}

export function projectCalendarWeek(
	entries: CalendarEntry[],
	focus: Date,
): CalendarWeekDay[] {
	const weekStart = startOfWeek(focus);
	return Array.from({ length: 7 }, (_, index) => {
		const date = addCalendarDays(weekStart, index);
		const dayEnd = addCalendarDays(date, 1);
		const allDayEntries = entries
			.filter((entry) => {
				if (!entry.allDay) return false;
				const start = parseCivilDate(entry.start);
				if (!start) return false;
				const candidateEnd = entry.end ? parseCivilDate(entry.end) : null;
				const end =
					candidateEnd && candidateEnd.getTime() > start.getTime()
						? candidateEnd
						: addCalendarDays(start, 1);
				return start < dayEnd && end > date;
			})
			.sort(compareEntries);

		const timedSegments = entries
			.filter((entry) => !entry.allDay)
			.map((entry) => ({ entry, interval: timedInterval(entry) }))
			.filter(
				(
					value,
				): value is {
					entry: CalendarEntry;
					interval: { start: Date; end: Date };
				} =>
					value.interval !== null &&
					value.interval.start < dayEnd &&
					value.interval.end > date,
			)
			.sort(
				(a, b) =>
					a.interval.start.getTime() - b.interval.start.getTime() ||
					compareEntries(a.entry, b.entry),
			)
			.map(({ entry, interval }) => {
				const start = new Date(
					Math.max(interval.start.getTime(), date.getTime()),
				);
				const end = new Date(
					Math.min(interval.end.getTime(), dayEnd.getTime()),
				);
				return {
					entry,
					start,
					end,
					startMinutes: (start.getTime() - date.getTime()) / 60_000,
					endMinutes: (end.getTime() - date.getTime()) / 60_000,
					lane: 0,
					laneCount: 1,
				};
			});

		return {
			date,
			allDayEntries,
			timedSegments: assignLanes(timedSegments),
		};
	});
}

export function projectCalendarDay(
	entries: CalendarEntry[],
	focus: Date,
): CalendarWeekDay {
	const key = isoDay(focus);
	const day = projectCalendarWeek(entries, focus).find(
		(candidate) => isoDay(candidate.date) === key,
	);
	if (day) return day;
	return { date: new Date(focus), allDayEntries: [], timedSegments: [] };
}
