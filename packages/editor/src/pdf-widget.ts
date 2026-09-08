import { WidgetType, type EditorView } from '@codemirror/view';
import type { LiveMarkdownOptions } from './types';

type Callbacks = {
	[K in 'resolveLink' | 'openPdf' | 'mountPdfEmbed']?:
		LiveMarkdownOptions[K] | undefined;
};

/** A headless mount point; the application owns all PDF rendering. */
export class PdfWidget extends WidgetType {
	private readonly cleanups = new WeakMap<HTMLElement, () => void>();
	constructor(
		private readonly target: string,
		private readonly label: string,
		private readonly embed: boolean,
		private readonly from: number,
		private readonly to: number,
		private readonly callbacks: Callbacks,
	) {
		super();
	}
	eq(other: PdfWidget) {
		return (
			other.target === this.target &&
			other.label === this.label &&
			other.embed === this.embed &&
			other.from === this.from &&
			other.to === this.to
		);
	}
	toDOM(view: EditorView): HTMLElement {
		const root = document.createElement(this.embed ? 'div' : 'span');
		root.className = 'cm-pdf-preview';
		let disposed = false;
		let mounted: (() => void) | undefined;
		let observer: IntersectionObserver | undefined;
		const open = document.createElement('button');
		open.type = 'button';
		open.textContent = this.label || this.target;
		open.setAttribute('aria-label', `Open PDF: ${this.label || this.target}`);
		root.append(open);
		if (!view.state.readOnly) {
			const edit = document.createElement('button');
			edit.type = 'button';
			edit.textContent = 'Edit source';
			edit.style.marginInlineStart = '0.5rem';
			edit.onclick = () => {
				view.dispatch({ selection: { anchor: this.from, head: this.to } });
				view.focus();
			};
			root.append(edit);
		}
		open.disabled = true;
		void this.callbacks
			.resolveLink?.(this.target)
			.then((resolved) => {
				if (disposed) return;
				if (resolved.kind !== 'pdf' || !resolved.relativePath) {
					open.textContent = `PDF unavailable: ${this.label || this.target}`;
					return;
				}
				const target = {
					relativePath: resolved.relativePath,
					page: resolved.page ?? null,
				};
				open.disabled = false;
				open.onclick = () => this.callbacks.openPdf?.(target);
				if (!this.embed || !this.callbacks.mountPdfEmbed) return;
				const container = document.createElement('div');
				container.style.height = '480px';
				container.style.width = '100%';
				container.style.overflow = 'hidden';
				root.append(container);
				const mount = () => {
					if (!disposed)
						mounted ??= this.callbacks.mountPdfEmbed?.(container, target);
				};
				if (typeof IntersectionObserver === 'undefined') mount();
				else {
					observer = new IntersectionObserver(
						([entry]) => {
							if (entry?.isIntersecting) mount();
							else {
								mounted?.();
								mounted = undefined;
							}
						},
						{ rootMargin: '200px' },
					);
					observer.observe(container);
				}
			})
			.catch(() => {
				if (!disposed)
					open.textContent = `PDF unavailable: ${this.label || this.target}`;
			});
		this.cleanups.set(root, () => {
			disposed = true;
			observer?.disconnect();
			mounted?.();
		});
		return root;
	}
	ignoreEvent(): boolean {
		return true;
	}
	destroy(dom: HTMLElement) {
		this.cleanups.get(dom)?.();
		this.cleanups.delete(dom);
	}
}
