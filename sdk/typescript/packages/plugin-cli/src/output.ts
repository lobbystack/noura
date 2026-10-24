// Keeps the kit's output about plugins. The tools the kit runs may name
// the JavaScript engine they build with, which a plugin author never
// chooses or depends on, so the kit's output names it in general terms.

/** Names of JavaScript engines and their build tools, as tools print them. */
export const ENGINE_NAMES = /componentize-qjs|componentize-js|quickjs|qjs|starlingmonkey|spidermonkey/gi;

/** `text` with every engine name replaced by a general term. */
export function plain(text: string): string {
	return text.replace(ENGINE_NAMES, (name) =>
		name.toLowerCase().startsWith('componentize') ? 'the component builder' : 'the JavaScript engine',
	);
}
