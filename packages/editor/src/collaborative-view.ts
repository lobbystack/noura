import {
	Compartment,
	EditorState,
	StateEffect,
	StateField,
	type Range,
} from '@codemirror/state';
import { Decoration, EditorView, WidgetType, keymap } from '@codemirror/view';
import { markdown } from '@codemirror/lang-markdown';
import { yCollab, yUndoManagerKeymap } from 'y-codemirror.next';
import * as Y from 'yjs';
import {
	decodeCollaborationUpdate,
	type CollaborationSession,
} from './collaboration';

class CursorLabel extends WidgetType {
	constructor(
		private name: string,
		private color: string,
	) {
		super();
	}
	toDOM() {
		const cursor = document.createElement('span');
		cursor.style.borderLeft = `2px solid ${this.color}`;
		cursor.style.color = this.color;
		cursor.style.fontSize = '0.7em';
		cursor.textContent = this.name;
		cursor.setAttribute('aria-label', `${this.name}'s cursor`);
		return cursor;
	}
}
const refreshPresence = StateEffect.define<void>();
function presenceDecorations(session: CollaborationSession) {
	const marks: Range<Decoration>[] = [];
	for (const member of session.presence) {
		try {
			const anchor = Y.createAbsolutePositionFromRelativePosition(
				Y.decodeRelativePosition(decodeCollaborationUpdate(member.anchor)),
				session.doc,
			);
			const head = Y.createAbsolutePositionFromRelativePosition(
				Y.decodeRelativePosition(decodeCollaborationUpdate(member.head)),
				session.doc,
			);
			if (
				!anchor ||
				!head ||
				anchor.type !== session.text ||
				head.type !== session.text
			)
				continue;
			const color = /^#[0-9a-f]{6}$/i.test(member.color)
				? member.color
				: '#64748b';
			if (anchor.index !== head.index)
				marks.push(
					Decoration.mark({
						attributes: { style: `background-color: ${color}33` },
					}).range(
						Math.min(anchor.index, head.index),
						Math.max(anchor.index, head.index),
					),
				);
			marks.push(
				Decoration.widget({
					widget: new CursorLabel(member.name, color),
					side: 1,
				}).range(head.index),
			);
		} catch {
			/* Invalid or unresolved native positions are not rendered. */
		}
	}
	return Decoration.set(marks, true);
}

export function createCollaborativeView(
	parent: HTMLElement,
	session: CollaborationSession,
	language: 'markdown' | 'text' = 'text',
) {
	const permissions = new Compartment();
	const presence = StateField.define({
		create: () => presenceDecorations(session),
		update: (value, transaction) =>
			transaction.docChanged ||
			transaction.effects.some((effect) => effect.is(refreshPresence))
				? presenceDecorations(session)
				: value,
		provide: (field) => EditorView.decorations.from(field),
	});
	const view = new EditorView({
		parent,
		state: EditorState.create({
			doc: session.text.toString(),
			extensions: [
				yCollab(session.text, null, { undoManager: session.undoManager }),
				keymap.of(yUndoManagerKeymap),
				permissions.of([
					EditorState.readOnly.of(session.bootstrap.readOnly),
					EditorView.editable.of(!session.bootstrap.readOnly),
				]),
				presence,
				EditorView.lineWrapping,
				...(language === 'markdown' ? [markdown()] : []),
				EditorView.updateListener.of((update) => {
					if (update.selectionSet) {
						const { anchor, head } = update.state.selection.main;
						void session.setPresence(anchor, head)?.catch(() => {});
					}
				}),
			],
		}),
	});
	let destroyed = false;
	let queued = false;
	const unsubscribe = session.subscribe(() => {
		if (queued) return;
		queued = true;
		queueMicrotask(() => {
			queued = false;
			if (destroyed) return;
			const readOnly =
				session.bootstrap.readOnly || session.status === 'Needs review';
			view.dispatch({
				effects: [
					refreshPresence.of(),
					permissions.reconfigure([
						EditorState.readOnly.of(readOnly),
						EditorView.editable.of(!readOnly),
					]),
				],
			});
		});
	});
	const destroy = view.destroy.bind(view);
	view.destroy = () => {
		destroyed = true;
		unsubscribe();
		destroy();
	};
	return view;
}
