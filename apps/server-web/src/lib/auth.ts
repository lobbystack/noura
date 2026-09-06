import { createAuthClient } from 'better-auth/svelte';
import { magicLinkClient } from 'better-auth/client/plugins';
import { passkeyClient } from '@better-auth/passkey/client';

function createClient() {
	return createAuthClient({ plugins: [magicLinkClient(), passkeyClient()] });
}
let cachedClient: ReturnType<typeof createClient> | undefined;
function client() {
	return (cachedClient ??= createClient());
}
export type Account = { id: string; email: string; name: string };
export type DeviceReview = {
	user_code: string;
	status: 'pending' | 'approved' | 'denied';
	client_id?: string;
};
function checked(error: { message?: string } | null | undefined) {
	if (error)
		throw new Error(
			error.message || 'The request could not be completed. Please try again.',
		);
}
export function userCode(value: string | null): string {
	const code = (value ?? '').replace(/[-\s]/g, '').toUpperCase();
	return /^[A-Z0-9]{8}$/.test(code) ? code : '';
}
export function accountPath(code: string): string {
	const normalized = userCode(code);
	return normalized
		? `/account?user_code=${encodeURIComponent(normalized)}`
		: '/account';
}
export function devicePath(code: string): string {
	const normalized = userCode(code);
	return normalized
		? `/account/device?user_code=${encodeURIComponent(normalized)}`
		: '/account/device';
}
export function invitationPath(token: string | null): string {
	return /^[A-Za-z0-9_-]{43}$/.test(token ?? '') ? `/invite/${token}` : '';
}
export async function session(): Promise<Account | null> {
	const result = await client().getSession();
	checked(result.error);
	return result.data?.user ?? null;
}
export async function sendMagicLink(
	email: string,
	code: string,
	invite = '',
): Promise<void> {
	const callbackURL = new URL(
		userCode(code) ? devicePath(code) : invitationPath(invite) || '/account',
		window.location.origin,
	).href;
	const result = await client().signIn.magicLink({
		email: email.trim(),
		callbackURL,
	});
	checked(result.error);
}
export async function signInPasskey(): Promise<void> {
	const result = await client().signIn.passkey();
	checked(result?.error);
}
export async function addPasskey(): Promise<void> {
	const result = await client().passkey.addPasskey({ name: 'Noura account' });
	checked(result?.error);
}
export async function signOut(): Promise<void> {
	const result = await client().signOut();
	checked(result.error);
}
async function deviceRequest(
	path: string,
	body?: { userCode: string },
): Promise<unknown> {
	const response = await fetch(`/api/auth/device${path}`, {
		method: body ? 'POST' : 'GET',
		credentials: 'same-origin',
		cache: 'no-store',
		...(body
			? {
					headers: { 'Content-Type': 'application/json' },
					body: JSON.stringify(body),
				}
			: {}),
	});
	if (!response.ok)
		throw new Error(
			response.status === 401
				? 'Sign in before reviewing this device.'
				: 'This code is expired, already used, or unavailable. Start again in Noura desktop.',
		);
	return response.json();
}
export async function reviewDevice(code: string): Promise<DeviceReview> {
	const normalized = userCode(code);
	if (!normalized)
		throw new Error('Enter the eight-character code shown in Noura desktop.');
	const value = (await deviceRequest(
		`?user_code=${encodeURIComponent(normalized)}`,
	)) as DeviceReview;
	if (
		value.client_id !== 'noura-desktop' ||
		!['pending', 'approved', 'denied'].includes(value.status)
	)
		throw new Error(
			'This request cannot be approved by this account. Start again in Noura desktop.',
		);
	return value;
}
export async function decideDevice(
	code: string,
	decision: 'approve' | 'deny',
): Promise<void> {
	const normalized = userCode(code);
	if (!normalized) throw new Error('Invalid device code.');
	await deviceRequest(`/${decision}`, { userCode: normalized });
}
