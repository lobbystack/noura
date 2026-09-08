import * as pdfjs from 'pdfjs-dist';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
import { base } from '$app/paths';
import { normalizePdfPage, type PdfPosition } from './navigation';
import type { PdfRead } from '@noura/workspace';
import type {
	PDFViewer,
	EventBus,
} from 'pdfjs-dist/types/web/pdf_viewer.component';
import 'pdfjs-dist/web/pdf_viewer.css';

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;
const assets = `${base}/pdfjs/`;
export interface PdfStatus extends PdfPosition {
	pages: number;
	matches: string;
}
export interface PdfHandle {
	go(page: number): void;
	zoom(delta: number): void;
	fit(): void;
	find(query: string, previous?: boolean, again?: boolean): void;
	thumbnail(canvas: HTMLCanvasElement, page: number): () => void;
	destroy(): void;
}

export async function mountPdf(
	host: HTMLDivElement,
	data: PdfRead,
	options: {
		position: PdfPosition;
		signal: AbortSignal;
		onstatus: (status: PdfStatus) => void;
		onpassword: (submit: (password: string) => void, wrong: boolean) => void;
		openExternal: (url: string) => Promise<void>;
	},
): Promise<PdfHandle> {
	const { PDFViewer, EventBus, PDFLinkService, PDFFindController } =
		await import('pdfjs-dist/web/pdf_viewer.mjs');
	if (options.signal.aborted) throw new DOMException('Aborted', 'AbortError');
	const container = document.createElement('div');
	container.style.cssText = 'position:absolute;inset:0;overflow:auto';
	host.replaceChildren(container);
	const events: EventBus = new EventBus();
	const links = new PDFLinkService({ eventBus: events });
	// PDF actions can only change the page. File launches and named actions
	// must never cross the native boundary.
	links.executeNamedAction = (action: string) => {
		if (action === 'NextPage')
			links.page = Math.min(links.pagesCount, links.page + 1);
		if (action === 'PrevPage') links.page = Math.max(1, links.page - 1);
		if (action === 'FirstPage') links.page = 1;
		if (action === 'LastPage') links.page = links.pagesCount;
	};
	links.addLinkAttributes = (link: HTMLAnchorElement, url: string) => {
		link.textContent ||= '';
		try {
			const parsed = new URL(url);
			if (!['http:', 'https:'].includes(parsed.protocol)) return;
			link.href = parsed.href;
			link.rel = 'noopener noreferrer';
			link.onclick = (event) => {
				event.preventDefault();
				event.stopPropagation();
				void options.openExternal(parsed.href).catch(() => {});
			};
		} catch {
			/* Invalid PDF links remain inert. */
		}
	};
	const find = new PDFFindController({ eventBus: events, linkService: links });
	const pages = document.createElement('div');
	pages.className = 'pdfViewer';
	container.replaceChildren(pages);
	const viewer: PDFViewer = new PDFViewer({
		container,
		viewer: pages,
		eventBus: events,
		linkService: links,
		findController: find,
		annotationMode: pdfjs.AnnotationMode.ENABLE,
		annotationEditorMode: pdfjs.AnnotationEditorType.DISABLE,
		enableAutoLinking: false,
		imageResourcesPath: `${assets}web/images/`,
		maxCanvasPixels: 8_388_608,
		maxCanvasDim: 8192,
		enableDetailCanvas: false,
	});
	links.setViewer(viewer);
	let matches = '';
	let destroyed = false;
	let documentReady = false;
	const pageRequests = new Set<Promise<pdfjs.PDFPageProxy>>();
	const publish = () => {
		if (!destroyed)
			options.onstatus({
				page: viewer.currentPageNumber,
				pages: viewer.pagesCount,
				scale: viewer.currentScaleValue || 'page-width',
				matches,
			});
	};
	events.on('pagechanging', publish);
	events.on('scalechanging', publish);
	events.on(
		'updatefindmatchescount',
		({
			matchesCount,
		}: {
			matchesCount: { current: number; total: number };
		}) => {
			matches = `${matchesCount.current} / ${matchesCount.total}`;
			publish();
		},
	);
	events.on('updatefindcontrolstate', ({ state }: { state: number }) => {
		if (state === 1) {
			matches = 'No matches';
			publish();
		}
	});
	const task = pdfjs.getDocument({
		data: data.bytes,
		cMapUrl: `${assets}cmaps/`,
		cMapPacked: true,
		standardFontDataUrl: `${assets}standard_fonts/`,
		wasmUrl: `${assets}wasm/`,
		iccUrl: `${assets}iccs/`,
		enableXfa: false,
		disableAutoFetch: true,
		maxImageSize: 33_554_432,
		canvasMaxAreaInBytes: 33_554_432,
	});
	task.onPassword = (submit: (password: string) => void, reason: number) =>
		options.onpassword(
			submit,
			reason === pdfjs.PasswordResponses.INCORRECT_PASSWORD,
		);
	const observer = new ResizeObserver(() => {
		if (!destroyed && viewer.pagesCount) {
			viewer.currentScaleValue = viewer.currentScaleValue;
			viewer.update();
		}
	});
	observer.observe(container);
	const cleanup = () => {
		if (destroyed) return;
		destroyed = true;
		observer.disconnect();
		if (viewer.renderingQueue) viewer.renderingQueue.renderView = () => false;
		const release = () => {
			// PDF.js accepts null to clear the viewer; its published types omit it.
			(viewer.setDocument as (document: pdfjs.PDFDocumentProxy | null) => void)(
				null,
			);
			links.setDocument(null);
			(find.setDocument as (document: pdfjs.PDFDocumentProxy | null) => void)(
				null,
			);
			return task.destroy();
		};
		// Settle issued page requests before removing their page views. Eager
		// page initialization is disabled so hidden documents start no new work.
		if (documentReady) void Promise.allSettled([...pageRequests]).then(release);
		else void release();
		container.remove();
		options.signal.removeEventListener('abort', cleanup);
	};
	options.signal.addEventListener('abort', cleanup, { once: true });
	try {
		const doc = await task.promise;
		documentReady = true;
		const getPage = doc.getPage.bind(doc);
		doc.getPage = (number) => {
			const pending = getPage(number);
			pageRequests.add(pending);
			void pending.then(
				() => pageRequests.delete(pending),
				() => pageRequests.delete(pending),
			);
			return pending;
		};
		if (destroyed) throw new DOMException('Aborted', 'AbortError');
		const initialized = new Promise<void>((resolve, reject) => {
			const abort = () => reject(new DOMException('Aborted', 'AbortError'));
			options.signal.addEventListener('abort', abort, { once: true });
			events.on(
				'pagesinit',
				() => {
					options.signal.removeEventListener('abort', abort);
					resolve();
				},
				{ once: true },
			);
		});
		links.setDocument(doc);
		viewer.setDocument(doc);
		await initialized;
		if (destroyed) throw new DOMException('Aborted', 'AbortError');
		viewer.currentScaleValue = options.position.scale;
		viewer.scrollPageIntoView({
			pageNumber: normalizePdfPage(options.position.page, doc.numPages),
		});
		publish();
		return {
			go: (page) =>
				viewer.scrollPageIntoView({
					pageNumber: normalizePdfPage(page, doc.numPages),
				}),
			zoom: (delta) => viewer.updateScale({ steps: delta }),
			fit: () => {
				viewer.currentScaleValue = 'page-width';
			},
			find: (query, previous = false, again = false) =>
				events.dispatch('find', {
					source: viewer,
					type: again ? 'again' : '',
					query,
					phraseSearch: true,
					caseSensitive: false,
					entireWord: false,
					highlightAll: true,
					findPrevious: previous,
				}),
			thumbnail: (canvas, page) => {
				let disposed = false;
				let render: pdfjs.RenderTask | undefined;
				void doc
					.getPage(page)
					.then((pdfPage) => {
						if (disposed || destroyed) return;
						const natural = pdfPage.getViewport({ scale: 1 });
						const viewport = pdfPage.getViewport({
							scale: 100 / natural.width,
						});
						canvas.width = Math.ceil(viewport.width);
						canvas.height = Math.ceil(viewport.height);
						render = pdfPage.render({
							canvas,
							viewport,
							annotationMode: pdfjs.AnnotationMode.DISABLE,
						});
						return render.promise;
					})
					.catch(() => {});
				return () => {
					disposed = true;
					render?.cancel();
					canvas.width = 0;
					canvas.height = 0;
				};
			},
			destroy: cleanup,
		};
	} catch (error) {
		cleanup();
		throw error;
	}
}
