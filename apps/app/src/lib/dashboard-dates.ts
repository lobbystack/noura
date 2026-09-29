export function plusDays(date: Date, days: number): Date {
	const next = new Date(date);
	next.setDate(next.getDate() + days);
	return next;
}

const DAY_MS = 24 * 60 * 60 * 1000;

function startOfDay(date: Date): number {
	return new Date(
		date.getFullYear(),
		date.getMonth(),
		date.getDate(),
	).getTime();
}

/** Whole calendar days from `now` to `date`: 0 today, 1 tomorrow, -1 yesterday. */
function dayOffset(date: Date, now: Date): number {
	return Math.round((startOfDay(date) - startOfDay(now)) / DAY_MS);
}

function dayLabel(date: Date, now: Date): string {
	const offset = dayOffset(date, now);
	if (offset === 0) return 'Today';
	if (offset === 1) return 'Tomorrow';
	if (offset === -1) return 'Yesterday';
	if (offset > 1 && offset < 7)
		return date.toLocaleDateString(undefined, { weekday: 'long' });
	return date.toLocaleDateString(undefined, {
		month: 'short',
		day: 'numeric',
		...(date.getFullYear() === now.getFullYear() ? {} : { year: 'numeric' }),
	});
}

/**
 * A short, readable label for a due or start value. All-day values read as
 * Today, Tomorrow, a weekday within the week, or a short date. Timed values
 * add their clock time, and show only the time when they fall today.
 */
export function dueLabel(due: string, now: Date): string {
	if (/^\d{4}-\d{2}-\d{2}$/.test(due)) {
		const [year, month, day] = due.split('-').map(Number);
		const date = new Date(year, month - 1, day);
		return Number.isNaN(date.getTime()) ? due : dayLabel(date, now);
	}
	const parsed = new Date(due);
	if (Number.isNaN(parsed.getTime())) return due;
	const time = parsed.toLocaleTimeString([], {
		hour: '2-digit',
		minute: '2-digit',
	});
	return dayOffset(parsed, now) === 0
		? time
		: `${dayLabel(parsed, now)} ${time}`;
}

/** True when an all-day or timed value lies before today. */
export function isOverdue(due: string, now: Date): boolean {
	const date = /^\d{4}-\d{2}-\d{2}$/.test(due)
		? new Date(`${due}T00:00:00`)
		: new Date(due);
	return !Number.isNaN(date.getTime()) && dayOffset(date, now) < 0;
}

export function daypartGreeting(now: Date): string {
	const hour = now.getHours();
	if (hour < 5) return 'Working late';
	if (hour < 12) return 'Good morning';
	if (hour < 18) return 'Good afternoon';
	return 'Good evening';
}
