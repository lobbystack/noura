// The Rust sync server on Railway. Nothing reads this file yet: `railway.ts`
// still runs the legacy server. At the owner's go-ahead, this file replaces it,
// after the hosted database is wiped, and that step records the go-ahead and
// the wipe. It also needs a server secret: see docs/self-hosting.md.
//
// Railway's health check takes an HTTP path, so it asks `/v2/limits`, the same
// route `sync-server healthcheck` asks.
import {
	defineRailway,
	github,
	postgres,
	preserve,
	project,
	service,
	volume,
} from 'railway/iac';

export default defineRailway(() => {
	const db = postgres('Postgres');

	const data = volume('sync-data', {
		region: 'us-east4-eqdc4a',
		sizeMB: 1024,
	});

	const server = service('server', {
		source: github('lobbystack/noura', { branch: 'main' }),
		build: {
			builder: 'DOCKERFILE',
			dockerfilePath: 'crates/sync-server/Dockerfile',
			watchPatterns: [
				'crates/**',
				'Cargo.lock',
				'Cargo.toml',
				'rust-toolchain.toml',
				'scripts/build-account-web.sh',
				'.dockerignore',
				'.railway/**',
			],
		},
		// The image runs `migrate` before `serve`, once the volume is mounted.
		healthcheck: '/v2/limits',
		healthcheckTimeout: 120,
		deploy: {
			restartPolicyMaxRetries: 5,
		},
		env: {
			NOURA_DATABASE_URL: db.env.DATABASE_URL,
			NOURA_SERVER_SECRET: preserve(),
			NOURA_RESEND_API_KEY: preserve(),
			NOURA_MAIL_FROM: 'Noura <noreply@noura.app>',
			NOURA_ALLOWED_EMAILS: preserve(),
			NOURA_PUBLIC_ORIGIN: 'https://sync.noura.app',
			NOURA_TRUSTED_PROXY_HEADER: 'x-real-ip',
			NOURA_LISTEN: '0.0.0.0:1900',
			NOURA_DATA_DIR: '/data',
			PORT: '1900',
			// Railway mounts volumes as root. The image's entrypoint starts as
			// root only to hand the mount to `noura`, then drops root.
			RAILWAY_RUN_UID: '0',
		},
		volumeMounts: {
			'/data': data,
		},
	});

	return project('noura', {
		resources: [db, data, server],
	});
});
