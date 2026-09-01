<script lang="ts">
	import { browser } from '$app/environment';
	import { Editor } from '@tiptap/core';
	import StarterKit from '@tiptap/starter-kit';
	import TaskList from '@tiptap/extension-task-list';
	import TaskItem from '@tiptap/extension-task-item';
	import { Markdown } from '@tiptap/markdown';

	let { markdown }: { markdown: string } = $props();
	let editor: Editor | null = null;

	function preview(node: HTMLDivElement) {
		if (browser) {
			editor = new Editor({
				element: node,
				extensions: [
					StarterKit,
					TaskList,
					TaskItem.configure({ nested: true }),
					Markdown,
				],
				content: markdown,
				contentType: 'markdown',
				editable: false,
			});
		}
		return () => {
			editor?.destroy();
			editor = null;
		};
	}
</script>

<div
	{@attach preview}
	class="
		text-sm [&_.ProseMirror]:outline-none
		[&_.ProseMirror_h1]:mb-3 [&_.ProseMirror_h1]:text-xl [&_.ProseMirror_h1]:font-semibold
		[&_.ProseMirror_h2]:mt-4 [&_.ProseMirror_h2]:mb-2 [&_.ProseMirror_h2]:text-lg [&_.ProseMirror_h2]:font-semibold
		[&_.ProseMirror_p]:mb-2 [&_.ProseMirror_p]:leading-relaxed
		[&_.ProseMirror_ul]:my-2 [&_.ProseMirror_ul]:pl-5
		[&_.ProseMirror_ol]:my-2 [&_.ProseMirror_ol]:pl-5
		[&_.ProseMirror_blockquote]:border-l-2 [&_.ProseMirror_blockquote]:border-border [&_.ProseMirror_blockquote]:pl-3 [&_.ProseMirror_blockquote]:text-muted-foreground
		[&_.ProseMirror_pre]:my-3 [&_.ProseMirror_pre]:overflow-x-auto [&_.ProseMirror_pre]:rounded-md [&_.ProseMirror_pre]:bg-muted [&_.ProseMirror_pre]:p-3
	"
></div>
