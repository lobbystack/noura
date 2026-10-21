//! A note's parse that outlives the call that made it.
//!
//! comrak allocates a tree's nodes in an arena the caller owns, so a parse
//! normally lives only as long as the caller's stack frame. [`Ast`] owns
//! its arena instead, so the editor parses a note once per text version
//! rather than several times a frame, and a parse made on another thread
//! can move to the thread that draws the note.

use std::ptr::NonNull;

use comrak::nodes::AstNode;
use comrak::{Arena, Options};

/// A comrak tree and the arena that holds its nodes.
pub struct Ast {
    /// The tree's root, a node in `arena`. Declared before `arena`, which
    /// must outlive every use of it.
    root: NonNull<AstNode<'static>>,
    /// Every node of the tree. The `'static` stands for the arena's own
    /// lifetime; [`Ast::with_root`] hands nodes out under a lifetime no
    /// longer than a borrow of `self`.
    #[expect(dead_code, reason = "owns the nodes `root` points into")]
    arena: Box<Arena<'static>>,
    /// The text version this tree is a parse of.
    text_seq: u64,
}

// SAFETY: every reference the tree holds points into `arena`, which the
// `Ast` owns and moves with it, and no reference into the tree escapes a
// borrow of the `Ast` (see `with_root`). Moving the whole `Ast` to another
// thread therefore moves everything a node can reach. comrak's nodes hold
// no `Rc` or thread-bound data, only `Cell`, `RefCell` and owned values.
unsafe impl Send for Ast {}

impl Ast {
    /// Parses `text` with `options`, as a parse of text version
    /// `text_seq`.
    pub fn parse(text: &str, options: &Options, text_seq: u64) -> Self {
        let arena: Box<Arena<'static>> = Box::new(Arena::new());
        // SAFETY: the arena lives on the heap at a stable address for as
        // long as `self.arena` holds it, which outlasts every node reference
        // made from this borrow (see the field order and `with_root`).
        let arena_ref: &'static Arena<'static> = unsafe { &*(&*arena as *const Arena<'static>) };
        let root = comrak::parse_document(arena_ref, text, options);
        Self {
            root: NonNull::from(root),
            arena,
            text_seq,
        }
    }

    /// The text version this tree is a parse of.
    pub fn text_seq(&self) -> u64 {
        self.text_seq
    }

    /// Runs `work` with the tree's root.
    ///
    /// The lifetime `work` sees is one it can't name or outlive, so it can
    /// read and walk the tree but can't keep a node, or link in a node from
    /// an arena that dies before this one.
    pub fn with_root<R>(&self, work: impl for<'a> FnOnce(&'a AstNode<'a>) -> R) -> R {
        // SAFETY: the root lives in `self.arena`, which outlives this borrow
        // of `self`; the higher-ranked lifetime keeps every node reference
        // inside `work`.
        let root: &AstNode<'_> = unsafe { &*(self.root.as_ptr() as *const AstNode<'_>) };
        work(root)
    }
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
        let ast = Ast::parse("# Title\n\nSome *text*.\n", &Options::default(), 7);
        let count = |ast: &Ast| ast.with_root(|root| root.descendants().count());
        let here = count(&ast);

        let there = std::thread::spawn(move || (count(&ast), ast.text_seq()))
            .join()
            .unwrap();

        assert_eq!(there, (here, 7));
        assert!(here > 3);
    }
}
