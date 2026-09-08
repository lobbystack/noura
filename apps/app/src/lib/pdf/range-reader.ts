import type { PdfInfo, PdfRangeInput } from '@noura/workspace';

export const PDF_CHUNK_SIZE = 256 * 1024;
const MAX_NATIVE_READ = 1024 * 1024;

/** PDF.js can coalesce ranges. Split those requests before crossing IPC and
 * bound concurrent reads; stopping drops queued work and ignores late replies. */
export class PdfRangeReader {
	private queue: { begin: number; end: number }[] = [];
	private active = 0;
	private stopped = false;
	constructor(
		private info: PdfInfo,
		private read: (input: PdfRangeInput) => Promise<Uint8Array>,
		private ondata: (begin: number, bytes: Uint8Array) => void,
		private onerror: (error: unknown) => void,
	) {}
	request(begin: number, end: number) {
		if (this.stopped) return;
		if (
			!Number.isSafeInteger(begin) ||
			!Number.isSafeInteger(end) ||
			begin < 0 ||
			end <= begin ||
			end > this.info.length
		) {
			this.fail(new Error('Invalid PDF byte range'));
			return;
		}
		this.queue.push({ begin, end });
		this.pump();
	}
	abort() {
		this.stopped = true;
		this.queue.length = 0;
	}
	private fail(error: unknown) {
		if (this.stopped) return;
		this.abort();
		this.onerror(error);
	}
	private pump() {
		while (!this.stopped && this.active < 2 && this.queue.length) {
			const range = this.queue.shift()!;
			this.active++;
			void this.fetch(range.begin, range.end)
				.catch((error) => this.fail(error))
				.finally(() => {
					this.active--;
					this.pump();
				});
		}
	}
	private async fetch(begin: number, end: number) {
		// PDF.js expects one complete response for each requested interval.
		const bytes =
			end - begin > MAX_NATIVE_READ ? new Uint8Array(end - begin) : undefined;
		for (let offset = begin; offset < end && !this.stopped;) {
			const length = Math.min(MAX_NATIVE_READ, end - offset);
			const chunk = await this.read({
				relativePath: this.info.relativePath,
				workspaceId: this.info.workspaceId,
				version: this.info.version,
				offset,
				length,
			});
			if (this.stopped) return;
			if (chunk.length !== length) throw new Error('Incomplete PDF byte range');
			if (!bytes) {
				this.ondata(begin, chunk);
				return;
			}
			bytes.set(chunk, offset - begin);
			offset += length;
		}
		if (!this.stopped && bytes) this.ondata(begin, bytes);
	}
}
