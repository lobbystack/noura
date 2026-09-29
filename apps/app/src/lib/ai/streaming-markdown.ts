const FENCE = /^ {0,3}(`{3,}|~{3,})/;

/**
 * The end of the last complete Markdown block in `text`: the offset just
 * after the last blank line that is not inside a code fence. Everything
 * before it can be rendered once and never again.
 */
export function stableBlockEnd(text: string): number {
	let fence: string | null = null;
	let stableEnd = 0;
	let offset = 0;
	while (offset < text.length) {
		const newline = text.indexOf('\n', offset);
		// The last line has no newline yet, so it may still grow.
		if (newline === -1) break;
		const line = text.slice(offset, newline);
		offset = newline + 1;
		const marker = FENCE.exec(line)?.[1];
		if (fence) {
			if (marker && marker[0] === fence[0] && marker.length >= fence.length)
				fence = null;
			continue;
		}
		if (marker) {
			fence = marker;
			continue;
		}
		if (line.trim() === '') stableEnd = offset;
	}
	return stableEnd;
}

/**
 * Renders a reply while it streams. Finished blocks are rendered once and
 * kept; only the unfinished tail is shown as plain text. Rendering the whole
 * reply on every chunk made long replies slower with every token.
 */
export class StreamingMarkdown {
	readonly #render: (markdown: string) => string;
	#source = '';
	#stableEnd = 0;
	#html = '';

	constructor(render: (markdown: string) => string) {
		this.#render = render;
	}

	update(text: string): { html: string; tail: string } {
		if (!text.startsWith(this.#source.slice(0, this.#stableEnd))) {
			// The text was replaced rather than extended: start over.
			this.#stableEnd = 0;
			this.#html = '';
		}
		this.#source = text;
		const end = stableBlockEnd(text);
		if (end > this.#stableEnd) {
			this.#html += this.#render(text.slice(this.#stableEnd, end));
			this.#stableEnd = end;
		}
		return { html: this.#html, tail: text.slice(this.#stableEnd) };
	}
}
