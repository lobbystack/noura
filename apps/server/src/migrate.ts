import { getMigrations } from 'better-auth/db/migration';
import { config } from './config';
import { createAuth } from './auth';
import { SyncStore } from './store';

const settings = config();
const store = new SyncStore(settings.databaseUrl);
const identity = createAuth(settings);
try {
	await store.migrate();
	await store.db.begin(async (tx) => {
		await tx`SELECT pg_advisory_xact_lock(192837466)`;
		const migration = await getMigrations(identity.auth.options);
		await migration.runMigrations();
	});
} finally {
	await identity.close();
	await store.close();
}
