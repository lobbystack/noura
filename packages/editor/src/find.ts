import { Prec, type Extension } from '@codemirror/state';
import { EditorView, keymap, type ViewUpdate } from '@codemirror/view';
import {
	SearchQuery,
	closeSearchPanel,
	findNext,
	findPrevious,
	getSearchQuery,
	openSearchPanel,
	search,
	searchKeymap,
	searchPanelOpen,
	setSearchQuery,
} from '@codemirror/search';

/** Where the selected match sits among all matches. */
export interface FindMatches {
	/** 1-based position of the selected match, or 0 when none is selected. */
	current: number;
	total: number;
	/** Counting stopped at `total`; there are more. */
	capped: boolean;
}

/**
 * Find in one editor, without any interface. The app draws the find bar and
 * drives the search through this.
 */
export interface FindController {
	/** The text being searched for. */
	query(): string;
	/** Search for `text` and select the first match at or after the caret. */
	setQuery(text: string): void;
	next(): void;
	previous(): void;
	matches(): FindMatches;
	/** Close find and return focus to the text. */
	close(): void;
	/** Called after anything that can change the query or the matches. */
	subscribe(listener: () => void): () => void;
}

/**
 * Receives the controller when find opens, `null` when it closes, and
 * `focus` again when find is asked for while already open.
 */
export interface FindHost {
	open(controller: FindController): void;
	focus(): void;
	close(): void;
}

/** Matches counted before the count shows as "1000+". */
const MATCH_LIMIT = 1000;

const hosts = new WeakMap<EditorView, FindHost>();

/**
 * Open find, or bring focus back to it when it is already open. The query
 * starts as the selected text, as in the default panel.
 */
export function openFind(view: EditorView): boolean {
	if (searchPanelOpen(view.state)) {
		const selected = view.state.sliceDoc(
			view.state.selection.main.from,
			view.state.selection.main.to,
		);
		if (selected && !selected.includes('\n'))
			view.dispatch({ effects: setSearchQuery.of(literal(selected)) });
		hosts.get(view)?.focus();
		return true;
	}
	return openSearchPanel(view);
}

function literal(text: string) {
	return new SearchQuery({ search: text, literal: true });
}

function countMatches(view: EditorView): FindMatches {
	const query = getSearchQuery(view.state);
	if (!query.valid || !query.search)
		return { current: 0, total: 0, capped: false };
	const { from, to } = view.state.selection.main;
	const cursor = query.getCursor(view.state);
	let total = 0;
	let current = 0;
	for (let match = cursor.next(); !match.done; match = cursor.next()) {
		total += 1;
		if (match.value.from === from && match.value.to === to) current = total;
		if (total >= MATCH_LIMIT) return { current, total, capped: true };
	}
	return { current, total, capped: false };
}

function createController(view: EditorView) {
	const listeners = new Set<() => void>();
	const controller: FindController = {
		query: () => getSearchQuery(view.state).search,
		setQuery(text) {
			const query = literal(text);
			const effects = setSearchQuery.of(query);
			if (!text) {
				view.dispatch({ effects });
				return;
			}
			// Select the first match from the caret on, wrapping to the top, so
			// typing narrows onto the nearest match like a browser's find.
			const start = view.state.selection.main.from;
			let match = query.getCursor(view.state, start).next();
			if (match.done) match = query.getCursor(view.state).next();
			if (match.done) {
				view.dispatch({ effects });
				return;
			}
			view.dispatch({
				effects: [
					effects,
					EditorView.scrollIntoView(match.value.from, { y: 'center' }),
				],
				selection: { anchor: match.value.from, head: match.value.to },
				userEvent: 'select.search',
			});
		},
		next: () => void findNext(view),
		previous: () => void findPrevious(view),
		matches: () => countMatches(view),
		close() {
			closeSearchPanel(view);
			view.focus();
		},
		subscribe(listener) {
			listeners.add(listener);
			return () => listeners.delete(listener);
		},
	};
	return {
		controller,
		notify: () => {
			for (const listener of listeners) listener();
		},
	};
}

/**
 * Find with the interface supplied by `host`: Mod-F opens it, Mod-G and F3
 * move between matches, and matches are highlighted while it is open.
 */
export function findExtension(host: FindHost): Extension {
	return [
		search({
			literal: true,
			createPanel(view) {
				hosts.set(view, host);
				const { controller, notify } = createController(view);
				// Find draws outside the editor; the panel only marks it open.
				const dom = document.createElement('div');
				dom.hidden = true;
				return {
					dom,
					top: true,
					mount: () => host.open(controller),
					update: (update: ViewUpdate) => {
						if (
							update.docChanged ||
							update.selectionSet ||
							update.transactions.some((transaction) =>
								transaction.effects.some((effect) => effect.is(setSearchQuery)),
							)
						)
							notify();
					},
					destroy: () => {
						hosts.delete(view);
						host.close();
					},
				};
			},
		}),
		Prec.high(
			keymap.of([
				{ key: 'Mod-f', run: openFind, scope: 'editor search-panel' },
			]),
		),
		keymap.of(searchKeymap),
		// The hidden panel still gets a panel row; keep it from drawing a line.
		EditorView.theme({
			'.cm-panels-top': { borderBottom: 'none' },
			'.cm-panels': { backgroundColor: 'transparent' },
		}),
	];
}
