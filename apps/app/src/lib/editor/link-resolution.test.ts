import { describe, expect, test } from 'bun:test';
import {
	linkSuggestions,
	relativeTarget,
	resolveLinkPath,
} from './link-resolution';

const files = [
	'Inbox.md',
	'Projects/Plan.md',
	'Projects/Assets/diagram.png',
	'Archive/Plan.md',
	'Archive/2024/Old notes.md',
	'Reading/paper.pdf',
];

describe('resolveLinkPath', () => {
	test('finds a note by name anywhere in the workspace', () => {
		expect(resolveLinkPath('Inbox.md', 'Old notes', files)).toBe(
			'Archive/2024/Old notes.md',
		);
		expect(resolveLinkPath('Inbox.md', 'diagram.png', files)).toBe(
			'Projects/Assets/diagram.png',
		);
		expect(resolveLinkPath('Inbox.md', 'old NOTES', files)).toBe(
			'Archive/2024/Old notes.md',
		);
	});

	test('prefers the note’s own folder, then the shortest path', () => {
		expect(resolveLinkPath('Archive/2024/Old notes.md', 'Plan', files)).toBe(
			'Archive/Plan.md',
		);
		expect(resolveLinkPath('Projects/Plan.md', 'Plan', files)).toBe(
			'Projects/Plan.md',
		);
	});

	test('follows relative and root paths, headings, and encoded spaces', () => {
		expect(resolveLinkPath('Projects/Plan.md', '../Inbox.md', files)).toBe(
			'Inbox.md',
		);
		expect(
			resolveLinkPath('Projects/Plan.md', 'Archive/Plan#Goals', files),
		).toBe('Archive/Plan.md');
		expect(
			resolveLinkPath('Inbox.md', 'Archive/2024/Old%20notes.md', files),
		).toBe('Archive/2024/Old notes.md');
		expect(resolveLinkPath('Projects/Plan.md', '/Inbox', files)).toBe(
			'Inbox.md',
		);
	});

	test('returns null for missing files, web links, and escapes', () => {
		expect(resolveLinkPath('Inbox.md', 'Nowhere', files)).toBeNull();
		expect(
			resolveLinkPath('Inbox.md', 'https://example.com', files),
		).toBeNull();
		expect(resolveLinkPath('Inbox.md', '../../etc/passwd', files)).toBeNull();
	});
});

describe('relativeTarget', () => {
	test('walks up to the shared folder', () => {
		expect(relativeTarget('Projects/Plan.md', 'Archive/Plan.md')).toBe(
			'../Archive/Plan.md',
		);
		expect(
			relativeTarget('Projects/Plan.md', 'Projects/Assets/diagram.png'),
		).toBe('Assets/diagram.png');
		expect(relativeTarget('Inbox.md', 'Reading/paper.pdf')).toBe(
			'Reading/paper.pdf',
		);
	});
});

describe('linkSuggestions', () => {
	test('uses names, and paths only when names repeat', () => {
		const suggestions = linkSuggestions(files);
		expect(suggestions).toContainEqual({ label: 'Inbox', target: 'Inbox' });
		expect(suggestions).toContainEqual({
			label: 'Plan',
			target: 'Projects/Plan',
			detail: 'Projects',
		});
		expect(suggestions).toContainEqual({
			label: 'paper.pdf',
			target: 'paper.pdf',
			detail: 'Reading',
		});
	});
});
