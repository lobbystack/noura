import { expect, test } from 'bun:test';
import { createPiRuntimeSpike } from './pi-runtime-spike';

test('Pi Agent Core streams through an injected browser-safe function and executes an in-memory tool', async () => {
	const agent = createPiRuntimeSpike();
	const events: string[] = [];
	agent.subscribe((event) => {
		events.push(event.type);
	});

	await agent.prompt('Run the runtime spike.');
	await agent.waitForIdle();

	expect(events).toContain('tool_execution_start');
	expect(events).toContain('tool_execution_end');
	expect(events).toContain('agent_end');
	expect(agent.state.messages.map((message) => message.role)).toEqual([
		'user',
		'assistant',
		'toolResult',
		'assistant',
	]);

	const finalMessage = agent.state.messages.at(-1);
	expect(finalMessage).toMatchObject({
		role: 'assistant',
		content: [{ type: 'text', text: 'The native tool result was received.' }],
	});
});

test('Pi Agent Core receives cancellation through the injected stream AbortSignal', async () => {
	const agent = createPiRuntimeSpike();
	const run = agent.prompt('Cancel the runtime spike.');
	agent.abort();
	await run;
	await agent.waitForIdle();

	expect(agent.state.isStreaming).toBeFalse();
	expect(agent.state.errorMessage).toBe('The native stream was cancelled.');
});
