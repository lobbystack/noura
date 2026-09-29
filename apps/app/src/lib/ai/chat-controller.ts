// The chat controller pulls in the Pi agent runtime and TypeBox. The chat
// store imports this module on the first send, so opening the app, and
// even the AI page, never downloads that code.
export { PiChatController } from '@noura/ai';
