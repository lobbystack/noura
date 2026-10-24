//! A note's parse that outlives the call that made it.
//!
//! comrak allocates a tree's nodes in an arena the caller owns, so a parse
//! normally lives only as long as the caller's stack frame. [`Ast`] owns
//! its arena instead, so the editor parses a note once per text version
//! rather than several times a frame, a parse made on another thread can
//! move to the thread that draws the note, and an edit can reparse only
//! the blocks it touches ([`Ast::update`]).

use std::ops::Range;
use std::ptr::NonNull;

use comrak::nodes::AstNode;
use comrak::{Arena, Options};

/// A comrak tree, the arena that holds its nodes, and the text it parsed.
pub struct Ast {
    /// The tree's root, a node in `arena`. Declared before `arena`, which
    /// must outlive every use of it.
    root: NonNull<AstNode<'static>>,
    /// Every node of the tree. The `'static` stands for the arena's own
    /// lifetime; [`Ast::with_root`] hands nodes out under a lifetime no
    /// longer than a borrow of `self`.
    arena: Box<Arena<'static>>,
    /// The text version this tree is a parse of.
    text_seq: u64,
    /// The text this tree is a parse of.
    source: String,
    /// Where each line of `source` starts, as comrak counts lines.
    line_starts: Vec<usize>,
}

// SAFETY: every reference the tree holds points into `arena`, which the
// `Ast` owns and moves with it, and no reference into the tree escapes a
// borrow of the `Ast` (see `with_root`). Moving the whole `Ast` to another
// thread therefore moves everything a node can reach. comrak's nodes hold
// no `Rc` or thread-bound data, only `Cell`, `RefCell` and owned values.
unsafe impl Send for Ast {}

/// The most text [`Ast::update`] reparses to close the blocks it reparses
/// the way a full parse would. Parsing this much takes a small part of a
/// frame.
const MAX_RUN_BYTES: usize = 32 * 1024;

/// What [`Ast::update`] reparsed: a run of top-level blocks and the lines
/// around them, in the text before the update and after it. Everything
/// before the run kept its place; everything after it moved by the
/// difference in lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reparsed {
    /// The lines the reparsed run covered before the update, 0-based.
    pub old_lines: Range<usize>,
    /// The lines it covers now.
    pub new_lines: Range<usize>,
    /// The top-level blocks that replaced the run, as indices among the
    /// root's children.
    pub blocks: Range<usize>,
    /// How many top-level blocks the run held before.
    pub replaced: usize,
}

impl Ast {
    /// Parses `source` with `options`, as a parse of text version
    /// `text_seq`.
    pub fn parse(source: String, options: &Options, text_seq: u64) -> Self {
        let arena: Box<Arena<'static>> = Box::new(Arena::new());
        let root = comrak::parse_document(arena_ref(&arena), &source, options);
        let line_starts = line_starts(&source);
        Self {
            root: NonNull::from(root),
            arena,
            text_seq,
            source,
            line_starts,
        }
    }

    /// The text version this tree is a parse of.
    pub fn text_seq(&self) -> u64 {
        self.text_seq
    }

    /// Marks the tree as a parse of text version `text_seq`, for a parse
    /// made elsewhere of the same text.
    pub fn set_text_seq(&mut self, text_seq: u64) {
        self.text_seq = text_seq;
    }

    /// The text this tree is a parse of.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Where line `line` of the source starts, 0-based, as comrak counts
    /// lines.
    pub(crate) fn line_start(&self, line: usize) -> Option<usize> {
        self.line_starts.get(line).copied()
    }

    /// Runs `work` with the tree's root.
    ///
    /// The lifetime `work` sees is one it can't name or outlive, so it can
    /// read and walk the tree but can't keep a node, or link in a node from
    /// an arena that dies before this one.
    pub fn with_root<R>(&self, work: impl for<'a> FnOnce(&'a AstNode<'a>) -> R) -> R {
        work(self.root())
    }

    fn root(&self) -> &AstNode<'_> {
        // SAFETY: the root lives in `self.arena`, which outlives this borrow
        // of `self`.
        unsafe { &*(self.root.as_ptr() as *const AstNode<'_>) }
    }

    /// Makes the tree a parse of `source`, text version `text_seq`, by
    /// reparsing only the top-level blocks the change from the old text
    /// reaches, with the blocks on either side for context, and moving the
    /// blocks after them to their new lines. `options` are the ones the
    /// whole text was parsed with.
    ///
    /// The result is the tree a full parse gives unless the change reaches
    /// past the reparsed blocks, as an unclosed code fence or a new link
    /// reference definition does. A full parse of the same text, made in
    /// the background, corrects those.
    pub fn update(&mut self, source: String, options: &Options, text_seq: u64) -> Reparsed {
        let old = self.source.as_bytes();
        let new = source.as_bytes();
        let prefix = common_prefix(old, new);
        let suffix = common_suffix(&old[prefix..], &new[prefix..]);
        let old_change_end = old.len() - suffix;
        let new_starts = line_starts(&source);
        let old_lines = line_count(&self.line_starts, old.len());
        let new_line_total = line_count(&new_starts, new.len());
        let delta = new_line_total as isize - old_lines as isize;
        let first_changed = line_of(&self.line_starts, prefix).min(old_lines.saturating_sub(1));
        let last_changed = line_of(&self.line_starts, old_change_end)
            .min(old_lines.saturating_sub(1))
            .max(first_changed);

        // SAFETY: the nodes live in `self.arena`, which outlives this call;
        // none of them escapes it.
        let root: &'static AstNode<'static> = unsafe { &*self.root.as_ptr() };
        let blocks: Vec<(&AstNode<'static>, usize, usize)> = root
            .children()
            .map(|child| {
                let pos = child.data.borrow().sourcepos;
                let start = pos.start.line.saturating_sub(1);
                (child, start, pos.end.line.saturating_sub(1).max(start))
            })
            .collect();
        // The blocks the change reaches, with one more on each side.
        let before = blocks.partition_point(|(_, _, end)| *end < first_changed);
        let through = blocks.partition_point(|(_, start, _)| *start <= last_changed);
        let first_block = before.saturating_sub(1);
        let mut after_block = (through.max(before) + 1).min(blocks.len());

        let start_line = match first_block.checked_sub(1) {
            Some(previous) => blocks[previous].2 + 1,
            None => 0,
        };
        let mut region_options = options.clone();
        if start_line > 0 {
            region_options.extension.front_matter_delimiter = None;
        }
        // Parse the run into this tree's arena, with the first line of the
        // block after it: that line closes the run's last block as it does
        // in a full parse. When the line joins the run instead, as after an
        // opening code fence, the run takes in more blocks, twice as many
        // each time, and parses again. Past `MAX_RUN_BYTES` it stops and
        // parses without that line, so its last block ends with the run,
        // and the background parse brings in the rest.
        let mut more = 1;
        let (old_end_line, new_end_line, parsed, new_blocks) = loop {
            let (old_end_line, new_end_line) = match blocks.get(after_block) {
                Some((_, next_start, _)) => (*next_start, (*next_start as isize + delta) as usize),
                None => (old_lines, new_line_total),
            };
            let run_bytes = byte_of_line(&new_starts, new_end_line, new.len())
                - byte_of_line(&new_starts, start_line, new.len());
            let closing_line = after_block < blocks.len() && run_bytes <= MAX_RUN_BYTES;
            let with_next_line = if closing_line {
                new_end_line + 1
            } else {
                new_end_line
            };
            let region = byte_of_line(&new_starts, start_line, new.len())
                ..byte_of_line(&new_starts, with_next_line, new.len());
            let parsed =
                comrak::parse_document(arena_ref(&self.arena), &source[region], &region_options);
            // Lines of the run, 1-based within it as comrak counts them.
            let run_lines = new_end_line - start_line;
            let new_blocks: Vec<&AstNode<'static>> = parsed
                .children()
                .filter(|block| block.data.borrow().sourcepos.start.line <= run_lines)
                .collect();
            let closed = new_blocks.last().is_none_or(|last| {
                let end = last.data.borrow().sourcepos.end;
                let end_line = if end.column == 0 {
                    end.line.saturating_sub(1)
                } else {
                    end.line
                };
                end_line <= run_lines
            });
            if closed || !closing_line {
                break (old_end_line, new_end_line, parsed, new_blocks);
            }
            after_block = (after_block + more).min(blocks.len());
            more *= 2;
        };
        for block in &new_blocks {
            shift_lines(block, start_line as isize);
        }

        // Swap the run's old blocks for the new ones and move the blocks
        // after them.
        let next = blocks.get(after_block).map(|(node, _, _)| *node);
        for (node, _, _) in &blocks[first_block..after_block] {
            node.detach();
        }
        for block in &new_blocks {
            match next {
                Some(next) => next.insert_before(block),
                None => root.append(block),
            }
        }
        if delta != 0 {
            for (node, _, _) in &blocks[after_block..] {
                shift_lines(node, delta);
            }
        }
        {
            let mut data = root.data.borrow_mut();
            if next.is_none() {
                let end = parsed.data.borrow().sourcepos.end;
                data.sourcepos.end.line = end.line + start_line;
                data.sourcepos.end.column = end.column;
            } else {
                data.sourcepos.end.line = (data.sourcepos.end.line as isize + delta) as usize;
            }
        }

        self.source = source;
        self.line_starts = new_starts;
        self.text_seq = text_seq;
        Reparsed {
            old_lines: start_line..old_end_line,
            new_lines: start_line..new_end_line,
            blocks: first_block..first_block + new_blocks.len(),
            replaced: after_block - first_block,
        }
    }
}

/// `arena` under the lifetime the tree's nodes carry.
fn arena_ref(arena: &Arena<'static>) -> &'static Arena<'static> {
    // SAFETY: the arena lives on the heap at a stable address for as long as
    // its `Ast` holds it, which outlasts every node reference made from this
    // borrow (see the field order and `with_root`).
    unsafe { &*(arena as *const Arena<'static>) }
}

/// Moves `node` and everything in it `by` lines.
fn shift_lines<'a>(node: &'a AstNode<'a>, by: isize) {
    let shift = |pos: &mut comrak::nodes::Sourcepos| {
        for line in [&mut pos.start.line, &mut pos.end.line] {
            if *line > 0 {
                *line = (*line as isize + by) as usize;
            }
        }
    };
    for node in node.descendants() {
        let mut data = node.data.borrow_mut();
        shift(&mut data.sourcepos);
        if let comrak::nodes::NodeValue::TaskItem(task) = &mut data.value {
            shift(&mut task.symbol_sourcepos);
        }
    }
}

/// Where each line of `text` starts. Lines end at `\n`, `\r\n` or a lone
/// `\r`, as CommonMark and comrak count them. A text that ends with a line
/// ending also lists where the empty line after it would start.
fn line_starts(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = Vec::with_capacity(bytes.len() / 32 + 1);
    starts.push(0);
    for at in memchr::memchr2_iter(b'\n', b'\r', bytes) {
        if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
            continue;
        }
        starts.push(at + 1);
    }
    starts
}

/// How many lines comrak sees in a text of `len` bytes whose lines start
/// at `starts`: a final line ending starts no line of its own.
fn line_count(starts: &[usize], len: usize) -> usize {
    if starts.len() > 1 && starts[starts.len() - 1] == len {
        starts.len() - 1
    } else {
        starts.len()
    }
}

/// The 0-based line that holds byte `at`.
fn line_of(starts: &[usize], at: usize) -> usize {
    starts
        .partition_point(|start| *start <= at)
        .saturating_sub(1)
}

/// Where line `line` starts, or the end of the text past the last line.
fn byte_of_line(starts: &[usize], line: usize, len: usize) -> usize {
    starts.get(line).copied().unwrap_or(len).min(len)
}

fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    const CHUNK: usize = 64;
    let mut at = 0;
    while at + CHUNK <= a.len().min(b.len()) && a[at..at + CHUNK] == b[at..at + CHUNK] {
        at += CHUNK;
    }
    at + a[at..]
        .iter()
        .zip(&b[at..])
        .take_while(|(x, y)| x == y)
        .count()
}

fn common_suffix(a: &[u8], b: &[u8]) -> usize {
    const CHUNK: usize = 64;
    let len = a.len().min(b.len());
    let mut at = 0;
    while at + CHUNK <= len
        && a[a.len() - at - CHUNK..a.len() - at] == b[b.len() - at - CHUNK..b.len() - at]
    {
        at += CHUNK;
    }
    at + a[..a.len() - at]
        .iter()
        .rev()
        .zip(b[..b.len() - at].iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

impl std::fmt::Debug for Ast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ast")
            .field("text_seq", &self.text_seq)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parse_moves_to_another_thread_and_reads_the_same_there() {
        let ast = Ast::parse("# Title\n\nSome *text*.\n".into(), &Options::default(), 7);
        let count = |ast: &Ast| ast.with_root(|root| root.descendants().count());
        let here = count(&ast);

        let there = std::thread::spawn(move || (count(&ast), ast.text_seq()))
            .join()
            .unwrap();

        assert_eq!(there, (here, 7));
        assert!(here > 3);
    }

    #[test]
    fn lines_end_at_lf_crlf_and_a_lone_cr() {
        assert_eq!(line_starts("a\nb\r\nc\rd"), [0, 2, 5, 7]);
        assert_eq!(line_count(&line_starts("a\n"), 2), 1);
        assert_eq!(line_count(&line_starts("a"), 1), 1);
        assert_eq!(line_count(&line_starts(""), 0), 1);
    }
}
