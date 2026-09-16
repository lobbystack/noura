import { Marked } from 'marked';
import createDOMPurify, { type WindowLike } from 'dompurify';

/** Creates a browser-local renderer; never accepts raw HTML or automatic remote resources. */
export function shareMarkdown(
	window: WindowLike,
): (markdown: string) => string {
	const purifier = createDOMPurify(window);
	const parser = new Marked({
		async: false,
		renderer: { html: () => '', image: () => '[Image omitted]' },
	});
	purifier.addHook('afterSanitizeAttributes', (node) => {
		if (node.tagName === 'A') {
			const href = node.getAttribute('href');
			try {
				if (!href || !['https:', 'http:'].includes(new URL(href).protocol)) {
					node.removeAttribute('href');
					return;
				}
				node.setAttribute('rel', 'noopener noreferrer');
				node.setAttribute('target', '_blank');
			} catch {
				node.removeAttribute('href');
			}
		}
	});
	return (markdown) =>
		purifier.sanitize(parser.parse(markdown) as string, {
			ALLOWED_TAGS: [
				'p',
				'br',
				'hr',
				'h1',
				'h2',
				'h3',
				'h4',
				'h5',
				'h6',
				'blockquote',
				'pre',
				'code',
				'strong',
				'em',
				'del',
				'ul',
				'ol',
				'li',
				'a',
				'table',
				'thead',
				'tbody',
				'tr',
				'th',
				'td',
			],
			ALLOWED_ATTR: ['href', 'title', 'start'],
			ALLOW_DATA_ATTR: false,
			ALLOW_ARIA_ATTR: false,
		});
}
