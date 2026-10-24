//! The notes the syntax tests walk: the Markdown from the legacy editor's
//! preview tests whose behavior survives, Obsidian-style syntax, and
//! syntax the editor doesn't know. `editor-egui`'s tests read the same
//! list.

/// Each note, with where it comes from or what it holds.
pub const NOTES: &[(&str, &str)] = &[
    (
        "packages/editor/src/preview-view.test.ts KITCHEN_SINK",
        "---\ntitle: Kitchen sink\ntags: [a, b]\n---\n\n# Heading with **bold** and *italic*\n\n\
         > [!note] Callout title\n> Callout body with [[Link|alias]] and #tag\n\n\
         > quote line one\n> quote line two\n\n\
         - [ ] open task\n* [x] done task\n1. [ ] ordered task\n- [ ]\n   1. nested\n\n\
         ```ts\nconst answer: number = 42; // comment\n```\n\n$$\nx^2 + y^2 = z^2\n$$\n\n\
         Inline $a+b$ and `code` and ==highlight== and %%hidden%% and <u>u</u>.\n\n\
         ![Alt](pic.png \"Title\") ![[photo.jpg|200]] ![[Other note]]\n\n\
         [a link](https://example.com) and https://bare.example.com\n\n\
         | A | B |\n| - | - |\n| 1 | 2 |\n\nSetext\n===\n\n---\n\nLast line ^block-id\n",
    ),
    (
        "packages/editor/src/preview.test.ts table",
        "| A | B |\n| - | - |\n| 1 | 2 |\n",
    ),
    (
        "packages/editor/src/preview.test.ts display math",
        "Intro\n\n$$x + y$$\n\nOutro\n",
    ),
    (
        "packages/editor/src/preview.test.ts embeds",
        "![[Notes/index]]\n\nprose ![[Note]] tail\n",
    ),
    (
        "packages/editor/src/preview.test.ts padded embed",
        "  ![[Note]]  \n",
    ),
    (
        "packages/editor/src/preview.test.ts code span",
        "Fenced-looking `[[Note]] $x$ ==h== #tag %%c%%` prose\n",
    ),
    (
        "packages/editor/src/preview.test.ts hidden comment",
        "before %%secret **bold**%% after\n",
    ),
    (
        "packages/editor/src/preview.test.ts inline math",
        "$x$ here\n",
    ),
    (
        "packages/editor/src/preview.test.ts frontmatter",
        "---\ntitle: Plan\ntags: [a, \"#b\"]\nlink: \"[[Note]]\"\n---\n\nBody #tag\n",
    ),
    (
        "packages/editor/src/preview.test.ts unclosed frontmatter",
        "---\ntext\n",
    ),
    (
        "packages/editor/src/preview.test.ts multi-line math",
        "Intro\n\n$$\nx^2 + y^2\n$$\n\nOutro\n",
    ),
    (
        "packages/editor/src/preview.test.ts math emphasis",
        "$$\na*b*c #x\n$$\n\nafter\n",
    ),
    (
        "packages/editor/src/preview.test.ts blockquote",
        "> one\n> two\n> three\n\nafter\n",
    ),
    (
        "packages/editor/src/preview.test.ts callout",
        "> [!note] Title\n> body\n\nafter\n",
    ),
    (
        "packages/editor/src/preview.test.ts folded callout",
        "> [!warning]- Careful\n> body\n\nafter\n",
    ),
    (
        "packages/editor/src/preview.test.ts fenced code",
        "```js\nconst a = 1;\n```\n\nafter\n",
    ),
    (
        "packages/editor/src/preview.test.ts images",
        "![Alt](pic.png \"A title\")\n\n![[photo.jpg|300]]\n\nend\n",
    ),
    (
        "packages/editor/src/preview.test.ts link",
        "See [the docs](https://example.com) now\n",
    ),
    (
        "packages/editor/src/preview.test.ts tasks",
        "- [ ] one\n* [x] two\n+ [ ] three\n1. [X] four\n- [ ]\n\nend\n",
    ),
    (
        "packages/editor/src/preview.test.ts currency",
        "It costs $5 or $10 today\n\nUSD $5 and CAD $10\n",
    ),
    (
        "highlight",
        "A ==highlight== here, ==two words== there, and ==**bold** inside==.\n\n\
         Not in `==code==`, $a==b==c$ or https://example.com/a==b==c.\n",
    ),
    (
        "callouts",
        "> [!info] Any type\n> body\n\n> [!tip]+ Open\n> > nested quote\n\n\
         > [!faq]-\n> folded, no title\n\n>[!note] no space\n\n> not [!a] callout\n",
    ),
    (
        "display math holding lines that look like blocks",
        "$$\na\n+ b\n- c\n> d\n# e\n===\n   \n1. f\n$$\n\n```\n$$\n- g\n$$\n```\n",
    ),
    (
        "callouts over setext headings",
        "> [!note] Title\n> ---\n\n> [!tip] Tip\n> text\n> ===\n> body\n",
    ),
    (
        "wikilinks",
        "[[target#heading|alias]] [[plain]] [[folder/note]] [[#heading]] [[note|]]\n",
    ),
    (
        "obsidian inline",
        "A #tag, #nested/tag and #2024 and a#b, %%comment%% and <u>under **bold**</u>.\n\n\
         Ends with a block ID ^abc-123\n\n# Heading #tag ^id\n",
    ),
    (
        "unknown syntax",
        "A ||spoiler|| and :smile: and x^2^ and H~2~O.\n\n\
         A footnote[^1] and {{template}} and <span class=\"x\">html</span>.\n\n\
         [^1]: The footnote.\n\nTerm\n: Definition\n\n[ref]: https://example.com\n\n\
         :::note\nfenced div\n:::\n\n@[toc]\n\n<details><summary>More</summary>\n</details>\n\n\
         >>> not a multiline quote\n\n# Folded <!-- {\"fold\":true} -->\n\nhidden\n",
    ),
    (
        "lockbook behaviors",
        "Setext\n===\n\nSub\n---\n\n__strong__ and _em_\nsoft\nbreak \u{a0}nbsp\n",
    ),
    (
        "line endings",
        "a\rb\r\n> [!note] A\r\n> body\r\n\r\n| a | b |\r\n|---|---|\r\n| 1 | 2 |\r\n",
    ),
];
