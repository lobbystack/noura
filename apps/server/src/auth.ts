import { betterAuth } from 'better-auth';
import { bearer, deviceAuthorization, magicLink } from 'better-auth/plugins';
import { passkey } from '@better-auth/passkey';
import { Pool } from 'pg';
import nodemailer from 'nodemailer';
import type { Config } from './config';
import { sendWithResend } from './mail';
import {
	magicLinkPendingIdentifier,
	passkeySignup,
	requireVerifiedPasskeyUser,
	resolvePasskeySignupUser,
} from './passkey-signup';

export function createAuth(
	config: Config,
	deliver?: (email: string, url: string) => Promise<void>,
) {
	const pool = new Pool({ connectionString: config.databaseUrl, max: 5 });
	const mail = config.smtpUrl
		? nodemailer.createTransport(config.smtpUrl)
		: undefined;
	const sendMail = async (email: string, subject: string, text: string) => {
		if (config.resendApiKey) {
			await sendWithResend(config.resendApiKey, {
				from: config.mailFrom,
				to: email,
				subject,
				text,
			});
			return;
		}
		if (!mail) throw new Error('No mail transport configured');
		await mail.sendMail({ from: config.mailFrom, to: email, subject, text });
	};
	const sendLink = async (
		email: string,
		url: string,
		subject: string,
		message: string,
	) => {
		if (deliver) return deliver(email, url);
		await sendMail(email, subject, `${message}\n\n${url}`);
	};
	const auth = betterAuth({
		database: pool,
		secret: config.authSecret,
		baseURL: config.origin,
		trustedOrigins: [config.origin],
		emailAndPassword: { enabled: false },
		rateLimit: { enabled: true, storage: 'database', window: 60, max: 30 },
		session: { expiresIn: 60 * 60 * 24 * 7, cookieCache: { enabled: false } },
		databaseHooks: {
			user: {
				create: {
					before: async (user) => {
						if (!config.allowedEmails.has(user.email.toLowerCase()))
							return false;
						return { data: user };
					},
				},
			},
		},
		plugins: [
			bearer(),
			deviceAuthorization({
				verificationUri: `${config.origin}/account/device`,
				validateClient: (clientId) => clientId === 'noura-desktop',
				expiresIn: '10m',
				interval: '5s',
			}),
			passkey({
				rpID: new URL(config.origin).hostname,
				rpName: 'Noura',
				origin: config.origin,
				registration: {
					requireSession: false,
					resolveUser: ({ ctx, context }) =>
						resolvePasskeySignupUser(ctx, context),
				},
				authentication: {
					afterVerification: ({ ctx }) => requireVerifiedPasskeyUser(ctx),
				},
			}),
			passkeySignup({
				origin: config.origin,
				allowedEmails: config.allowedEmails,
				sendActivation: (email, url) =>
					sendLink(
						email,
						url,
						'Verify your Noura account',
						'Finish creating your Noura account using this link. It expires in 10 minutes.',
					),
			}),
			magicLink({
				storeToken: 'hashed',
				expiresIn: 600,
				sendMagicLink: async ({ email, url }, ctx) => {
					const normalizedEmail = email.toLowerCase();
					if (!config.allowedEmails.has(normalizedEmail) || !ctx) return;
					const existing =
						await ctx.context.internalAdapter.findUserByEmail(normalizedEmail);
					if (existing && !existing.user.emailVerified) {
						const passkeys = await ctx.context.adapter.findMany({
							model: 'passkey',
							where: [{ field: 'userId', value: existing.user.id }],
							limit: 1,
						});
						if (passkeys.length) return;
					}
					const marker = magicLinkPendingIdentifier(normalizedEmail);
					await ctx.context.internalAdapter.deleteVerificationByIdentifier(
						marker,
					);
					await ctx.context.internalAdapter.createVerificationValue({
						identifier: marker,
						value: normalizedEmail,
						expiresAt: new Date(Date.now() + 600_000),
					});
					try {
						await sendLink(
							email,
							url,
							'Sign in to Noura',
							'Sign in to Noura using this link. It expires in 10 minutes.',
						);
					} catch (error) {
						await ctx.context.internalAdapter.deleteVerificationByIdentifier(
							marker,
						);
						throw error;
					}
				},
			}),
		],
	});
	return {
		auth,
		close: async () => {
			mail?.close();
			await pool.end();
		},
	};
}

export type AccountAuth = ReturnType<typeof createAuth>['auth'];
