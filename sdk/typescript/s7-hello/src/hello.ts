// Trial S7: a hello plugin in TypeScript. It shows a count and a button that
// adds one to it.
import type { Node } from 'noura-trial:s7/plugin@0.1.0';

let count = 0;

export function render(): Node[] {
	return [
		{ id: 'root', kind: { tag: 'panel' }, children: new Uint32Array([1, 2]) },
		{
			id: 'count',
			kind: { tag: 'text', val: `Count: ${count}` },
			children: new Uint32Array(),
		},
		{
			id: 'add-one',
			kind: { tag: 'button', val: 'Add one' },
			children: new Uint32Array(),
		},
	];
}

export function handleEvent(nodeId: string): void {
	if (nodeId === 'add-one') {
		count += 1;
	}
}
