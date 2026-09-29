import { describe, expect, test } from 'bun:test';
import { StreamingMarkdown, stableBlockEnd } from './streaming-markdown';

describe('stableBlockEnd', () => {
	test('ends after the last blank line', () => {
		const text = 'First paragraph.\n\nSecond, still stream';
		expect(text.slice(0, stableBlockEnd(text))).toBe('First paragraph.\n\n');
	});

	test('keeps an open code fence unstable, blank lines included', () => {
		const text = 'Intro.\n\n```ts\nconst a = 1;\n\nconst b = 2;\n';
		expect(stableBlockEnd(text)).toBe('Intro.\n\n'.length);
	});

	test('resumes after the fence closes', () => {
		const text = '```\ncode\n\nmore\n```\n\nAfter';
		expect(text.slice(stableBlockEnd(text))).toBe('After');
	});

	test('a shorter or different marker does not close a fence', () => {
		const text = '````\n```\n\nstill code\n';
		expect(stableBlockEnd(text)).toBe(0);
		expect(stableBlockEnd('~~~\n```\n\nx\n')).toBe(0);
	});

	test('nothing is stable before the first blank line', () => {
		expect(stableBlockEnd('One line')).toBe(0);
		expect(stableBlockEnd('')).toBe(0);
	});
});

describe('StreamingMarkdown', () => {
	test('renders each finished block once', () => {
		const rendered: string[] = [];
		const stream = new StreamingMarkdown((markdown) => {
			rendered.push(markdown);
			return `<p>${markdown.trim()}</p>`;
		});
		expect(stream.update('Hel')).toEqual({ html: '', tail: 'Hel' });
		expect(stream.update('Hello.\n\nWor')).toEqual({
			html: '<p>Hello.</p>',
			tail: 'Wor',
		});
		expect(stream.update('Hello.\n\nWorld')).toEqual({
			html: '<p>Hello.</p>',
			tail: 'World',
		});
		expect(stream.update('Hello.\n\nWorld.\n\n')).toEqual({
			html: '<p>Hello.</p><p>World.</p>',
			tail: '',
		});
		expect(rendered).toEqual(['Hello.\n\n', 'World.\n\n']);
	});

	test('starts over when the text is replaced', () => {
		const stream = new StreamingMarkdown((markdown) => `[${markdown.trim()}]`);
		stream.update('One.\n\nTwo');
		expect(stream.update('Other.\n\n')).toEqual({ html: '[Other.]', tail: '' });
	});
});
