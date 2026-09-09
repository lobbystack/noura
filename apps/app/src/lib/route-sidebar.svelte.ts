import { createContext, type Snippet } from 'svelte';

/** Route-owned controls rendered in the shared sidebar, scoped to this layout. */
export class RouteSidebar {
	content = $state.raw<Snippet | null>(null);
	mount(content: Snippet) {
		this.content = content;
		return () => {
			if (this.content === content) this.content = null;
		};
	}
}
export const [getRouteSidebar, setRouteSidebar] = createContext<RouteSidebar>();
