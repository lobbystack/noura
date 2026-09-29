import type { BlockContext, Line, MarkdownConfig } from '@lezer/markdown';
import { tags } from '@lezer/highlight';

/**
 * Lookahead limit for fences that must be closed before they count. Larger
 * unclosed blocks parse as ordinary Markdown, which is also what happens when
 * a file really has no closing fence.
 */
const LOOKAHEAD = 256 * 1024;

interface ReadableInput {
	length: number;
	read(from: number, to: number): string;
}

/**
 * The parse context keeps its input private. Reading ahead is the only way to
 * refuse an unclosed fence without consuming lines that belong to other
 * blocks, so this degrades to "no match" if the field ever disappears.
 */
function lookahead(cx: BlockContext, from: number): string | null {
	const input = (cx as unknown as { input?: ReadableInput }).input;
	if (!input || typeof input.read !== 'function') return null;
	return input.read(from, Math.min(input.length, from + LOOKAHEAD));
}

const FRONTMATTER_OPEN = /^---[ \t]*$/;
const FRONTMATTER_CLOSE = /^(?:---|\.\.\.)[ \t]*$/;

/**
 * YAML frontmatter at the very start of a file. Without this, the opening
 * `---` parses as a rule and the block as a Setext heading, so its keys get
 * heading, tag, and link styling.
 */
const Frontmatter: MarkdownConfig = {
	defineNodes: [
		{ name: 'Frontmatter', block: true, style: tags.meta },
		{ name: 'FrontmatterMark', style: tags.processingInstruction },
	],
	parseBlock: [
		{
			name: 'Frontmatter',
			before: 'HorizontalRule',
			parse(cx: BlockContext, line: Line) {
				if (cx.lineStart !== 0 || !FRONTMATTER_OPEN.test(line.text)) {
					return false;
				}
				const ahead = lookahead(cx, 0);
				if (ahead === null) return false;
				const rest = ahead.split('\n').slice(1);
				if (!rest.some((text) => FRONTMATTER_CLOSE.test(text))) return false;
				const marks = [cx.elt('FrontmatterMark', 0, 3)];
				while (cx.nextLine()) {
					if (FRONTMATTER_CLOSE.test(line.text)) {
						marks.push(
							cx.elt('FrontmatterMark', cx.lineStart, cx.lineStart + 3),
						);
						cx.nextLine();
						break;
					}
				}
				cx.addElement(cx.elt('Frontmatter', 0, cx.prevLineEnd(), marks));
				return true;
			},
		},
	],
};

function mathOpening(line: Line): boolean {
	return (
		line.indent - line.baseIndent < 4 && line.text.startsWith('$$', line.pos)
	);
}

/**
 * Display math delimited by `$$`, on one line or across several. Parsing it
 * as a block keeps its content away from emphasis, tags, and links, and lets
 * the preview replace the whole block with one rendered formula.
 */
const MathBlock: MarkdownConfig = {
	defineNodes: [
		{ name: 'MathBlock', block: true, style: tags.special(tags.content) },
		{ name: 'MathMark', style: tags.processingInstruction },
	],
	parseBlock: [
		{
			name: 'MathBlock',
			before: 'FencedCode',
			parse(cx: BlockContext, line: Line) {
				if (!mathOpening(line)) return false;
				const start = cx.lineStart + line.pos;
				const opener = line.text.slice(line.pos + 2);
				const openerEnd = cx.lineStart + line.text.length;
				// `$$x$$` on one line.
				if (
					/\S/.test(opener.replace(/\$\$\s*$/, '')) &&
					/\$\$\s*$/.test(opener)
				) {
					const close = cx.lineStart + line.text.lastIndexOf('$$');
					const marks = [
						cx.elt('MathMark', start, start + 2),
						cx.elt('MathMark', close, close + 2),
					];
					cx.nextLine();
					cx.addElement(cx.elt('MathBlock', start, openerEnd, marks));
					return true;
				}
				const ahead = lookahead(cx, openerEnd);
				if (ahead === null || !/\$\$[ \t]*(?:\n|$)/.test(ahead)) return false;
				const marks = [cx.elt('MathMark', start, start + 2)];
				while (cx.nextLine()) {
					const closing = line.text.search(/\$\$[ \t]*$/);
					if (closing >= 0) {
						marks.push(
							cx.elt(
								'MathMark',
								cx.lineStart + closing,
								cx.lineStart + closing + 2,
							),
						);
						cx.nextLine();
						break;
					}
				}
				cx.addElement(cx.elt('MathBlock', start, cx.prevLineEnd(), marks));
				return true;
			},
			endLeaf: (_cx: BlockContext, line: Line) => mathOpening(line),
		},
	],
};

/** Obsidian-flavoured block syntax the preview and commands rely on. */
export const markdownSyntax: MarkdownConfig[] = [Frontmatter, MathBlock];
