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
	const callbackURL = accountCallbackURL(code, invite);
	const result = await client().signIn.magicLink({
		email: email.trim(),
		callbackURL,
	});
	checked(result.error);
}
export function accountCallbackPath(code: string, invite = ''): string {
	if (userCode(code)) return devicePath(code);
	// The account form retains a validated path, while URL query input is a token.
	const token = invite.startsWith('/invite/')
		? invite.slice('/invite/'.length)
		: invite;
	return invitationPath(token) || '/account';
}
function accountCallbackURL(code: string, invite = ''): string {
	return new URL(accountCallbackPath(code, invite), window.location.origin)
		.href;
}
async function signupRequest<T>(path: string, body: object): Promise<T> {
	const response = await fetch(`/api/auth/passkey-sign-up/${path}`, {
		method: 'POST',
		credentials: 'same-origin',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(body),
	});
	const value = (await response.json()) as T & {
		message?: string;
		error?: { message?: string };
	};
	if (!response.ok)
		throw new Error(
			value.message ||
				value.error?.message ||
				'Passkey signup could not be completed.',
		);
	return value;
}
export async function signUpPasskey(
	email: string,
	code: string,
	invite = '',
): Promise<void> {
	const { context } = await signupRequest<{ context: string }>('start', {
		email: email.trim(),
		callbackURL: accountCallbackURL(code, invite),
	});
	const result = await client().passkey.addPasskey({
		name: 'Noura account',
		authenticatorAttachment: 'platform',
		context,
	});
	checked(result?.error);
	if (!result.data?.credentialID)
		throw new Error('The browser did not return the new passkey.');
	await signupRequest('complete', {
		context,
		credentialID: result.data.credentialID,
	});
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
