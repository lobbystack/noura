# AI runtime architecture

AI chat runs in the desktop app's own process. The `Ai` service in `crates/app-core/src/ai.rs` runs each answer, and `crates/local-core` sends the provider requests and holds the credentials. The AI plugin in `crates/plugins/ai` draws the chat through the `ai` interface, while Noura's own code draws the settings, the consent prompt and the tool prompt.

```text
AI plugin (crates/plugins/ai)       chat page, through the noura:plugin `ai` interface
   │
app-core Ai service                 runs, consent, tool approval, chat records
   │
local-core                          provider stream (genai), keyring, web access
```

## Boundaries

The run loop, the provider requests, the credentials and every durable write stay in Rust on your device:

- **Run loop**: each answer runs on a thread of its own, and each round of tool calls on another
- **Plugin access**: a plugin reaches the service only through the `ai` interface, which the `ai` capability gates, and sees only the runs it started
- **Provider transport**: `local-core` streams through `genai`. A provider endpoint must use HTTPS, or HTTP to a loopback address.
- **Credentials**: the operating system's credential store holds each provider key under the service `org.noura.ai`. `ai/providers.json` in the app data keeps only a reference to it, and `local-core` reads the key only when a request starts.

Noura loads the provider and consent files when the session starts. If it can't read them, AI requests fail with `ai_settings_unreadable` until Noura restarts.

## Consent

Workspace content reaches a provider only after you consent for that workspace and provider. The consent covers the provider's kind, endpoint and model, and the policy version, so changing any of them asks again. `app-core` and `local-core` each check it before a request goes out.

A run waits for your answer before Noura writes or sends anything. Granting starts every run waiting on that provider. Denying, pressing Escape or clicking outside the prompt ends those runs and writes nothing. Revoking consent cancels the provider's running streams.

## Instructions and context

Each request starts with Noura's base instructions. Then come the body of the workspace's root `AGENTS.md`, unless the file is missing, empty or unreadable, and the newest context summary, if any. The conversation follows. When a chat grows past the provider's context window, Noura writes a summary message and sends that in place of the older messages.

Plugins add tools, not instructions. A plugin with the `tools` capability offers tools with JSON Schema inputs, and the model sees each one as `<plugin>__<tool>`. Turning a plugin off interrupts the runs it started or that offer its tools, with the code `plugin-disabled`, and later requests offer only the tools of plugins still on.

## Tool approval

You approve each tool call once, as it comes. The prompt shows the tool, its plugin, its description and its input, and it starts on Deny. Closing it, stopping the run or opening another workspace denies the calls still waiting, and a denied call returns `{"denied":true}` to the model. Noura never runs a tool by default.

## Streaming

Each provider stream has a UUID and numbers its frames from 1. The frames pass through a bounded channel of 32, and a send that waits more than 5 seconds ends the stream, so a reader that goes away cancels it. The run thread adds each text delta to the run in memory and wakes the windows. The plugin then reads the text so far and redraws.

Provider errors reach a plugin as a stable code and a fixed message. They never carry provider bodies, request headers or keys.

## Cancellation and recovery

A chat runs one answer at a time, and a second start fails while the first runs. Stopping a run that waits for consent ends it with nothing written. Stopping a stream cancels it and writes the partial text as a `cancelled` message.

Noura recovers an interrupted chat when the plugin next reads it. A message left `in-progress` becomes `interrupted`. A tool call counts as completed only when a completed result with the same run ID, call ID and tool name exists. Noura never runs a call again on its own, and you can resume the chat.

## Durable chat records

Markdown files under `chats/` hold each chat as `chat.md` and its messages under `messages/<date>/`. A new chat writes nothing until its first message goes out. Before a stream opens, Noura writes your message and an `in-progress` assistant record. When the stream ends, it writes the final text and status, then the turn's tool calls. Each tool result reaches disk after its call runs and before the next request goes out.

Every write carries the expected revision of the chat, and of the message when Noura finishes one. A conflicting write retries up to three times. If someone edited the message elsewhere, the answer goes to a new message. Stream deltas never reach disk, and the SQLite index stays a cache that Noura rebuilds from the files.

## Web access

The AI plugin can search the web and read pages through Brave Search. Web access needs a Brave Search key in the credential store and a separate one-time consent for the workspace, and it fails closed until both exist. Noura also sends a call only when a finished run asked for that exact query or URL.

`local-core` applies these limits:

- **Destinations**: HTTPS only, with no user info in the URL
- **Addresses**: no loopback, link-local, private or cloud metadata address, checked on each resolved address and each redirect
- **Size and time**: by default 10 results, 3 redirects, a 5 MiB response, 1 MiB of text and a 15 second timeout
- **Content**: text types only, with HTML turned into text and its links kept as footnotes

Web access never uses browser cookies, browser history or local URL schemes.
