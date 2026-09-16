import {
	BrowserWorkspaceServer,
	createOpfsWorkspaceRegistry,
	installBrowserWorkspaceWorker,
} from '@noura/browser-workspace';
import { loadWorkspaceFormat } from '@noura/workspace-format-wasm';
declare const __NOURA_WORKSPACE_WASM_URL__: string;

let stage = 'WASM';
async function start() {
	const format = await loadWorkspaceFormat(
		new URL(__NOURA_WORKSPACE_WASM_URL__, self.location.href).href,
	);
	stage = 'OPFS';
	const registry = await createOpfsWorkspaceRegistry(format);
	installBrowserWorkspaceWorker(
		self,
		new BrowserWorkspaceServer({
			format,
			registry,
			onEvent: (event) => self.postMessage({ type: 'event', event }),
		}),
	);
}

// Do not accept client requests before WASM and OPFS have initialized.
void start().then(
	() => self.postMessage({ type: 'ready' }),
	() => self.postMessage({ type: 'startup-error', stage }),
);
