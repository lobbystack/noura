import { getMigrations } from 'better-auth/db/migration';
import { config } from './config';
import { createAuth } from './auth';
import { SyncStore } from './store';
import { retryReady } from './startup';

const settings = config();
const store = new SyncStore(settings.databaseUrl);
const identity = createAuth(settings);
try {
	// Pre-deploy migrations run before the container is healthchecked, and the
	// managed database may still be waking. Retry instead of failing the deploy.
	await retryReady(() => store.migrate(), {
		onRetry: (error, attempt, delayMs) =>
			console.warn(
				`Database not ready for migrations (attempt ${attempt}): ${
					error instanceof Error ? error.message : 'unknown error'
				}; retrying in ${delayMs}ms`,
			),
	});
	await retryReady(
		() =>
			store.transaction(async (tx) => {
				await tx`SELECT pg_advisory_xact_lock(192837466)`;
				const migration = await getMigrations(identity.auth.options);
				await migration.runMigrations();
			}),
		{
			onRetry: (error, attempt, delayMs) =>
				console.warn(
					`Auth migrations not ready (attempt ${attempt}): ${
						error instanceof Error ? error.message : 'unknown error'
					}; retrying in ${delayMs}ms`,
				),
		},
	);
} finally {
	await identity.close();
	await store.close();
}
