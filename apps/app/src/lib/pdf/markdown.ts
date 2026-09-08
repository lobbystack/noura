import { mount, unmount } from 'svelte';
import { goto } from '$app/navigation';
import type { LiveMarkdownOptions } from '@noura/editor/types';
import { getNouraClient } from '$lib/state.svelte';
import PdfViewer from '$lib/components/pdf-viewer.svelte';
import { pdfHref } from './navigation';

export function markdownAssets(
	sourceRelativePath?: string,
): Pick<
	LiveMarkdownOptions,
	'resolveImage' | 'resolveLink' | 'openPdf' | 'mountPdfEmbed'
> {
	if (!sourceRelativePath) return {};
	const files = getNouraClient().files;
	return {
		resolveImage: (target) =>
			files
				.readLocalAsset({ sourceRelativePath, target })
				.then((asset) => asset.dataUrl)
				.catch(() => null),
		resolveLink: async (target) => {
			const resolved = await files.resolveMarkdownLink({
				sourceRelativePath,
				target,
			});
			if (resolved.kind === 'managed')
				return {
					kind: 'managed',
					label: resolved.object.title,
					preview: resolved.object.body,
				};
			if (resolved.kind === 'markdown')
				return {
					kind: 'markdown',
					label: target,
					preview: resolved.document.body,
				};
			return resolved;
		},
		openPdf: (target) => {
			void goto(pdfHref(target.relativePath, target.page ?? undefined));
		},
		mountPdfEmbed: (container, target) => {
			const component = mount(PdfViewer, {
				target: container,
				props: {
					relativePath: target.relativePath,
					embedded: true,
					initialPosition: { page: target.page ?? 1, scale: 'page-width' },
				},
			});
			return () => {
				void unmount(component);
			};
		},
	};
}
