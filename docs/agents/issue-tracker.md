# Issue tracker: Noura-Internal

Specs and tickets for this repository live as Markdown files in the private Noura-Internal workspace, an Obsidian vault attached to the session as an extra directory. The repository is public, so keep specs and tickets out of it and out of GitHub Issues.

If no Noura-Internal directory is attached to the session, stop and ask the owner for its location.

## Layout

Each feature follows the same layout:

- Each feature gets a folder: `Specs/<feature-slug>/`.
- The spec is `Specs/<feature-slug>/spec.md`.
- Each ticket is its own file: `Specs/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01`.
- A ticket starts with a `Status:` line set to `ready-for-agent`, `in-progress` or `done`.
- A blocked ticket adds a `Blocked by: NN, NN` line under its status.
- Discussion goes at the end of the file under a `## Comments` heading.

## Publish a spec or ticket

Create the file under `Specs/<feature-slug>/`, adding folders as needed.

## Fetch a ticket

Read the file at the path or number the owner gives you.
