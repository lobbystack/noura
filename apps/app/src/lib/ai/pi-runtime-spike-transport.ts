import { Channel, invoke } from '@tauri-apps/api/core';
import type { PiRuntimeSpikeFrame, PiRuntimeSpikeTransport } from '@noura/ai';

export function createTauriPiRuntimeSpikeTransport(): PiRuntimeSpikeTransport {
	return {
		async stream(operationId, onFrame) {
			const channel = new Channel<PiRuntimeSpikeFrame>(onFrame);
			await invoke('pi_runtime_spike_stream', { operationId, channel });
		},
		cancel(operationId) {
			return invoke<boolean>('pi_runtime_spike_cancel', { operationId });
		},
	};
}
