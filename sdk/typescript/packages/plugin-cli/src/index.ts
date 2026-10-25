// The CLI's commands as functions, for tools and tests that drive them.
export { BuildError, type Built, SOURCE, build, describe } from './build.js';
export { type Dev, dev } from './dev.js';
export {
	type Capability,
	MANIFEST_FILE,
	type Manifest,
	ManifestError,
	readManifest,
} from './manifest.js';
export { ENGINE_NAMES, plain } from './output.js';
export { ARCHIVE_SUFFIX, type Packed, archiveName, pack } from './pack.js';
