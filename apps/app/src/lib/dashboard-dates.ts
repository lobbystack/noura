export function plusDays(date: Date, days: number): Date {
	const next = new Date(date);
	next.setDate(next.getDate() + days);
	return next;
}

/** Label for a due value: all-day "today" stays friendly, everything
 * else shows its stored date or clock time. Timed values on another
 * day keep the weekday so the 7-day horizon stays readable. */
export function dueLabel(due: string, now: Date): string {
	if (due.length === 10) {
		const todayLocal = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
		return due === todayLocal ? 'Today' : due;
	}
	const parsed = new Date(due);
	if (Number.isNaN(parsed.getTime())) return due;
	if (parsed.toDateString() === now.toDateString()) {
		return parsed.toLocaleTimeString([], {
			hour: '2-digit',
			minute: '2-digit',
		});
	}
	return parsed.toLocaleString([], {
		weekday: 'short',
		hour: '2-digit',
		minute: '2-digit',
	});
}

export function daypartGreeting(now: Date): string {
	const hour = now.getHours();
	if (hour < 5) return 'Working late';
	if (hour < 12) return 'Good morning';
	if (hour < 18) return 'Good afternoon';
	return 'Good evening';
}
