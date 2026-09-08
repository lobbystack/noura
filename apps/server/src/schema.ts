/** Original Noura schema; auth sessions contain token hashes, never bearer secrets. */
export const schema = `
CREATE TABLE IF NOT EXISTS noura_devices (
 id text PRIMARY KEY, account_id text NOT NULL, public_key text NOT NULL,
 revoked boolean NOT NULL DEFAULT false
);
CREATE TABLE IF NOT EXISTS noura_sessions (
 token_hash text PRIMARY KEY, device_id text NOT NULL REFERENCES noura_devices(id),
 expires_at timestamptz NOT NULL
);
CREATE TABLE IF NOT EXISTS noura_device_challenges (
 token_hash text PRIMARY KEY, account_id text NOT NULL, expires_at timestamptz NOT NULL
);
CREATE TABLE IF NOT EXISTS noura_rate_limits (
 account_id text PRIMARY KEY, window_start bigint NOT NULL, count integer NOT NULL
);
CREATE TABLE IF NOT EXISTS noura_collaboration_limits (
 device_id text NOT NULL REFERENCES noura_devices(id) ON DELETE CASCADE,
 bucket text NOT NULL CHECK(bucket IN ('durable','presence')),
 tokens double precision NOT NULL, updated_at timestamptz NOT NULL,
 PRIMARY KEY(device_id,bucket)
);
CREATE TABLE IF NOT EXISTS noura_workspaces (
 id text PRIMARY KEY, sequence bigint NOT NULL DEFAULT 0,
 quota_bytes bigint NOT NULL DEFAULT 1073741824, used_bytes bigint NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS noura_members (
 workspace_id text REFERENCES noura_workspaces(id), account_id text NOT NULL,
 role text NOT NULL CHECK(role IN ('owner','admin','editor','viewer')),
 PRIMARY KEY(workspace_id, account_id)
);
ALTER TABLE noura_members ADD COLUMN IF NOT EXISTS history_after bigint NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS noura_objects (
 workspace_id text REFERENCES noura_workspaces(id), id text NOT NULL,
 epoch bigint NOT NULL DEFAULT 1,
 PRIMARY KEY(workspace_id, id)
);
ALTER TABLE noura_objects ADD COLUMN IF NOT EXISTS generation text;
ALTER TABLE noura_objects ADD COLUMN IF NOT EXISTS document_mode text;
CREATE TABLE IF NOT EXISTS noura_grants (
 workspace_id text NOT NULL, object_id text NOT NULL, account_id text NOT NULL,
 role text NOT NULL CHECK(role IN ('editor','viewer')),
 PRIMARY KEY(workspace_id, object_id, account_id),
 FOREIGN KEY(workspace_id, object_id) REFERENCES noura_objects(workspace_id,id)
);
ALTER TABLE noura_grants ADD COLUMN IF NOT EXISTS history_after bigint NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS noura_operations (
 workspace_id text NOT NULL, operation_id text NOT NULL, object_id text NOT NULL,
 device_id text NOT NULL REFERENCES noura_devices(id), sequence bigint NOT NULL,
 epoch bigint NOT NULL, policy_revision bigint NOT NULL DEFAULT 0,
 nonce text NOT NULL, ciphertext text NOT NULL,
 signature text NOT NULL, digest text NOT NULL, payload_bytes integer NOT NULL,
 PRIMARY KEY(workspace_id, operation_id), UNIQUE(workspace_id,sequence),
 FOREIGN KEY(workspace_id,object_id) REFERENCES noura_objects(workspace_id,id)
);
CREATE INDEX IF NOT EXISTS noura_operations_object_seq ON noura_operations(workspace_id,object_id,sequence);
ALTER TABLE noura_operations ADD COLUMN IF NOT EXISTS policy_revision bigint NOT NULL DEFAULT 0;
ALTER TABLE noura_operations ADD COLUMN IF NOT EXISTS generation text;
ALTER TABLE noura_operations ADD COLUMN IF NOT EXISTS kind text;
CREATE TABLE IF NOT EXISTS noura_key_envelopes (
 workspace_id text NOT NULL, object_id text NOT NULL, epoch bigint NOT NULL,
 device_id text REFERENCES noura_devices(id), wrapped_key text NOT NULL,
 PRIMARY KEY(workspace_id, object_id, epoch, device_id),
 FOREIGN KEY(workspace_id,object_id) REFERENCES noura_objects(workspace_id,id)
);
ALTER TABLE noura_workspaces ADD COLUMN IF NOT EXISTS access_revision bigint NOT NULL DEFAULT 0;
ALTER TABLE noura_devices ADD COLUMN IF NOT EXISTS encryption_recipient text;
ALTER TABLE noura_key_envelopes ADD COLUMN IF NOT EXISTS signing_device text REFERENCES noura_devices(id);
ALTER TABLE noura_key_envelopes ADD COLUMN IF NOT EXISTS signature text;
CREATE TABLE IF NOT EXISTS noura_access_log (
 workspace_id text NOT NULL REFERENCES noura_workspaces(id), revision bigint NOT NULL,
 device_id text NOT NULL REFERENCES noura_devices(id), policy jsonb NOT NULL,
 signature text NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(workspace_id, revision)
);
CREATE TABLE IF NOT EXISTS noura_transitions (
 workspace_id text NOT NULL REFERENCES noura_workspaces(id), id text NOT NULL,
 device_id text NOT NULL REFERENCES noura_devices(id), digest text NOT NULL,
 body jsonb NOT NULL, payload_bytes integer NOT NULL, committed boolean NOT NULL DEFAULT false,
 created_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY(workspace_id,id)
);
CREATE TABLE IF NOT EXISTS noura_object_activations (
 workspace_id text NOT NULL REFERENCES noura_workspaces(id), id text NOT NULL,
 object_id text NOT NULL, device_id text NOT NULL REFERENCES noura_devices(id),
 digest text NOT NULL, body jsonb NOT NULL, payload_bytes integer NOT NULL,
 committed boolean NOT NULL DEFAULT false, created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(workspace_id,id)
);
CREATE TABLE IF NOT EXISTS noura_workspace_capabilities (
 workspace_id text PRIMARY KEY REFERENCES noura_workspaces(id),
 device_id text NOT NULL REFERENCES noura_devices(id), capability jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS noura_checkpoints (
 workspace_id text NOT NULL, object_id text NOT NULL, epoch bigint NOT NULL,
 transition_id text NOT NULL, generation text NOT NULL, covered_sequence bigint NOT NULL,
 checkpoint jsonb NOT NULL,
 PRIMARY KEY(workspace_id,object_id,epoch),
 FOREIGN KEY(workspace_id,object_id) REFERENCES noura_objects(workspace_id,id),
 FOREIGN KEY(workspace_id,transition_id) REFERENCES noura_transitions(workspace_id,id)
);
CREATE TABLE IF NOT EXISTS noura_activation_checkpoints (
 workspace_id text NOT NULL, object_id text NOT NULL, activation_id text NOT NULL,
 epoch bigint NOT NULL, generation text NOT NULL, covered_sequence bigint NOT NULL,
 checkpoint jsonb NOT NULL,
 PRIMARY KEY(workspace_id,object_id,epoch),
 FOREIGN KEY(workspace_id,object_id) REFERENCES noura_objects(workspace_id,id),
 FOREIGN KEY(workspace_id,activation_id) REFERENCES noura_object_activations(workspace_id,id)
);
CREATE TABLE IF NOT EXISTS noura_public_links (
 id text PRIMARY KEY, token_hash text NOT NULL UNIQUE,
 workspace_id text NOT NULL, object_id text NOT NULL,
 snapshot jsonb NOT NULL, revision bigint NOT NULL DEFAULT 1,
 payload_bytes integer NOT NULL, expires_at timestamptz NOT NULL,
 revoked boolean NOT NULL DEFAULT false,
 FOREIGN KEY(workspace_id,object_id) REFERENCES noura_objects(workspace_id,id)
);
CREATE TABLE IF NOT EXISTS noura_invitations (
 id text PRIMARY KEY, token_hash text NOT NULL UNIQUE,
 workspace_id text NOT NULL REFERENCES noura_workspaces(id),
 inviter_account_id text NOT NULL,
 role text NOT NULL CHECK(role IN ('admin','editor','viewer')),
 accepted_account_id text,
 expires_at timestamptz NOT NULL,
 accepted_at timestamptz,
 completed_at timestamptz,
 revoked_at timestamptz,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS noura_invitations_workspace_created
 ON noura_invitations(workspace_id,created_at DESC);
CREATE TABLE IF NOT EXISTS noura_blobs (
 workspace_id text NOT NULL, object_id text NOT NULL, id text NOT NULL,
 epoch bigint NOT NULL, size bigint NOT NULL CHECK(size>0),
 upload_id text NOT NULL UNIQUE, device_id text NOT NULL REFERENCES noura_devices(id),
 tus_info jsonb, complete boolean NOT NULL DEFAULT false, failed boolean NOT NULL DEFAULT false,
 storage text NOT NULL DEFAULT 'local' CHECK(storage IN ('local','s3')),
 transition_id text, activation_id text,
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(workspace_id,object_id,id),
 CHECK(transition_id IS NULL OR activation_id IS NULL)
);
ALTER TABLE noura_blobs ADD COLUMN IF NOT EXISTS transition_id text;
ALTER TABLE noura_blobs ADD COLUMN IF NOT EXISTS activation_id text;
ALTER TABLE noura_blobs DROP CONSTRAINT IF EXISTS noura_blobs_workspace_id_object_id_fkey;
ALTER TABLE noura_object_activations DROP CONSTRAINT IF EXISTS noura_object_activations_workspace_id_object_id_key;
`;
