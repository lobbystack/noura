// A page in Noura's rail that counts the files in your workspace and the
// times you press its button.
import type { ViewExports } from '@noura/plugin-types';
import type { Event, Node, NodeKind } from 'noura:plugin/ui@0.1.0';
import { listEntries } from 'noura:plugin/workspace@0.1.0';

const PAGE = 'main';

let presses = 0;

function node(id: string, kind: NodeKind, children: number[] = []): Node {
	return { id, kind, children: new Uint32Array(children) };
}

function plural(count: number, one: string, many: string): string {
	return `${count} ${count === 1 ? one : many}`;
}

export const view: ViewExports = {
	pages() {
		return [{ id: PAGE, title: '{{name}}' }];
	},

	render(): Node[] {
		// Needs the workspace-read capability, which plugin.toml declares.
		const files = listEntries().filter((entry) => entry.tag === 'ok' && entry.val.kind === 'file');
		return [
			node('root', { tag: 'panel', val: '{{name}}' }, [1, 2, 3]),
			node('files', { tag: 'text', val: `Your workspace holds ${plural(files.length, 'file', 'files')}.` }),
			node('presses', { tag: 'text', val: `You pressed the button ${plural(presses, 'time', 'times')}.` }),
			node('press', { tag: 'button', val: 'Press' }),
		];
	},

	handleEvent(_page: string, event: Event) {
		if (event.tag === 'pressed' && event.val === 'press') {
			presses += 1;
		}
	},
};
