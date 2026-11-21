# AI runtime architecture

AI chat and Orbs run in the desktop app's own process. The `Ai` service in `crates/app-core/src/ai.rs` runs each answer, `crates/app-core/src/ai/orbs.rs` runs Orbs and their triggers, and `crates/local-core` sends the provider requests and holds the credentials. The AI plugin in `crates/plugins/ai` draws the chat through the `ai` interface, while Noura's own code draws the settings and the tool prompt.

```text
AI plugin (crates/plugins/ai)       chat page, through the noura:plugin `ai` interface
   │
app-core Ai service                 runs, completions, tool approval, chat records,
   │                                Orbs, their triggers and run records
local-core                          provider stream (genai), keyring, web access
```

## Boundaries

The run loop, the provider requests, the credentials and every durable write stay in Rust on your device:

- **Run loop**: each answer runs on a thread of its own, and each round of tool calls on another
- **Plugin access**: a plugin reaches the service only through the `ai` interface, which the `ai` capability gates, and sees only the runs it started
- **Provider transport**: `local-core` streams through `genai`. A provider endpoint must use HTTPS, or HTTP to a loopback address.
- **Credentials**: the operating system's credential store holds each provider key under the service `org.noura.ai`. `ai/providers.json` in the app data keeps only a reference to it, and `local-core` reads the key only when a request starts.

Noura loads the provider file when the session starts. If it can't read them, AI requests fail with `ai_settings_unreadable` until Noura restarts.

## Authorization

Setting up a provider is your say-so (ADR 0006). A run writes your message and sends the request without a prompt, whatever workspace content it carries. Removing a provider cancels its running streams, and a later request to it fails with `provider_not_found`.

## Completions

`complete` in the `ai` interface sends one prompt to a provider and returns its answer, with no chat written and nothing kept. The request holds Noura's base instructions, the plugin's instructions and the prompt. With `json`, Noura asks for one JSON value and refuses an answer that doesn't parse with `invalid_json`. A completion gives up after two minutes with `provider_timed_out`.

## Instructions and context

Each request of a plain chat starts with Noura's base instructions. Then come the body of the workspace's root `AGENTS.md`, unless the file is missing, empty or unreadable, and the newest context summary, if any. The conversation follows. When a chat grows past the provider's context window, Noura writes a summary message and sends that in place of the older messages.

A request of an Orb's run starts with Noura's Orb instructions, which name the Orb, then the root `AGENTS.md`, the Orb's own instructions from its file's body, and its memory file, up to 20,000 characters. Noura reads them again for each request, so a line the Orb remembered shows in its next one.

Plugins add tools, not instructions. A plugin with the `tools` capability offers tools with JSON Schema inputs, and the model sees each one as `<plugin>__<tool>`. Turning a plugin off interrupts the runs it started or that offer its tools, with the code `plugin-disabled`, and later requests offer only the tools of plugins still on.

## Tool approval

A plain chat belongs to the default Orb, and you approve each of its tool calls once, as it comes. The prompt shows the tool, its plugin, its description and its input, and it starts on Deny. Closing it, stopping the run or opening another workspace denies the calls still waiting, and a denied call returns `{"denied":true}` to the model.

An Orb runs its tools without asking (ADR 0007). Its file lists the tools it may run, and the model sees no others. When the model asks for calls, the service runs each one, writes its result to the Orb's thread and asks the model again, up to 16 rounds, whether you wrote to the Orb or a trigger started the run. Creating the Orb and writing its tools and triggers is your authorization. Plugins can't write Orb files, so no model tool can widen it.

## Orbs

An Orb is a named agent in the workspace: `Orbs/<name>/<name>.md`, its `Memory.md`, its `Heartbeat.md` checklist and a `Runs` folder of records, as [the workspace format](../workspace-format/v1.md#orbs) describes. Each Orb keeps one thread, a chat whose `orb_id` names it.

- **Noura's tools**: `noura__read` reads a note or object that the Orb's `reads` allow, `noura__remember` adds a line to its memory file, and `noura__web_search` and `noura__web_fetch` reach the web through the limits below. An Orb gets each one only when its file lists it.
- **Run records**: Noura writes a run's record before its first message and again when it ends, with its status, how many tool calls it made and a line for each. One run at a time works per Orb.
- **Triggers**: a thread checks them every 30 seconds while a workspace is open, and at once when a plugin emits an event. A schedule catches up once on the last time it missed. An event starts each Orb once per object revision. A trigger key in each record keeps a firing from running twice, and triggers start at most 20 runs per Orb an hour.
- **Loops**: an event that a plugin emits during an Orb's tool call starts no Orb.
- **A while away**: after 7 days without you on this device, Noura holds every Orb's triggers until you answer whether to keep them running. The time you were last active and the hold live in `ai/orbs.json` in the app data.
- **Quits**: a run that Noura's quit left running answers again once at the next check, 10 seconds after the workspace opens. A run that quit during a tool call ends `interrupted` with `orb_interrupted`, and Noura never runs the call again.

## Streaming

Each provider stream has a UUID and numbers its frames from 1. The frames pass through a bounded channel of 32, and a send that waits more than 5 seconds ends the stream, so a reader that goes away cancels it. The run thread adds each text delta to the run in memory and wakes the windows. The plugin then reads the text so far and redraws.

Provider errors reach a plugin as a stable code and a fixed message. They never carry provider bodies, request headers or keys.

## Cancellation and recovery

A chat runs one answer at a time, and a second start fails while the first runs. Stopping a stream cancels it and writes the partial text as a `cancelled` message.

Noura recovers an interrupted chat when the plugin next reads it. A message left `in-progress` becomes `interrupted`. A tool call counts as completed only when a completed result with the same run ID, call ID and tool name exists. Noura never runs a call again on its own, and you can resume the chat.

## Durable chat records

Markdown files under `Chats/` hold each chat as `chat.md` and its messages under `messages/<date>/`. A new chat writes nothing until its first message goes out. Before a stream opens, Noura writes your message and an `in-progress` assistant record. When the stream ends, it writes the final text and status, then the turn's tool calls. Each tool result reaches disk after its call runs and before the next request goes out.

Every write carries the expected revision of the chat, and of the message when Noura finishes one. A conflicting write retries up to three times. If someone edited the message elsewhere, the answer goes to a new message. Stream deltas never reach disk, and the SQLite index stays a cache that Noura rebuilds from the files.

## Web access

The AI plugin and Orbs can search the web and read pages through Brave Search. Web access needs a Brave Search key in the credential store, and it fails closed until the key exists. Noura also sends a call only when a finished run asked for that exact query or URL. An Orb's web calls go out without a prompt, as its other tools do.

`local-core` applies these limits:

- **Destinations**: HTTPS only, with no user info in the URL
- **Addresses**: no loopback, link-local, private or cloud metadata address, checked on each resolved address and each redirect
- **Size and time**: by default 10 results, 3 redirects, a 5 MiB response, 1 MiB of text and a 15 second timeout
- **Content**: text types only, with HTML turned into text and its links kept as footnotes

Web access never uses browser cookies, browser history or local URL schemes.
