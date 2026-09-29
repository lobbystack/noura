import { startBrowserWorkspaceWorker } from '@noura/browser-workspace';
declare const __NOURA_WORKSPACE_WASM_URL__: string;

// Requests that arrive while WASM and storage start wait for them.
void startBrowserWorkspaceWorker(self, {
	wasmUrl: new URL(__NOURA_WORKSPACE_WASM_URL__, self.location.href).href,
});
