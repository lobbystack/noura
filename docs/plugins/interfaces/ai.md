# Chat with AI providers

The `ai` interface sends your messages to the AI providers you set up in Noura and keeps each chat in the workspace. Import it from `noura:plugin/ai@0.1.0`. Each call needs the `ai` capability; without it, the call throws `capability-not-declared` with `ai`.

Noura keeps the providers, their keys and your consent on this device, and no call returns a key. Before Noura sends the workspace's content to a provider, it asks you in a prompt of its own whether that provider may receive it. It asks again after you change the provider's kind, endpoint or model.

## Functions

- `providers()`: the providers you set up and turned on in Settings > AI providers. Each has an `id`, a `name`, a `kind`, such as `openai` or `ollama`, and the `model` it answers with.
- `chats()`: the workspace's chats, the one changed last first.
- `readChat(id)`: a chat and its messages, oldest first. A message has a `kind`, a `status` and its `content`: Markdown, or JSON text for a tool call or a tool result.
- `createChat()`: a new chat titled `New chat`. Noura writes its files when its first message goes out, and `chats` lists it from then on. A chat whose first message never goes out leaves no file.
- `send(request)`: send a message in a chat to a provider, with the tools the model may ask to call, and get back a run. Until you consent for this workspace and provider, the run waits as `waiting-for-consent` while Noura asks you. Once you consent, Noura writes your message and an empty answer to the chat before the request leaves this device. The answer then streams into the run. A chat answers one message at a time, so a second `send` to it fails while the first runs.
- `readRun(id)`: the run as it stands now, with the answer so far in `text`.
- `answerTools(run, results, tools)`: answer the tool calls of a completed run, one result per call, and stream the model's next answer into the same run.
- `stop(id)`: stop a run. The answer so far stays in the chat as a cancelled message.
- `webReady()`: whether the model may search the web and read pages in this workspace. It returns `true` once you add a Brave Search key in Settings > Web search and allow web search for the workspace.
- `webSearch(query, maxResults)`: search the web with Brave Search and get each result's `title`, `url` and `snippet`. Pass from 1 to 10 for `maxResults`, or `undefined` for 10.
- `webFetch(url)`: read the page at an `https` address and get its `text`, the `url` that answered after redirects, its `contentType` and whether Noura cut the text at its size limit (`truncated`).

`readRun`, `answerTools` and `stop` reach only the plugin's own runs. Another plugin's run is `not-found`.

## Search the web for the model

Offer the model web tools in `send` and `answerTools` only while `webReady()` returns `true`. When a completed run lists a call to one of your web tools, call `webSearch` or `webFetch`, then pass the answer back as that call's result with `answerTools`. Noura writes each call and its result to the chat.

Noura sends each web request for the plugin through its own boundary, so the plugin needs no `network` capability and opens no connection. Your web consent covers one workspace on this device, and it's a separate choice from a provider's consent. Noura asks for it in its own prompt and doesn't ask again before each call.

A web call that fails throws a `web-error`:

- `refused`: Noura refused the call as it refuses the interface's other calls, such as `capability-not-declared` with `ai`.
- `failed`: the call failed with a `failure`, whose `code` says why, such as `web_search_key_missing`, `web_consent_required`, `web_search_key_rejected`, `web_search_rate_limited`, `web_url_not_https` or `web_url_unsafe_host`. Its `message` holds Noura's own words for the model or for you.

Pass a failure back to the model as the call's result, so the chat goes on without the web. A failure never holds the key or what Brave Search or the site answered.

`webFetch` reads only HTML and plain text, up to 5 MiB. It follows up to three redirects and checks every address each host resolves to. It refuses local and private addresses before it connects, and it cuts the text at 1 MiB.

## Example

This plugin adds a page where you ask a question of the first provider you set up. Its manifest declares `ai` for the questions and `views` for the page:

```toml
id = "example-ai"
name = "Ask"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "ai"

[[capabilities]]
name = "views"
```

`handleEvent` starts a chat for each question and sends it. `render` reads the run each time Noura draws the page, and Noura draws it again as text arrives:

```ts
import type { ViewExports } from '@noura/plugin-types/0.1.0';
import {
	type Run,
	createChat,
	providers,
	readRun,
	send,
	stop,
} from 'noura:plugin/ai@0.1.0';
import type { Event, Node, NodeKind } from 'noura:plugin/ui@0.1.0';

/** The run of the last question you sent. */
let run: string | undefined;

function node(id: string, kind: NodeKind, children: number[] = []): Node {
	return { id, kind, children: new Uint32Array(children) };
}

function text(val: string): NodeKind {
	return { tag: 'text', val };
}

/** The answer so far, with a button to stop it while it runs. */
function answer(current: Run): Node[] {
	const stopButton = node('stop', { tag: 'button', val: 'Stop' });
	switch (current.status) {
		case 'waiting-for-consent':
			return [
				node(
					'answer',
					text('Noura is asking you whether the provider may receive it.'),
				),
				stopButton,
			];
		case 'denied':
			return [node('answer', text('Your question stayed on this device.'))];
		case 'failed':
			return [
				node('answer', text(current.failure?.message ?? 'The answer failed.')),
			];
		case 'streaming':
			return [
				node('answer', { tag: 'markdown', val: current.text }),
				stopButton,
			];
		default:
			return [node('answer', { tag: 'markdown', val: current.text })];
	}
}

export const view: ViewExports = {
	pages: () => [{ id: 'ask', title: 'Ask' }],

	render(): Node[] {
		const [provider] = providers();
		if (!provider) {
			return [
				node('root', { tag: 'panel', val: 'Ask' }, [1]),
				node(
					'none',
					text('Set up a provider in Settings > AI providers, then ask here.'),
				),
			];
		}
		const shown = run === undefined ? [] : answer(readRun(run));
		return [
			node('root', { tag: 'panel', val: `Ask ${provider.name}` }, [
				1,
				...shown.map((_, index) => index + 3),
			]),
			node(
				'ask',
				{
					tag: 'form',
					val: { label: 'Ask', submit: 'Send', required: ['question'] },
				},
				[2],
			),
			node('question', {
				tag: 'input',
				val: {
					label: 'Question',
					value: { tag: 'text', val: '' },
					choices: [],
				},
			}),
			...shown,
		];
	},

	handleEvent(_page: string, event: Event) {
		if (event.tag === 'pressed' && event.val === 'stop' && run !== undefined) {
			stop(run);
		} else if (event.tag === 'submitted') {
			const question = event.val.fields.find(
				(field) => field.node === 'question',
			)?.value;
			const [provider] = providers();
			if (provider && question?.tag === 'text' && question.val.trim() !== '') {
				run = send({
					chat: createChat().id,
					provider: provider.id,
					message: question.val,
					tools: [],
				}).id;
			}
		}
	},
};
```

## Notes

- If you deny a run, or stop it while it waits for consent, Noura doesn't write your message. The run's `messageWritten` is then `false`, so the plugin can give the message back to you. The same goes for a run that fails before Noura writes the message.
- Noura writes the answer and its status to disk before the run shows as `completed`, `cancelled` or `failed`.
- A failed run's `failure` holds a stable `code`, such as `provider_request_failed`, and a message. It never holds the provider's own answer, its headers or a key.
- A chat is a folder of Markdown files in the workspace: `chat.md` and one file per message. A plugin with `workspace-read` reads them like other files, but only `ai` changes them.
- The mock host asks no provider. Pass `providers` to `createMockHost` to list some. A run there waits for consent until the plugin stops it.
- The mock host has no Brave Search key. Its `webReady` returns `false`, and `webSearch` and `webFetch` fail with `web_search_key_missing`, as Noura's do before you add a key.
