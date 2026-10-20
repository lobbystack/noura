//! Indent math. Two units that diverge once tabs are involved:
//! **graphemes** for slicing source, **columns** for nesting depth
//! (CommonMark §2.2: tabs in indent positions act like spaces to
//! the next 4-col stop). Structural prefix helpers live in
//! [`super::block::container`].

pub(crate) mod wrap_layout;

pub use editor_core::nodes::NodeValueExt;

#[cfg(test)]
mod test {
    #[test]
    fn range_lines_char_no_newline() {
        let text = "*";
        let md = crate::MdRender::test(text);
        let lines = md.range_split_newlines((0.into(), text.len().into()));
        assert_eq!(lines, vec![(0.into(), 1.into())]);
    }

    #[test]
    fn range_lines_char_newline() {
        let text = "*\n";
        let md = crate::MdRender::test(text);
        let lines = md.range_split_newlines((0.into(), text.len().into()));
        assert_eq!(lines, vec![(0.into(), 1.into()), (2.into(), 2.into())]);
    }
}
