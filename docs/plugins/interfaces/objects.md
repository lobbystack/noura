# Create and change your objects

The `objects` interface creates, reads, changes, moves and deletes objects of the plugin's own types: Markdown files in the workspace with a stable ID, properties the type's schema checks, and a body. Import it from `noura:plugin/objects@0.1.0`. It needs the `object-types` capability, and the plugin must register the type through [`object-types`](object-types.md). A call about any other type, such as `note`, fails with `capability-not-declared` naming `object-types`.

## Functions

- `create(object)`: write a new object. Without a `path`, its file goes in a folder named after the type, such as `readings/Dune.md` for the type `reading`, with a number added when the name is taken. With a path that a file has already, it fails with `already-exists`.
- `get(id)`: the object with this ID, read from its file.
- `list(objectType)`: every object of one of the plugin's types. One whose properties don't fit the schema comes with its `problem`, and Noura leaves its file as it is.
- `update(id, expectedRevision, changes)`: change the body, set properties or remove them. The rest of the file keeps its bytes.
- `moveTo(id, expectedRevision, path)`: move or rename the object's file. It keeps its ID.
- `delete(id, expectedRevision)`: move the file to the system trash, or to the workspace's trash folder when the system has none.

Each write carries the revision the plugin read. When the file changed since, as after an edit in another app, the write fails with `revision-conflict` and writes nothing. Each write returns once the file is on disk.

## Example

This plugin keeps a reading list of `reading` objects, each with an author and whether you finished it. A form adds a book, and each book's checkbox marks it finished. Its manifest declares `object-types` and `views`:

```toml
id = "example-objects"
name = "Readings"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "object-types"

[[capabilities]]
name = "views"
```

`objectTypes.types` registers the type with its schema. The view reads the objects from their files each time Noura draws it, so it shows edits you make elsewhere:

```ts
import type { ObjectTypesExports, ViewExports } from '@noura/plugin-types';
import { create, get, list, update } from 'noura:plugin/objects@0.1.0';
import type { Event, Node } from 'noura:plugin/ui@0.1.0';

const SCHEMA = {
	properties: [
		{ name: 'author', kind: 'string' },
		{ name: 'finished', kind: 'boolean', default: false },
	],
};

export const objectTypes: ObjectTypesExports = {
	types: () => [{ name: 'reading', schema: JSON.stringify(SCHEMA) }],
};

function finished(properties: string): boolean {
	return (JSON.parse(properties) as { finished?: boolean }).finished === true;
}

export const view: ViewExports = {
	pages: () => [{ id: 'readings', title: 'Readings' }],

	render(): Node[] {
		const readings = list('reading').sort((a, b) =>
			a.title.localeCompare(b.title),
		);
		const items = readings.map((_, index) => index + 5);
		return [
			{
				id: 'root',
				kind: { tag: 'panel', val: 'Readings' },
				children: new Uint32Array([1, 4]),
			},
			{
				id: 'add',
				kind: {
					tag: 'form',
					val: { label: 'Add a book', submit: 'Add', required: ['title'] },
				},
				children: new Uint32Array([2, 3]),
			},
			{
				id: 'title',
				kind: {
					tag: 'input',
					val: { label: 'Title', value: { tag: 'text', val: '' }, choices: [] },
				},
				children: new Uint32Array(),
			},
			{
				id: 'author',
				kind: {
					tag: 'input',
					val: {
						label: 'Author',
						value: { tag: 'text', val: '' },
						choices: [],
					},
				},
				children: new Uint32Array(),
			},
			{
				id: 'books',
				kind: {
					tag: 'list',
					val: { label: 'Books', reorder: false, opensOnClick: false },
				},
				children: new Uint32Array(items),
			},
			...readings.map((reading): Node => ({
				id: reading.id,
				kind: {
					tag: 'item',
					val: {
						label: reading.title,
						detail: JSON.parse(reading.properties).author,
						checked: finished(reading.properties),
					},
				},
				children: new Uint32Array(),
			})),
		];
	},

	handleEvent(_page: string, event: Event) {
		if (event.tag === 'submitted') {
			const text = (node: string) => {
				const value = event.val.fields.find(
					(field) => field.node === node,
				)?.value;
				return value?.tag === 'text' ? value.val.trim() : '';
			};
			create({
				objectType: 'reading',
				title: text('title'),
				body: '',
				properties: JSON.stringify({ author: text('author') }),
			});
		} else if (event.tag === 'checked') {
			const reading = get(event.val.item);
			update(reading.id, reading.revision, {
				properties: JSON.stringify({ finished: event.val.checked }),
				remove: [],
			});
		}
	},
};
```

## Notes

- An object's title is its file name without `.md`. To rename an object, move it with `moveTo`.
- A plugin that turns off leaves its objects' files in the workspace, readable as Markdown without Noura.
- The [`object-types`](object-types.md) page shows the properties panel Noura draws beside an object's body.
