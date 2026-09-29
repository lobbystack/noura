import { WidgetType, type EditorView } from '@codemirror/view';
import type { ImageOptions, TableCell } from './preview';
import type { LiveMarkdownOptions } from './types';

/** Put the caret on a widget's source so the raw syntax shows for editing. */
function revealSource(view: EditorView, element: HTMLElement) {
	const position = view.posAtDOM(element);
	view.dispatch({ selection: { anchor: position } });
	view.focus();
}

export class ImageWidget extends WidgetType {
	constructor(
		private readonly alt: string,
		private readonly src: string,
		private readonly options: ImageOptions,
		private readonly resolve?: LiveMarkdownOptions['resolveImage'],
		private readonly loadRemote?: LiveMarkdownOptions['loadRemoteImage'],
	) {
		super();
	}

	eq(widget: ImageWidget) {
		return (
			widget.src === this.src &&
			widget.alt === this.alt &&
			widget.options.width === this.options.width &&
			widget.options.title === this.options.title
		);
	}

	toDOM() {
		const wrap = document.createElement('span');
		wrap.className = 'cm-md-image';
		wrap.setAttribute('aria-label', this.alt || 'Image');
		const placeholder = document.createElement('span');
		placeholder.className = 'cm-md-image-placeholder';
		placeholder.textContent = this.alt || this.src;
		wrap.appendChild(placeholder);
		const show = (url: string) => {
			const img = document.createElement('img');
			img.src = url;
			img.alt = this.alt;
			img.loading = 'lazy';
			if (this.options.title) img.title = this.options.title;
			if (this.options.width) img.width = this.options.width;
			wrap.replaceChildren(img);
		};
		if (/^https?:\/\//i.test(this.src)) {
			// Remote images stay unloaded until asked: loading one tells the
			// server that this note was opened.
			const button = document.createElement('button');
			button.type = 'button';
			button.className = 'cm-md-image-load';
			button.textContent = 'Load image';
			button.addEventListener('click', () => {
				if (!this.loadRemote) return show(this.src);
				button.disabled = true;
				button.textContent = 'Loading…';
				this.loadRemote(this.src)
					.then(show)
					.catch(() => {
						button.disabled = false;
						button.textContent = 'Couldn’t load image. Try again';
					});
			});
			wrap.appendChild(button);
			return wrap;
		}
		const resolved = this.resolve?.(this.src) ?? null;
		if (resolved instanceof Promise) {
			void resolved.then((url) => url && show(url)).catch(() => {});
		} else if (resolved) {
			show(resolved);
		}
		return wrap;
	}

	ignoreEvent() {
		return false;
	}
}

let katexStyles: Promise<unknown> | null = null;

/** KaTeX and its stylesheet load on the first formula, not with the editor. */
async function loadKatex() {
	katexStyles ??= import('katex/dist/katex.min.css').catch(() => {
		katexStyles = null;
	});
	const [{ default: katex }] = await Promise.all([
		import('katex'),
		katexStyles,
	]);
	return katex;
}

export class MathWidget extends WidgetType {
	constructor(
		private readonly source: string,
		private readonly displayMode: boolean,
	) {
		super();
	}

	eq(widget: MathWidget) {
		return (
			widget.source === this.source && widget.displayMode === this.displayMode
		);
	}

	toDOM() {
		const element = document.createElement(this.displayMode ? 'div' : 'span');
		element.className = this.displayMode
			? 'cm-md-math-block'
			: 'cm-md-math-inline';
		// The raw source stays visible until (and if) KaTeX fails to load or
		// render; a math failure must never reject unhandled.
		element.textContent = this.source;
		void loadKatex()
			.then((katex) => {
				katex.render(this.source, element, {
					displayMode: this.displayMode,
					throwOnError: false,
					trust: false,
					strict: 'ignore',
				});
			})
			.catch(() => {
				/* keep the plain-text source */
			});
		return element;
	}

	ignoreEvent() {
		// Clicking a formula puts the caret there, which reveals its source.
		return false;
	}
}

interface LinkCallbacks {
	resolveLink?: LiveMarkdownOptions['resolveLink'] | undefined;
	openLink?: LiveMarkdownOptions['openLink'] | undefined;
}

export class LinkWidget extends WidgetType {
	constructor(
		private readonly target: string,
		private readonly label: string,
		private readonly embed: boolean,
		private readonly callbacks: LinkCallbacks,
	) {
		super();
	}

	// Positions are read from the DOM when needed, so an edit above the link
	// keeps the rendered widget instead of rebuilding and re-resolving it.
	eq(widget: LinkWidget) {
		return (
			widget.target === this.target &&
			widget.label === this.label &&
			widget.embed === this.embed
		);
	}

	toDOM(view: EditorView) {
		const element: HTMLElement = document.createElement(
			this.embed ? 'div' : 'span',
		);
		element.className = this.embed
			? 'cm-md-embed-preview'
			: 'cm-md-link-preview';
		element.tabIndex = 0;
		element.setAttribute('role', 'link');
		element.textContent = this.label;
		const follow = (event: MouseEvent | KeyboardEvent) => {
			event.preventDefault();
			if (this.callbacks.openLink) {
				this.callbacks.openLink(this.target, {
					kind: 'wiki',
					newTab: event.metaKey || event.ctrlKey,
				});
			} else {
				revealSource(view, element);
			}
		};
		// Keep CodeMirror from moving the caret into the link on mousedown.
		element.addEventListener('mousedown', (event) => event.preventDefault());
		element.addEventListener('click', follow);
		element.addEventListener('keydown', (event) => {
			if (event.key === 'Enter') follow(event);
			else if (event.key === ' ') {
				event.preventDefault();
				revealSource(view, element);
			}
		});
		if (!this.callbacks.resolveLink) return element;
		void this.callbacks
			.resolveLink(this.target)
			.then((resolved) => {
				if (resolved.kind === 'unresolved') {
					element.dataset.state = 'unresolved';
					return;
				}
				element.dataset.state = 'resolved';
				if (!this.embed) return;
				const heading = document.createElement('strong');
				heading.textContent = resolved.label ?? this.label;
				const preview = document.createElement('p');
				preview.textContent = (resolved.preview ?? '').slice(0, 800);
				element.replaceChildren(heading, preview);
			})
			.catch(() => {
				element.dataset.state = 'unresolved';
			});
		return element;
	}
}

export class TableWidget extends WidgetType {
	constructor(
		private readonly source: string,
		private readonly rows: TableCell[][],
	) {
		super();
	}

	eq(widget: TableWidget) {
		return widget.source === this.source;
	}

	toDOM(view: EditorView) {
		const table = document.createElement('table');
		table.className = 'cm-md-table';
		this.rows.forEach((row, rowIndex) => {
			if (
				rowIndex === 1 &&
				row.every((cell) => /^\s*:?-+:?\s*$/.test(cell.text))
			) {
				return;
			}
			const container =
				rowIndex === 0
					? document.createElement('thead')
					: document.createElement('tbody');
			const tr = document.createElement('tr');
			for (const cell of row) {
				const element = document.createElement(rowIndex === 0 ? 'th' : 'td');
				element.textContent = cell.text.trim();
				element.tabIndex = 0;
				const reveal = () => {
					const start = view.posAtDOM(table);
					view.dispatch({
						selection: { anchor: start + cell.from, head: start + cell.to },
					});
					view.focus();
				};
				element.addEventListener('click', reveal);
				element.addEventListener('keydown', (event) => {
					if (event.key === 'Enter' || event.key === ' ') {
						event.preventDefault();
						reveal();
					}
				});
				tr.appendChild(element);
			}
			container.appendChild(tr);
			table.appendChild(container);
		});
		return table;
	}
}

export class CheckboxWidget extends WidgetType {
	constructor(private readonly checked: boolean) {
		super();
	}

	eq(widget: CheckboxWidget) {
		return widget.checked === this.checked;
	}

	toDOM() {
		const box = document.createElement('span');
		box.className = 'cm-md-checkbox';
		box.setAttribute('role', 'checkbox');
		box.setAttribute('aria-checked', String(this.checked));
		box.dataset.checked = String(this.checked);
		box.contentEditable = 'false';
		return box;
	}

	ignoreEvent() {
		// The editor's mousedown handler toggles the task.
		return false;
	}
}
