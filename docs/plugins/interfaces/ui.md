# Describe a view with UI nodes

The `ui` interface defines the nodes a plugin returns to show something and the events Noura sends back. Import its types from `noura:plugin/ui@0.1.0`. It has no functions and needs no capability; the interfaces that draw, such as [`view`](view.md), need theirs.

## Nodes

A view is a flat list of nodes, because WIT has no recursive types. The first node is the root, and each node's `children` holds the indices of its children in the list, as a `Uint32Array`. Each node has an `id`, unique within the view, which events name it by.

A node's `kind` sets what Noura draws:

- Layout and text: `panel`, `text` and `markdown`.
- Controls: `button`, `input` (a single or multi-line text, a checkbox, a choice, a date or a date and time), `form` and `name-field`.
- Collections: `list` with `item` children, `virtual-list`, `table` with `row` children, `board` with `board-column` and `card` children, and `calendar` with `entry` children.
- Menus: `menu`, `menu-item` and `separator`, the only child of an item, a row, a card or a button.

Every label must hold more than spaces, since screen readers read it. Noura checks a view against its rules before it draws it, and shows why in its place when the view breaks one.

## Events

Noura sends an `event` when you act in a view: `pressed` names a button, `selected` and `activated` name an item, a row, a card or an entry, `changed` carries an input's new value, `submitted` carries every input in a form, `chosen` names a menu item, `moved`, `checked`, `named` and `cancelled` come from drags, checkboxes and name fields, and `sorted` names a table column. After the plugin handles an event, Noura draws its views again.

## Example

This plugin adds a reading list: a form to add a book and a list of books, each with a menu to remove it. Its manifest declares `views`:

```toml
id = "example-ui"
name = "Reading list"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "views"
```

`add` appends a node and records it as a child of its parent, so the indices stay right as the view grows:

```ts
import type { ViewExports } from '@noura/plugin-types';
import type { Event, Node, NodeKind } from 'noura:plugin/ui@0.1.0';

const books = ['The Left Hand of Darkness'];

function add(nodes: Node[], parent: number | undefined, id: string, kind: NodeKind): number {
	nodes.push({ id, kind, children: new Uint32Array() });
	const index = nodes.length - 1;
	const holder = parent === undefined ? undefined : nodes[parent];
	if (holder) {
		holder.children = Uint32Array.from([...holder.children, index]);
	}
	return index;
}

export const view: ViewExports = {
	pages: () => [{ id: 'reading', title: 'Reading list' }],

	render(): Node[] {
		const nodes: Node[] = [];
		const root = add(nodes, undefined, 'root', { tag: 'panel', val: 'Reading list' });
		const form = add(nodes, root, 'add-book', {
			tag: 'form',
			val: { label: 'Add a book', submit: 'Add', required: ['title'] },
		});
		add(nodes, form, 'title', { tag: 'input', val: { label: 'Title', value: { tag: 'text', val: '' }, choices: [] } });
		const list = add(nodes, root, 'books', {
			tag: 'list',
			val: { label: 'Books', reorder: false, opensOnClick: false },
		});
		books.forEach((title, index) => {
			const item = add(nodes, list, `book-${index}`, { tag: 'item', val: { label: title } });
			const menu = add(nodes, item, `menu-${index}`, { tag: 'menu', val: `Actions for ${title}` });
			add(nodes, menu, `remove-${index}`, { tag: 'menu-item', val: { label: 'Remove', enabled: true } });
		});
		return nodes;
	},

	handleEvent(_page: string, event: Event) {
		if (event.tag === 'submitted') {
			const title = event.val.fields.find((field) => field.node === 'title')?.value;
			if (title?.tag === 'text' && title.val.trim() !== '') {
				books.push(title.val.trim());
			}
		} else if (event.tag === 'chosen' && event.val.startsWith('remove-')) {
			books.splice(Number(event.val.slice('remove-'.length)), 1);
		}
	},
};
```

## Notes

- A view holds at most 10,000 nodes and nests at most 64 panels deep. A `virtual-list` holds up to 1,000,000 rows, which Noura fetches as they scroll into view; the [`view`](view.md) page shows one.
- Noura moves nothing on its own: after a drag, the list or board shows the items where the plugin draws them next.
- The [reference](../reference/noura-plugin-0.1.0.md#noura_plugin_ui_0_1_0) lists every node kind's fields.
