export interface OutboundMail {
	from: string;
	to: string;
	subject: string;
	text: string;
}

const RESEND_ENDPOINT = 'https://api.resend.com/emails';

/**
 * Deliver mail through Resend's HTTPS API. The SMTP ports this replaces are
 * blocked on most container platforms, so this is the portable delivery path.
 * Provider error text is written to local diagnostics, never raised to callers,
 * so a mail failure cannot leak provider details into an HTTP response.
 */
export async function sendWithResend(
	apiKey: string,
	message: OutboundMail,
	fetchImpl: typeof fetch = fetch,
) {
	const response = await fetchImpl(RESEND_ENDPOINT, {
		method: 'POST',
		headers: {
			Authorization: `Bearer ${apiKey}`,
			'Content-Type': 'application/json',
		},
		body: JSON.stringify({
			from: message.from,
			to: [message.to],
			subject: message.subject,
			text: message.text,
		}),
	});
	if (!response.ok) {
		const detail = (await response.text().catch(() => '')).slice(0, 500);
		console.error(`Resend delivery failed (${response.status}): ${detail}`);
		throw new Error(`Resend delivery failed (${response.status})`);
	}
}
