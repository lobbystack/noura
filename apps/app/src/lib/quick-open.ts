/**
 * Quick open ranks workspace files by how well their name, then their path,
 * matches what was typed. Every typed character must appear in order
 * (a subsequence); consecutive characters, word starts and a match in the
 * file name rather than the folders all rank higher.
 */

export interface QuickOpenMatch {
	path: string;
	score: number;
}

const WORD_START = /[\s\-_./()[\]]/;

function subsequenceScore(text: string, query: string): number | null {
	const haystack = text.toLocaleLowerCase();
	let score = 0;
	let position = 0;
	let previous = -2;
	for (const character of query) {
		const found = haystack.indexOf(character, position);
		if (found === -1) return null;
		score += 1;
		if (found === previous + 1) score += 4;
		if (found === 0 || WORD_START.test(text[found - 1] ?? '')) score += 3;
		if (text[found] !== haystack[found] && found > 0) score += 1;
		previous = found;
		position = found + 1;
	}
	// Shorter texts that match equally well are closer matches.
	return score - text.length * 0.01;
}

export function quickOpenScore(path: string, query: string): number | null {
	const wanted = query.trim().toLocaleLowerCase().replace(/\s+/g, ' ');
	if (wanted.length === 0) return 0;
	const cut = path.lastIndexOf('/');
	const name = path.slice(cut + 1).replace(/\.md$/i, '');
	const nameScore = subsequenceScore(name, wanted);
	if (nameScore !== null) {
		const exact = name.toLocaleLowerCase() === wanted ? 20 : 0;
		const prefix = name.toLocaleLowerCase().startsWith(wanted) ? 10 : 0;
		return 100 + nameScore + exact + prefix;
	}
	return subsequenceScore(path, wanted);
}

/** The best `limit` matches for `query`, best first. */
export function quickOpen(
	paths: Iterable<string>,
	query: string,
	limit = 8,
): QuickOpenMatch[] {
	const matches: QuickOpenMatch[] = [];
	for (const path of paths) {
		const score = quickOpenScore(path, query);
		if (score !== null) matches.push({ path, score });
	}
	matches.sort(
		(left, right) =>
			right.score - left.score || left.path.localeCompare(right.path),
	);
	return matches.slice(0, limit);
}
