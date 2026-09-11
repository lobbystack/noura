import { describe, expect, test } from 'bun:test';
import { sendWithResend } from './mail';

const message = {
	from: 'Noura <noreply@noura.app>',
	to: 'account@example.invalid',
	subject: 'Sign in to Noura',
	text: 'Sign in using this link.',
};

describe('Resend HTTPS mailer', () => {
	test('posts the expected request', async () => {
		let captured: { url: string; init: RequestInit } | undefined;
		const fetchImpl = (async (
			url: string | URL | Request,
			init?: RequestInit,
		) => {
			captured = { url: String(url), init: init! };
			return new Response('{}', { status: 200 });
		}) as unknown as typeof fetch;

		await sendWithResend('re_test_key', message, fetchImpl);

		expect(captured?.url).toBe('https://api.resend.com/emails');
		const headers = new Headers(captured?.init.headers);
		expect(headers.get('Authorization')).toBe('Bearer re_test_key');
		expect(headers.get('Content-Type')).toBe('application/json');
		expect(JSON.parse(String(captured?.init.body))).toEqual({
			from: message.from,
			to: [message.to],
			subject: message.subject,
			text: message.text,
		});
	});

	test('reports provider failure without leaking the API key', async () => {
		const fetchImpl = (async () =>
			new Response('invalid api key re_test_key', {
				status: 422,
			})) as unknown as typeof fetch;

		const error = await sendWithResend(
			're_secret_key',
			message,
			fetchImpl,
		).catch((caught: unknown) => caught);
		expect(error).toBeInstanceOf(Error);
		expect((error as Error).message).toBe('Resend delivery failed (422)');
		expect((error as Error).message).not.toContain('re_secret_key');
	});
});
