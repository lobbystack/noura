import { Channel, invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { CoreEvent } from '@noura/shared';
import type { CoreTransport } from './index';
import { decodePdfResponse } from './pdf-response';

export function createTauriTransport(): CoreTransport {
	return {
		request: async <T>(
			command: string,
			payload: Record<string, unknown> = {},
		) => {
			if (command === 'files_read_pdf') {
				const buffer = await invoke<ArrayBuffer | number[]>(command, payload);
				return decodePdfResponse(
					buffer instanceof ArrayBuffer
						? buffer
						: new Uint8Array(buffer).buffer,
				) as T;
			}
			return invoke<T>(command, payload);
		},
		stream: async <T>(
			command: string,
			payload: Record<string, unknown>,
			handler: (frame: T) => void,
		) => {
			const channel = new Channel<T>();
			channel.onmessage = handler;
			await invoke(command, { ...payload, channel });
		},
		async subscribe(handler) {
			const unlisten: UnlistenFn = await listen<CoreEvent>(
				'noura://core-event',
				({ payload }) => handler(payload),
			);
			return unlisten;
		},
	};
}
