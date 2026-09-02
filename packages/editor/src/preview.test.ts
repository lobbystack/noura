import { describe, expect, test } from 'bun:test';
import { EditorSelection, EditorState } from '@codemirror/state';
import type { Decoration as DecorationInstance } from '@codemirror/view';
import { Decoration, WidgetType } from '@codemirror/view';
import { markdown, markdownLanguage } from '@codemirror/lang-markdown';
import { GFM } from '@lezer/markdown';
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
		extensions: [markdown({ base: markdownLanguage, extensions: [GFM] })],
	} as Parameters<typeof EditorState.create>[0]);
	const set = buildDecorations(
		state,
		[{ from: 0, to: state.doc.length }],
		Decoration,
		{
			image: (alt: string, src: string) => new StubWidget(`image:${src}`),
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
		const notCode = ranges.filter((item) => item.spec.class !== 'cm-md-code');
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
