# Verify the desktop Local Alpha

Use this runbook for contributor and release review of the desktop Local Alpha. Use synthetic content only.

## Prepare the test environment

1. Run the automated gates from the repository root:

   ```sh
   bun run check
   bun run test
   bun run build
   bun run format:check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   bun run bindings:check
   bun run licenses:check
   ```

2. Run the shared-engine proof by itself when diagnosing Model Context Protocol (MCP) or watcher failures:

   ```sh
   cargo test -p noura-mcp local_app_and_mcp_share_canonical_files_and_rebuilt_projections
   ```

3. Start the desktop app:

   ```sh
   bun run tauri dev
   ```

4. Create a disposable workspace. Keep `.noura/workspace.yaml` open so its workspace identifier (ID) and enabled plugins remain visible.

## Verify canonical content

1. Create a project named `Alpha project`.
2. Create a task named `Review local alpha` in that project. Set its status to `todo`, priority to `high`, and due date to today.
3. Create a note named `Syntax sample`. Add headings, emphasis, a link, a task list, a fenced code block, and a table.
4. Open the workspace folder outside Noura. Confirm that each object is an ordinary Markdown file with a stable frontmatter ID and the expected body.

## Verify calendar views

1. Open Calendar and choose Month, Week, then Day.
2. Confirm each view uses the same focus date. Confirm Today resets it and previous/next uses the active view's unit.
3. Add a date-only entry with a multi-day exclusive end. Confirm it appears on every covered date and never shifts because of the device timezone.
4. Add a timed entry that crosses midnight. Confirm it appears on both intersected days, after all-day entries.
5. Resize the desktop window to a narrow width. Confirm the Month grid scrolls without collapsing and the Week and Day agendas remain readable.
6. Select a Calendar item. Confirm its canonical object tab and inspector open.

## Verify live projections and conflicts

1. Edit the task file in an external editor. Change its title, due date, status, and project, then save.
2. Without reloading Noura, confirm Inbox, Tasks, Projects, Calendar, and the workspace tree adopt the file change.
3. Start an edit in Noura, change the same content externally, then save in Noura. Confirm conflict review opens and no conflict markers reach the Markdown file.
4. Choose the local resolution. Confirm the local version becomes canonical.
5. Repeat the conflict, choose the external resolution, and confirm the external version becomes canonical.
6. Put Noura in the background, make another external edit, and return to it. Confirm focus recovery refreshes every projection.

## Verify Model Context Protocol mutations

1. Connect an MCP client to:

   ```sh
   cargo run -p noura-mcp -- --workspace /path/to/disposable_workspace
   ```

2. Call `workspace.read` for `Review local alpha`, then call `tasks.update` with its current revision. Change the title and status.
3. Call `tasks.create` with the project ID and a date in the visible Calendar range.
4. Confirm the open desktop app updates Tasks, Projects, Calendar, Inbox, and search without a reload.
5. Inspect both task files and confirm the MCP results match their Markdown bytes.

## Verify disposable-index recovery

1. In Settings, choose **Rebuild index**. Confirm the button stays disabled while running and a success toast appears. Confirm every projection refreshes.
2. Record each object's ID, path, body, and dated properties.
3. Quit Noura.
4. Find the workspace’s local index by matching the ID from `.noura/workspace.yaml`. On macOS, search the application-data directory with:

   ```sh
   find "$HOME/Library/Application Support" -path "*/workspaces/*/index.sqlite" -print
   ```

   On other platforms, locate the index in Noura’s operating system application-data directory. Match the workspace ID before deleting any index.

5. Delete only that workspace's `index.sqlite`, then reopen Noura and the disposable workspace.
6. Confirm notes, tasks, projects, IDs, paths, bodies, search results, Calendar results, and project boards match the recorded state. Calendar may return to Month because its view preference is disposable.

## Record the results

Record one result for every row. Resolve failures before handoff.

| Check                        | Result      | Evidence or issue |
| ---------------------------- | ----------- | ----------------- |
| Automated gates              | Pass / Fail |                   |
| Canonical Markdown           | Pass / Fail |                   |
| Month view                   | Pass / Fail |                   |
| Week view                    | Pass / Fail |                   |
| Day view                     | Pass / Fail |                   |
| Narrow desktop width         | Pass / Fail |                   |
| External-edit refresh        | Pass / Fail |                   |
| Local conflict resolution    | Pass / Fail |                   |
| External conflict resolution | Pass / Fail |                   |
| MCP mutation refresh         | Pass / Fail |                   |
| Settings rebuild             | Pass / Fail |                   |
| Deleted-index recovery       | Pass / Fail |                   |
