import { getCurrentWindow } from '@tauri-apps/api/window';
import type { HostLifecycleAdapter } from './host-lifecycle';

export function createTauriHostLifecycle(): HostLifecycleAdapter {
	const window = getCurrentWindow();
	return {
		onCloseRequested: (handler) => window.onCloseRequested(handler),
		forceClose: () => window.destroy(),
	};
}
