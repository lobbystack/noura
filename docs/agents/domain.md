# Domain docs

Noura uses a single context with three kinds of domain documents:

- `GLOSSARY.md` at the repository root defines Noura’s domain terms. The repository is public, so write entries that describe the product and leave internal plans out.
- Architecture decision records (ADRs) live in the Noura-Internal workspace under `Architecture and Design/`, named `ADR NNNN - Title.md`. Number a new ADR one above the highest existing number.
- Public architecture and format documents live in `docs/architecture/` and `docs/workspace-format/`. They describe current behavior. ADRs record why a decision was made.

## Before you explore

Read `GLOSSARY.md` and the ADRs that touch the area you’re working in. If a file doesn’t exist yet, continue without mentioning it.

## While you work

- Name domain concepts with the glossary’s terms in specs, tickets, test names and proposals.
- When your work contradicts an ADR, say so and name the ADR.
