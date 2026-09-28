// noura runs as a single-page app inside Tauri and in the hosted browser
// build, so pages render only on the client. Server rendering in dev also
// made Node load workspace packages' TypeScript without Vite.
export const ssr = false;
