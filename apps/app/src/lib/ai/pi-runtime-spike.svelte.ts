import { createNativePiRuntimeSpike } from '@noura/ai';
import { createTauriPiRuntimeSpikeTransport } from './pi-runtime-spike-transport';

function assistantText(message: unknown) {
	if (!message || typeof message !== 'object' || !('content' in message))
		return '';
	const { content } = message;
	if (!Array.isArray(content)) return '';
	return content
		.filter((part): part is { type: 'text'; text: string } =>
			Boolean(
				part &&
				typeof part === 'object' &&
				'type' in part &&
				part.type === 'text' &&
				'text' in part &&
				typeof part.text === 'string',
			),
		)
		.map((part) => part.text)
		.join('');
}

class PiRuntimeSpikeStore {
	#agent = createNativePiRuntimeSpike(createTauriPiRuntimeSpikeTransport());
	text = $state('');
	status = $state<'idle' | 'streaming' | 'completed' | 'cancelled' | 'failed'>(
		'idle',
	);

	constructor() {
		this.#agent.subscribe((event) => {
			if (event.type === 'agent_start') this.status = 'streaming';
			if (event.type === 'message_update')
				this.text = assistantText(event.message);
			if (event.type === 'message_end')
				this.text = assistantText(event.message);
			if (event.type === 'agent_end') {
				this.status = this.#agent.state.errorMessage
					? this.#agent.state.errorMessage.includes('cancel')
						? 'cancelled'
						: 'failed'
					: 'completed';
			}
		});
	}

	get running() {
		return this.status === 'streaming';
	}

	async run() {
		if (this.running) return;
		this.#agent.reset();
		this.text = '';
		this.status = 'streaming';
		await this.#agent.prompt('Verify the native Tauri Channel stream.');
	}

	stop() {
		if (this.running) this.#agent.abort();
	}
}

export const piRuntimeSpike = new PiRuntimeSpikeStore();
