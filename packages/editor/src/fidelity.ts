const BOM = '﻿';

export interface LineMetadata {
	usesCrlf: boolean;
	hasBom: boolean;
}

/**
 * Normalize editor text to LF while remembering the original conventions.
 * Raw files keep their CRLF and BOM conventions on disk; the editor works
 * exclusively with LF text.
 */
export function preserveLineMetadata(raw: string): {
	text: string;
	metadata: LineMetadata;
} {
	const hasBom = raw.startsWith(BOM);
	const withoutBom = hasBom ? raw.slice(1) : raw;
	const usesCrlf = withoutBom.includes('\r\n');
	const text = withoutBom.replace(/\r\n/g, '\n');
	return { text, metadata: { usesCrlf, hasBom } };
}

/** Compose raw text back with the file's original CRLF/BOM conventions. */
export function composeRawText(text: string, metadata: LineMetadata): string {
	const body = metadata.usesCrlf ? text.replace(/\n/g, '\r\n') : text;
	return metadata.hasBom ? BOM + body : body;
}

/**
 * Conservative shrink guard for remote merge inputs: an in-flight merged body
 * that collapses a large document without matching user deletions is rejected.
 */
export function detectSuspiciousShrink(
	before: string,
	after: string,
	threshold = 0.35,
): boolean {
	return before.length >= 200 && after.length < before.length * (1 - threshold);
}
