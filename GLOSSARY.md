# Noura

Noura is a local-first workspace for humans and AI. Your work lives in ordinary files, and plugins add the features each workspace needs.

## Language

**Kernel**: The always-on part of Noura that plugins build on: files, the file tree and file viewers, the editor, search, the Inbox, settings, sync, speech, MCP and the plugin system. _Avoid_: core, platform

**Plugin**: A package that adds object types, views, commands or AI tools to a workspace through the public plugin API. Some plugins ship with Noura and others are installed later, but every plugin runs in a sandbox and gets only the capabilities it declares. _Avoid_: module, extension, add-on, built-in plugin

**Capability**: A permission a plugin declares, such as reading the workspace or calling a network host, that you approve when you install the plugin. _Avoid_: permission scope, grant

**Title**: The name of a note, task or project, which is its file name without the extension. A heading inside the file is ordinary text. _Avoid_: heading

**Link**: A reference from one file to another by its title, written `[[Title]]` in a body or a property. Noura keeps it pointing at the same file when that file moves or gets a new title. _Avoid_: wikilink, reference ID

### Orbs

**Orb**: A named AI agent that lives in the workspace as a file, with your instructions, the tools it runs without asking, its memory and its triggers. _Avoid_: agent, bot, assistant

**Thread**: An Orb's one ongoing conversation, a chat that every run of the Orb adds to. _Avoid_: session, conversation

**Run**: One answer of an Orb, from the message that starts it to its last tool call, with a record in the Orb's folder. _Avoid_: job, task, execution

**Trigger**: What starts an Orb's run without you: a schedule, or an event a plugin tells Noura about. _Avoid_: routine, automation, hook

**Memory**: The note beside an Orb that holds what it keeps from one run to the next. _Avoid_: knowledge, context

**Default Orb**: The Orb without a file that plain chats belong to. Its tool calls each wait for your answer. _Avoid_: main chat, Noura agent

### Speech

**Speech**: The part of the kernel that records your microphone and system audio and turns speech into text with models stored on your device. _Avoid_: STT, voice engine

**Dictation**: The plugin that turns what you say into text and pastes it into the app you're typing in, anywhere on your computer. _Avoid_: voice typing, push-to-talk

**Transcription**: The plugin that records a meeting, a call or a lecture, and the object it saves for each recording, holding your notes and its Summary. _Avoid_: meeting, conversation, recording, session

**Transcript**: The word-for-word text of a Transcription, with who spoke and when. _Avoid_: transcription text, captions

**Summary**: The AI-written account of a Transcription that Noura adds below your notes, ending with its next steps. _Avoid_: résumé, enhanced notes, minutes
