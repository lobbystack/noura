# Register object types

The `object-types` interface is what a plugin offers Noura to add its own kinds of objects and the properties panel beside each one. Export it from `src/plugin.ts` as `objectTypes`, typed `ObjectTypesExports`. Noura calls it only when the manifest declares the `object-types` capability.

Objects of a plugin's types open in Noura's note editor. The plugin draws the properties panel beside the body, and writes the objects through the [`objects`](objects.md) interface.

## Functions

- `types()`: the types the plugin registers when it starts. A type's name is a lowercase letter followed by lowercase letters or hyphens. Its schema lists its properties as JSON. Noura keeps its own types, such as `note`, holds `task` for the Tasks plugin and holds `project` for the Projects plugin.
- `renderPanel(object)`: the [nodes](ui.md) of the properties panel for the object with this ID. A panel can't hold a virtual list.
- `handlePanelEvent(object, event)`: something you did in an object's panel. Noura draws the panel again afterwards.

## Schemas

A schema lists each property's `name` and `kind`, in the order Noura writes them, with an optional `default` and `"nullable": true` to accept `null`. A kind is `"string"`, `"number"`, `"boolean"`, `"date"`, `{ "enum": [...] }` for one of a set of strings, or `{ "reference": "object_type" }` for the ID of an object of that type:

```json
{
	"properties": [
		{
			"name": "course",
			"kind": { "enum": ["starter", "main", "dessert"] },
			"default": "main"
		},
		{ "name": "vegetarian", "kind": "boolean", "default": false },
		{ "name": "cooked", "kind": "date", "nullable": true }
	]
}
```

Noura checks each write against the schema and refuses a property that doesn't fit with `invalid-property`. The [workspace format](../../workspace-format/v1.md#schema-declaration) describes the declaration in full.

## Example

This plugin registers the `recipe` type and draws its panel: the course as a choice and whether the recipe is vegetarian as a checkbox. Each change writes the recipe's file. Its manifest declares `object-types`:

```toml
id = "example-object-types"
name = "Recipes"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "object-types"
```

`renderPanel` reads the recipe's file each time Noura draws the panel. `handlePanelEvent` writes the input that changed, with the revision it read:

```ts
import type { ObjectTypesExports } from '@noura/plugin-types/0.1.0';
import { get, update } from 'noura:plugin/objects@0.1.0';
import type { Event, Node } from 'noura:plugin/ui@0.1.0';

const COURSES = ['starter', 'main', 'dessert'];

const SCHEMA = {
	properties: [
		{ name: 'course', kind: { enum: COURSES }, default: 'main' },
		{ name: 'vegetarian', kind: 'boolean', default: false },
	],
};

export const objectTypes: ObjectTypesExports = {
	types: () => [{ name: 'recipe', schema: JSON.stringify(SCHEMA) }],

	renderPanel(object: string): Node[] {
		const recipe = get(object);
		const properties = JSON.parse(recipe.properties) as {
			course?: string;
			vegetarian?: boolean;
		};
		const choices = COURSES.map((course) => ({
			id: course,
			label: course[0]?.toUpperCase() + course.slice(1),
		}));
		return [
			{
				id: 'panel',
				kind: { tag: 'panel', val: 'Recipe' },
				children: new Uint32Array([1, 2]),
			},
			{
				id: 'course',
				kind: {
					tag: 'input',
					val: {
						label: 'Course',
						value: { tag: 'choice', val: properties.course },
						choices,
					},
				},
				children: new Uint32Array(),
			},
			{
				id: 'vegetarian',
				kind: {
					tag: 'input',
					val: {
						label: 'Vegetarian',
						value: { tag: 'checkbox', val: properties.vegetarian === true },
						choices: [],
					},
				},
				children: new Uint32Array(),
			},
		];
	},

	handlePanelEvent(object: string, event: Event) {
		if (event.tag !== 'changed') {
			return;
		}
		const { node, value } = event.val;
		const recipe = get(object);
		const changed =
			value.tag === 'choice' || value.tag === 'checkbox'
				? { [node]: value.val }
				: {};
		update(recipe.id, recipe.revision, {
			properties: JSON.stringify(changed),
			remove: [],
		});
	},
};
```

## Notes

- A type another running plugin registered stays that plugin's. Registering it fails, and the plugin's other types register all the same.
- Turning a plugin off releases its types. Their files stay in the workspace, and the next start registers them again.
