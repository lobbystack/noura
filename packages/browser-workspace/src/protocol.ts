import type { CoreError, CoreEvent } from '@noura/shared';

export type BrowserWorkerRequest = {
	type: 'request';
	id: string;
	command: string;
	payload: Record<string, unknown>;
};

export type BrowserWorkerResponse =
	| { type: 'response'; id: string; ok: true; value: unknown }
	| { type: 'response'; id: string; ok: false; error: CoreError }
	| { type: 'event'; event: CoreEvent };

export interface BrowserWorkerEndpoint {
	postMessage(message: BrowserWorkerRequest): void;
	addEventListener(
		type: 'message' | 'error' | 'messageerror',
		listener: (event: MessageEvent<BrowserWorkerResponse> | ErrorEvent) => void,
	): void;
	removeEventListener(
		type: 'message' | 'error' | 'messageerror',
		listener: (event: MessageEvent<BrowserWorkerResponse> | ErrorEvent) => void,
	): void;
}

export type BrowserWorkerPort = {
	postMessage(message: BrowserWorkerResponse): void;
	addEventListener(
		type: 'message',
		listener: (event: MessageEvent<BrowserWorkerRequest>) => void,
	): void;
};
