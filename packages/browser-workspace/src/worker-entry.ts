import { startBrowserWorkspaceWorker } from './worker';
import type { BrowserWorkerPort } from './protocol';

void startBrowserWorkspaceWorker(self as unknown as BrowserWorkerPort);
