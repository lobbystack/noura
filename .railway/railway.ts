import {
	defineRailway,
	github,
	postgres,
	preserve,
	project,
	service,
	volume,
} from 'railway/iac';

export default defineRailway((ctx) => {
	const production = ctx.isEnvironment('production');

	const db = postgres('Postgres');

	const blobs = volume('server-blobs', { sizeMB: 1024 });

	const server = service('server', {
		source: github('lobbystack/noura', { branch: 'main' }),
		build: {
			builder: 'DOCKERFILE',
			dockerfilePath: 'apps/server/Dockerfile',
			watchPatterns: [
				'apps/server/**',
				'apps/server-web/**',
				'apps/app/**',
				'packages/**',
				'bun.lock',
				'package.json',
				'.railway/**',
			],
		},
		preDeploy: 'bun --no-install migrate.js',
		healthcheck: '/ready',
		healthcheckTimeout: 120,
		deploy: {
			restartPolicyType: 'ON_FAILURE',
			restartPolicyMaxRetries: 5,
		},
		env: {
			DATABASE_URL: db.env.DATABASE_URL,
			AUTH_SECRET: preserve(),
			RESEND_API_KEY: preserve(),
			MAIL_FROM: 'Noura <noreply@noura.app>',
			ALLOWED_EMAILS: 'redacted@example.invalid',
			PUBLIC_ORIGIN: 'https://sync.noura.app',
			PORT: '1900',
			HOST: '0.0.0.0',
			BLOB_ROOT: '/data/blobs',
			RAILWAY_RUN_UID: '0',
		},
		domains: production ? [{ domain: 'sync.noura.app', port: 1900 }] : [],
		volumeMounts: {
			'/data/blobs': blobs,
		},
	});

	return project('noura', {
		resources: [db, blobs, server],
	});
});
