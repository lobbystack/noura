import { shareMarkdown } from '$account/share-markdown';

let renderer: ((markdown: string) => string) | null = null;

/**
 * Renders a chat message to sanitized HTML. It uses the same strict tag
 * allowlist and DOMPurify pass as shared pages: no raw HTML, no images, no
 * scripts, and only http(s) links. One static render per finished message
 * replaces the editor view and Y.Doc each message used to create.
 */
export function renderChatMarkdown(markdown: string): string {
	renderer ??= shareMarkdown(window);
	return renderer(markdown);
}
