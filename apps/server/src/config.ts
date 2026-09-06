export interface Config {
	databaseUrl: string;
	origin: string;
	port: number;
	host: string;
	authSecret: string;
	smtpUrl: string;
	mailFrom: string;
	allowedEmails: Set<string>;
}

export function config(
	env: Record<string, string | undefined> = process.env,
): Config {
	const required = (name: string) => {
		const value = env[name];
		if (!value) throw new Error(`Missing ${name}`);
		return value;
	};
	const origin = new URL(required('PUBLIC_ORIGIN'));
	if (
		!['http:', 'https:'].includes(origin.protocol) ||
		origin.origin !== env.PUBLIC_ORIGIN ||
		(origin.protocol !== 'https:' &&
			!['localhost', '127.0.0.1'].includes(origin.hostname))
	)
		throw new Error(
			'PUBLIC_ORIGIN must be an HTTPS origin (HTTP is allowed on loopback only)',
		);
	const authSecret = required('AUTH_SECRET');
	if (authSecret.length < 32)
		throw new Error('AUTH_SECRET must contain at least 32 random characters');
	const port = Number(env.PORT ?? 1900);
	if (!Number.isInteger(port) || port < 1 || port > 65535)
		throw new Error('Invalid PORT');
	return {
		databaseUrl: required('DATABASE_URL'),
		origin: origin.origin,
		port,
		host: env.HOST ?? '127.0.0.1',
		authSecret,
		smtpUrl: required('SMTP_URL'),
		mailFrom: required('MAIL_FROM'),
		allowedEmails: new Set(
			(env.ALLOWED_EMAILS ?? '')
				.split(',')
				.map((e) => e.trim().toLowerCase())
				.filter(Boolean),
		),
	};
}
