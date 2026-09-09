import { createHash } from 'node:crypto';
import { APIError, createAuthEndpoint, formCsrfMiddleware } from 'better-auth/api';
import { setSessionCookie } from 'better-auth/cookies';
import { generateRandomString } from 'better-auth/crypto';
import * as z from 'zod';

const expiresInSeconds = 600;

type SignupContext = {
	email: string;
	callbackURL: string;
};

type Activation = SignupContext & {
	credentialID: string;
	userId: string;
};

function digest(value: string): string {
	return createHash('sha256').update(value).digest('base64url');
}

function contextIdentifier(token: string): string {
	return `noura-passkey-signup:${digest(token)}`;
}

function activationIdentifier(token: string): string {
	return `noura-passkey-activation:${digest(token)}`;
}

export function magicLinkPendingIdentifier(email: string): string {
	return `noura-magic-link-pending:${digest(email.trim().toLowerCase())}`;
}

function parse<T>(value: string): T {
	try {
		return JSON.parse(value) as T;
	} catch {
		throw new APIError('BAD_REQUEST', { message: 'This signup has expired.' });
	}
}

export function passkeySignup(options: {
	origin: string;
	allowedEmails: Set<string>;
	sendActivation: (email: string, url: string) => Promise<void>;
}) {
	function callbackURL(value: string | undefined): string {
		const url = new URL(value || '/account', options.origin);
		if (url.origin !== options.origin)
			throw new APIError('BAD_REQUEST', { message: 'Invalid callback URL.' });
		return url.toString();
	}

	return {
		id: 'noura-passkey-signup',
		endpoints: {
			startPasskeySignup: createAuthEndpoint(
				'/passkey-sign-up/start',
				{
					method: 'POST',
					use: [formCsrfMiddleware],
					body: z.object({
						email: z.email(),
						callbackURL: z.string().optional(),
					}),
				},
				async (ctx) => {
					const email = ctx.body.email.trim().toLowerCase();
					if (!options.allowedEmails.has(email))
						throw new APIError('FORBIDDEN', {
							message: 'This email is not invited to this server.',
						});
					const existing = await ctx.context.internalAdapter.findUserByEmail(email);
					if (existing?.user.emailVerified)
						throw new APIError('CONFLICT', {
							message: 'An account already exists for this email. Sign in instead.',
						});
					const pendingMagicLink =
						await ctx.context.internalAdapter.findVerificationValue(
							magicLinkPendingIdentifier(email),
						);
					if (pendingMagicLink && pendingMagicLink.expiresAt > new Date())
						throw new APIError('CONFLICT', {
							message: 'Use the sign-in link already sent to this email.',
						});
					const context = generateRandomString(32);
					await ctx.context.internalAdapter.createVerificationValue({
						identifier: contextIdentifier(context),
						value: JSON.stringify({
							email,
							callbackURL: callbackURL(ctx.body.callbackURL),
						} satisfies SignupContext),
						expiresAt: new Date(Date.now() + expiresInSeconds * 1000),
					});
					return ctx.json({ context });
				},
			),
			completePasskeySignup: createAuthEndpoint(
				'/passkey-sign-up/complete',
				{
					method: 'POST',
					use: [formCsrfMiddleware],
					body: z.object({ context: z.string(), credentialID: z.string() }),
				},
				async (ctx) => {
					const pending = await ctx.context.internalAdapter.consumeVerificationValue(
						contextIdentifier(ctx.body.context),
					);
					if (!pending)
						throw new APIError('BAD_REQUEST', {
							message: 'This signup has expired. Start again.',
						});
					const signup = parse<SignupContext>(pending.value);
					const passkey = await ctx.context.adapter.findOne<{
						credentialID: string;
						userId: string;
					}>({
						model: 'passkey',
						where: [{ field: 'credentialID', value: ctx.body.credentialID }],
					});
					const user = passkey
						? await ctx.context.internalAdapter.findUserById(passkey.userId)
						: null;
					if (!passkey || !user || user.email.toLowerCase() !== signup.email)
						throw new APIError('BAD_REQUEST', {
							message: 'The passkey could not be linked to this signup.',
						});
					const token = generateRandomString(32);
					await ctx.context.internalAdapter.createVerificationValue({
						identifier: activationIdentifier(token),
						value: JSON.stringify({
							...signup,
							credentialID: passkey.credentialID,
							userId: user.id,
						} satisfies Activation),
						expiresAt: new Date(Date.now() + expiresInSeconds * 1000),
					});
					const url = new URL('/api/auth/passkey-sign-up/activate', options.origin);
					url.searchParams.set('token', token);
					await options.sendActivation(user.email, url.toString());
					return ctx.json({ status: true });
				},
			),
			activatePasskeySignup: createAuthEndpoint(
				'/passkey-sign-up/activate',
				{
					method: 'GET',
					query: z.object({ token: z.string() }),
				},
				async (ctx) => {
					const pending = await ctx.context.internalAdapter.consumeVerificationValue(
						activationIdentifier(ctx.query.token),
					);
					if (!pending)
						throw new APIError('BAD_REQUEST', {
							message: 'This activation link has expired.',
						});
					const activation = parse<Activation>(pending.value);
					const passkeys = await ctx.context.adapter.findMany<{
						id: string;
						credentialID: string;
					}>({
						model: 'passkey',
						where: [{ field: 'userId', value: activation.userId }],
					});
					if (!passkeys.some((passkey) => passkey.credentialID === activation.credentialID))
						throw new APIError('BAD_REQUEST', {
							message: 'The registered passkey is no longer available.',
						});
					for (const passkey of passkeys) {
						if (passkey.credentialID !== activation.credentialID)
							await ctx.context.adapter.delete({
								model: 'passkey',
								where: [{ field: 'id', value: passkey.id }],
							});
					}
					const user = await ctx.context.internalAdapter.updateUser(activation.userId, {
						emailVerified: true,
					});
					const session = await ctx.context.internalAdapter.createSession(user.id);
					await setSessionCookie(ctx, { session, user });
					throw ctx.redirect(activation.callbackURL);
				},
			),
		},
	};
}

export async function resolvePasskeySignupUser(
	ctx: {
		context: {
			internalAdapter: {
				findVerificationValue(identifier: string): Promise<{
					value: string;
					expiresAt: Date;
				} | null>;
				findUserByEmail(email: string): Promise<{
					user: {
						id: string;
						email: string;
						emailVerified: boolean;
						name: string;
					};
				} | null>;
				createUser(
					user: { email: string; emailVerified: boolean; name: string },
					source: { method: string },
				): Promise<{ id: string; email: string; name: string }>;
			};
		};
	},
	context: string | null | undefined,
) {
	if (!context)
		throw new APIError('BAD_REQUEST', { message: 'Signup context is required.' });
	const pending = await ctx.context.internalAdapter.findVerificationValue(
		contextIdentifier(context),
	);
	if (!pending || pending.expiresAt <= new Date())
		throw new APIError('BAD_REQUEST', { message: 'This signup has expired.' });
	const signup = parse<SignupContext>(pending.value);
	const existing = await ctx.context.internalAdapter.findUserByEmail(signup.email);
	if (existing?.user.emailVerified)
		throw new APIError('CONFLICT', {
			message: 'An account already exists for this email. Sign in instead.',
		});
	const user =
		existing?.user ??
		(await ctx.context.internalAdapter.createUser(
			{
				email: signup.email,
				emailVerified: false,
				name: signup.email.split('@')[0] || 'Noura user',
			},
			{ method: 'passkey' },
		));
	return { id: user.id, name: user.email, displayName: user.name || user.email };
}

export async function requireVerifiedPasskeyUser(ctx: {
	body: { response: { id?: string } };
	context: {
		adapter: {
			findOne<T>(input: unknown): Promise<T | null>;
		};
		internalAdapter: {
			findUserById(id: string): Promise<{ emailVerified: boolean } | null>;
		};
	};
}) {
	const credentialID = ctx.body.response.id;
	const passkey = credentialID
		? await ctx.context.adapter.findOne<{ userId: string }>({
				model: 'passkey',
				where: [{ field: 'credentialID', value: credentialID }],
			})
		: null;
	const user = passkey
		? await ctx.context.internalAdapter.findUserById(passkey.userId)
		: null;
	if (!user?.emailVerified)
		throw new APIError('FORBIDDEN', {
			message: 'Verify your email before signing in with this passkey.',
		});
}
