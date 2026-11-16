# Read the workspace

The `workspace` interface reads the files, folders and objects in the open workspace. Import it from `noura:plugin/workspace@0.1.0`. Each call needs the `workspace-read` capability; without it, the call throws `capability-not-declared` with `workspace-read`.

## Functions

- `listEntries()`: every file and folder, by workspace-relative path with `/` between its parts. An entry is a result: a name that a string can't hold, such as one that isn't valid UTF-8, comes back as the `unsupported-path` error in its place.
- `readFile(path)`: a file's bytes and the revision of those bytes. A path that leaves the workspace or names `.noura` throws `invalid-path`, and a missing file throws `not-found`.
- `readObject(id)`: a Noura object by its stable ID, wherever its file is now. The ID stays the same when you move or rename the file. When two files hold the ID, as after a copy made in another app, it throws `identity-conflict` with the ID.
- `readObjectAt(path)`: the object that the file at `path` holds, even when another file holds the same ID. A missing file, or one that holds no object, throws `not-found` with the path.
- `linkTo(id)`: the link Noura writes for the object with this stable ID, such as `[[Garden]]`, or `[[Projects/Garden/Garden]]` when another file shares its title. Put it in a reference property, such as a task's `project`. Noura rewrites the link when the file moves or gets a new title.
- `resolveLink(link, from)`: the object a link leads to, or `undefined` when it leads to a plain note or nowhere. Pass the path of the file that holds the link as `from`, or `undefined` to resolve from the top of the workspace. The link may keep its brackets, a heading after `#` or a label after `|`.

## Example

This plugin adds a page that lists the Markdown files in the workspace. Each shows the type of the object it holds, or its first line when it holds none. Its manifest declares `workspace-read` for the reads and `views` for the page:

```toml
id = "example-workspace"
name = "Markdown files"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "workspace-read"

[[capabilities]]
name = "views"
```

`render` lists the entries, keeps the Markdown files and describes each one. A call that fails throws an error whose `payload` is the error Noura gave:

```ts
import type { ViewExports } from '@noura/plugin-types/0.1.0';
import type { Node, NodeKind } from 'noura:plugin/ui@0.1.0';
import {
	listEntries,
	readFile,
	readObjectAt,
} from 'noura:plugin/workspace@0.1.0';

function node(id: string, kind: NodeKind, children: number[] = []): Node {
	return { id, kind, children: new Uint32Array(children) };
}

/** The type of the object the file holds, or its first line. */
function describe(path: string): string {
	try {
		return `A ${readObjectAt(path).objectType}`;
	} catch (error) {
		const tag = (error as { payload?: { tag: string } }).payload?.tag;
		if (tag !== 'not-found') {
			throw error;
		}
	}
	const text = new TextDecoder().decode(readFile(path).bytes);
	return text.split('\n')[0]?.trim() || '(empty)';
}

export const view: ViewExports = {
	pages: () => [{ id: 'markdown', title: 'Markdown files' }],

	render(): Node[] {
		const paths = listEntries()
			.flatMap((entry) =>
				entry.tag === 'ok' && entry.val.kind === 'file' ? [entry.val.path] : [],
			)
			.filter((path) => path.endsWith('.md'))
			.sort();
		const list: NodeKind = {
			tag: 'list',
			val: { label: 'Markdown files', reorder: false, opensOnClick: false },
		};
		return [
			node('root', { tag: 'panel', val: 'Markdown files' }, [1]),
			node(
				'files',
				list,
				paths.map((_, index) => index + 2),
			),
			...paths.map((path) =>
				node(path, {
					tag: 'item',
					val: { label: path, detail: describe(path) },
				}),
			),
		];
	},
};
```

## Notes

- `readFile` reads the file from disk on each call, so an edit you make in another app shows on the next one.
- Links resolve as the editor resolves them: relative to the linking file, then from the top of the workspace, then by title anywhere, preferring the linking file's folder and then the shortest path. The [workspace format](../../workspace-format/v1.md#links) has the details.
- A file larger than 16 MB fails with `failed`.
- To change files, a plugin writes objects of its own types through the [`objects`](objects.md) interface. It can't write any other file.
