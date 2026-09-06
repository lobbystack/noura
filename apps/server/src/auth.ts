import { betterAuth } from 'better-auth';
import { bearer, deviceAuthorization, magicLink } from 'better-auth/plugins';
import { passkey } from '@better-auth/passkey';
import { Pool } from 'pg';
import nodemailer from 'nodemailer';
import type { Config } from './config';

export function createAuth(
	config: Config,
	deliver?: (email: string, url: string) => Promise<void>,
) {
	const pool = new Pool({ connectionString: config.databaseUrl, max: 5 });
	const mail = nodemailer.createTransport(config.smtpUrl);
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
			}),
			magicLink({
				storeToken: 'hashed',
				expiresIn: 600,
				sendMagicLink: async ({ email, url }) => {
					if (!config.allowedEmails.has(email.toLowerCase())) return;
					if (deliver) {
						await deliver(email, url);
						return;
					}
					await mail.sendMail({
						from: config.mailFrom,
						to: email,
						subject: 'Sign in to Noura',
						text: `Sign in to Noura using this link. It expires in 10 minutes.\n\n${url}`,
					});
				},
			}),
		],
	});
	return {
		auth,
		close: async () => {
			mail.close();
			await pool.end();
		},
	};
}

export type AccountAuth = ReturnType<typeof createAuth>['auth'];
