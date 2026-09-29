import { describe, expect, test } from 'bun:test';
import { EditorSelection, EditorState } from '@codemirror/state';
import type { Decoration as DecorationInstance } from '@codemirror/view';
import { Decoration, WidgetType } from '@codemirror/view';
import { markdownSupport } from './factory';
import { buildDecorations, inlineMathMatches } from './preview';

describe('inlineMathMatches', () => {
	test('recognizes compact inline math delimiters', () => {
		expect(inlineMathMatches('Use $x^2 + y^2$ here')).toEqual([
			{ index: 4, source: 'x^2 + y^2', text: '$x^2 + y^2$' },
		]);
	});

	test('does not interpret currency prose as math', () => {
		expect(inlineMathMatches('It costs $5 or $10 today')).toEqual([]);
		expect(inlineMathMatches('USD $5 and CAD $10')).toEqual([]);
	});
});

class StubWidget extends WidgetType {
	constructor(readonly token: string) {
		super();
	}

	eq(other: StubWidget) {
		return other.token === this.token;
	}

	toDOM(): HTMLElement {
		throw new Error('state-level decoration tests never render widgets');
	}
}

interface Specification {
	block?: boolean;
	class?: string;
	widget?: WidgetType;
}

interface DecoratedRange {
	from: number;
	to: number;
	spec: Specification;
}

function decorate(doc: string, cursor?: number) {
	const state = EditorState.create({
		doc,
		selection:
			cursor === undefined ? undefined : EditorSelection.cursor(cursor),
		extensions: [markdownSupport()],
	} as Parameters<typeof EditorState.create>[0]);
	const set = buildDecorations(
		state,
		[{ from: 0, to: state.doc.length }],
		Decoration,
		{
			image: (
				alt: string,
				src: string,
				options?: { width?: number | undefined; title?: string | undefined },
			) =>
				new StubWidget(
					`image:${src}:${alt}:${options?.width ?? ''}:${options?.title ?? ''}`,
				),
			checkbox: (checked: boolean) => new StubWidget(`checkbox:${checked}`),
			math: (source: string, displayMode: boolean) =>
				new StubWidget(`math:${displayMode}:${source}`),
			table: () => new StubWidget('table'),
			link: (target: string, label: string, embed: boolean) =>
				new StubWidget(`link:${embed}:${target}:${label}`),
		},
	);
	const ranges: DecoratedRange[] = [];
	set.between(0, state.doc.length, (from, to, deco: DecorationInstance) => {
		ranges.push({ from, to, spec: deco.spec as Specification });
	});
	return { state, ranges };
}

const blockTokens = (ranges: DecoratedRange[]) =>
	ranges.filter((item) => item.spec.block === true);

describe('buildDecorations', () => {
	test('replaces a GFM table with one block widget and hides the syntax', () => {
		const doc = '| A | B |\n| - | - |\n| 1 | 2 |\n';
		const { state, ranges } = decorate(doc, doc.length);
		const blocks = blockTokens(ranges);
		expect(blocks).toHaveLength(1);
		expect(blocks[0]?.from).toBe(state.doc.line(1).from);
		expect(blocks[0]?.to).toBe(state.doc.line(3).to);
		expect(blocks[0]?.spec.widget).toBeInstanceOf(StubWidget);
		// Table content is replaced wholesale; no leaves inside.
		expect(ranges).toHaveLength(1);
	});

	test('keeps an active table as raw markdown with row classes', () => {
		const { ranges } = decorate('| A | B |\n| - | - |\n| 1 | 2 |\n', 2);
		expect(blockTokens(ranges)).toHaveLength(0);
		expect(ranges.some((item) => item.spec.class === 'cm-md-table-row')).toBe(
			true,
		);
	});

	test('replaces display math blocks with a block widget', () => {
		const { state, ranges } = decorate('Intro\n\n$$x + y$$\n\nOutro\n');
		const blocks = blockTokens(ranges);
		expect(blocks).toHaveLength(1);
		expect(blocks[0]?.from).toBe(state.doc.line(3).from);
		expect(blocks[0]?.to).toBe(state.doc.line(3).to);
	});

	test('keeps cursor-adjacent display math as raw syntax', () => {
		const { ranges } = decorate('Intro\n\n$$x + y$$\n\nOutro\n', 10);
		expect(blockTokens(ranges)).toHaveLength(0);
	});

	test('renders standalone embeds as blocks and inline embeds inline', () => {
		const doc = '![[Notes/index]]\n\nprose ![[Note]] tail\n';
		const { state, ranges } = decorate(doc, doc.length);
		const blocks = blockTokens(ranges);
		expect(blocks).toHaveLength(1);
		expect(blocks[0]?.from).toBe(state.doc.line(1).from);
		expect(blocks[0]?.to).toBe(state.doc.line(1).to);
		const inline = ranges.filter(
			(item) => item.spec.widget !== undefined && item.spec.block !== true,
		);
		expect(inline).toHaveLength(1);
		expect(inline[0]?.from).toBeGreaterThan(state.doc.line(3).from);
	});

	test('clamps embeds surrounded by whitespace to the whole line', () => {
		const doc = '  ![[Note]]  \n';
		const { state, ranges } = decorate(doc, doc.length);
		const blocks = blockTokens(ranges);
		expect(blocks).toHaveLength(1);
		expect(blocks[0]?.from).toBe(state.doc.line(1).from);
		expect(blocks[0]?.to).toBe(state.doc.line(1).to);
	});

	test('leaves Obsidian syntax inside inline code spans literal', () => {
		const doc = 'Fenced-looking `[[Note]] $x$ ==h== #tag %%c%%` prose\n';
		const { ranges } = decorate(doc, doc.length - 1);
		// Only the code span itself and its hidden backticks.
		const notCode = ranges.filter(
			(item) =>
				item.spec.class !== 'cm-md-code' &&
				!(item.spec.class === undefined && item.spec.widget === undefined),
		);
		expect(notCode).toHaveLength(0);
		// The code span itself is still marked for styling.
		expect(ranges.some((item) => item.spec.class === 'cm-md-code')).toBe(true);
	});

	test('drops markup decorations nested inside hidden comments', () => {
		const doc = 'before %%secret **bold**%% after\n';
		const { ranges } = decorate(doc, doc.length);
		expect(ranges).toHaveLength(1);
		expect(ranges[0]?.spec).toEqual({});
	});

	test('still renders inline math outside code spans', () => {
		const doc = '$x$ here\n';
		const { ranges } = decorate(doc, doc.length);
		expect(ranges).toHaveLength(1);
		expect(ranges[0]?.spec.widget).toBeInstanceOf(StubWidget);
		expect(ranges[0]?.spec.block).toBeUndefined();
	});
});

const widgetTokens = (ranges: DecoratedRange[]) =>
	ranges
		.map((item) => item.spec.widget)
		.filter((widget): widget is StubWidget => widget instanceof StubWidget)
		.map((widget) => widget.token);

const lineClasses = (
	state: EditorState,
	ranges: DecoratedRange[],
	line: number,
) =>
	ranges
		.filter(
			(item) =>
				item.from === item.to && item.from === state.doc.line(line).from,
		)
		.map((item) => item.spec.class);

const hiddenRanges = (ranges: DecoratedRange[]) =>
	ranges.filter(
		(item) => item.spec.class === undefined && item.spec.widget === undefined,
	);

describe('frontmatter', () => {
	const doc =
		'---\ntitle: Plan\ntags: [a, "#b"]\nlink: "[[Note]]"\n---\n\nBody #tag\n';

	test('styles the block and leaves its keys undecorated', () => {
		const { state, ranges } = decorate(doc, doc.length);
		for (const line of [1, 2, 3, 4, 5]) {
			expect(lineClasses(state, ranges, line)).toEqual(['cm-md-frontmatter']);
		}
		const inside = ranges.filter(
			(item) => item.from < state.doc.line(5).to && item.from !== item.to,
		);
		expect(inside).toHaveLength(0);
		expect(widgetTokens(ranges)).toEqual([]);
		expect(ranges.some((item) => item.spec.class === 'cm-md-tag')).toBe(true);
	});

	test('an unclosed opening rule is not frontmatter', () => {
		const { state, ranges } = decorate('---\ntext\n', 0);
		expect(lineClasses(state, ranges, 2)).not.toContain('cm-md-frontmatter');
	});
});

describe('display math', () => {
	test('renders a multi-line $$ block as one block widget', () => {
		const doc = 'Intro\n\n$$\nx^2 + y^2\n$$\n\nOutro\n';
		const { state, ranges } = decorate(doc, doc.length);
		const blocks = blockTokens(ranges);
		expect(blocks).toHaveLength(1);
		expect(blocks[0]?.from).toBe(state.doc.line(3).from);
		expect(blocks[0]?.to).toBe(state.doc.line(5).to);
		expect(widgetTokens(ranges)).toEqual(['math:true:x^2 + y^2']);
	});

	test('keeps emphasis inside math literal', () => {
		const doc = '$$\na*b*c #x\n$$\n\nafter\n';
		const { ranges } = decorate(doc, doc.length);
		expect(ranges.some((item) => item.spec.class === 'cm-md-emphasis')).toBe(
			false,
		);
	});
});

describe('blocks', () => {
	test('styles every line of a blockquote, not only the first', () => {
		const doc = '> one\n> two\n> three\n\nafter\n';
		const { state, ranges } = decorate(doc, doc.length);
		for (const line of [1, 2, 3])
			expect(lineClasses(state, ranges, line)).toContain('cm-md-quote-line');
	});

	test('styles every line of a callout and hides its markers', () => {
		const doc = '> [!note] Title\n> body\n\nafter\n';
		const { state, ranges } = decorate(doc, doc.length);
		expect(lineClasses(state, ranges, 1)).toContain('cm-md-callout');
		expect(lineClasses(state, ranges, 2)).toContain('cm-md-callout');
		const hidden = hiddenRanges(ranges).map((item) => [
			item.from,
			state.sliceDoc(item.from, item.to),
		]);
		// Both `>` markers, and the brackets around the type.
		expect(hidden).toEqual([
			[0, '> '],
			[2, '[!'],
			[8, ']'],
			[16, '> '],
		]);
		// The type shows as a label.
		expect(
			ranges.some(
				(item) =>
					item.spec.class === 'cm-md-callout-label' &&
					state.sliceDoc(item.from, item.to) === 'note',
			),
		).toBe(true);
	});

	test('shows a callout’s source while the caret is in it', () => {
		const doc = '> [!warning]- Careful\n> body\n\nafter\n';
		const { ranges } = decorate(doc, doc.indexOf('body') + 2);
		expect(hiddenRanges(ranges)).toHaveLength(0);
	});

	test('marks fenced code lines for styling', () => {
		const doc = '```js\nconst a = 1;\n```\n\nafter\n';
		const { state, ranges } = decorate(doc, doc.length);
		for (const line of [1, 2, 3])
			expect(lineClasses(state, ranges, line)).toContain('cm-md-codeblock');
	});
});

describe('inline syntax', () => {
	test('reveals the markers of the emphasis the caret is in, however long', () => {
		const long = '**' + 'x'.repeat(120) + '**';
		expect(hiddenRanges(decorate(long + '\n', 60).ranges)).toHaveLength(0);
		const outside = decorate(long + '\n\nnext\n', long.length + 3).ranges;
		expect(hiddenRanges(outside)).toHaveLength(2);
	});

	test('renders images with a title and Obsidian image embeds', () => {
		const doc = '![Alt](pic.png "A title")\n\n![[photo.jpg|300]]\n\nend\n';
		const { ranges } = decorate(doc, doc.length);
		expect(widgetTokens(ranges)).toEqual([
			'image:pic.png:Alt::A title',
			'image:photo.jpg:photo.jpg:300:',
		]);
	});

	test('hides markdown link syntax and keeps the label clickable', () => {
		const doc = 'See [the docs](https://example.com) now\n';
		const { state, ranges } = decorate(doc, doc.length);
		const label = ranges.find((item) =>
			item.spec.class?.includes('cm-md-link-rendered'),
		);
		expect(label && state.sliceDoc(label.from, label.to)).toBe('the docs');
	});
});

describe('tasks', () => {
	test('renders checkboxes for bullet, ordered, and empty tasks', () => {
		const doc =
			'- [ ] one\n* [x] two\n+ [ ] three\n1. [X] four\n- [ ]\n\nend\n';
		const { ranges } = decorate(doc, doc.length);
		expect(widgetTokens(ranges)).toEqual([
			'checkbox:false',
			'checkbox:true',
			'checkbox:false',
			'checkbox:true',
			'checkbox:false',
		]);
	});

	test('shows the raw box while the caret is on it', () => {
		const { ranges } = decorate('- [ ] one\n', 3);
		expect(widgetTokens(ranges)).toEqual([]);
	});
});
