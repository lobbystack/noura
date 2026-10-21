//! Text changes over UTF-16 units, and the transform that lets the editor
//! keep typing while the sandbox works on records ahead of its edits.
//!
//! An [`Op`] covers the whole text it applies to: it retains, deletes or
//! inserts from the start, and its retains and deletes add up to that text's
//! length. [`transform`] takes two changes made to the same text and rewrites
//! each to apply after the other, so both orders reach the same text. Where
//! both insert at one place, the sandbox's text goes first.

use std::fmt;

/// One component of an [`Op`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Component {
    /// Keeps this many UTF-16 units.
    Retain(u32),
    /// Inserts this text.
    Insert(String),
    /// Deletes this many UTF-16 units.
    Delete(u32),
}

/// A change to a text, over UTF-16 units.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Op {
    components: Vec<Component>,
    base_len: u32,
    target_len: u32,
}

impl fmt::Debug for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.components).finish()
    }
}

/// A change in the form the editor makes them: `removed` UTF-16 units at
/// `start` give way to `inserted`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Splice {
    /// Where the change starts, in UTF-16 units.
    pub start: u32,
    /// How many UTF-16 units it removes.
    pub removed: u32,
    /// The text it inserts.
    pub inserted: String,
}

/// Why an [`Op`] doesn't fit a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the change doesn't fit the text")]
pub struct Mismatch;

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

impl Op {
    /// The change that keeps all `len` units of a text.
    pub fn identity(len: u32) -> Self {
        let mut op = Self::default();
        op.retain(len);
        op
    }

    /// A change of a text of `len` units made of `splices`, each made
    /// against the text the ones before it left.
    pub fn from_splices(len: u32, splices: &[Splice]) -> Result<Self, Mismatch> {
        let mut op = Self::identity(len);
        for splice in splices {
            let end = splice.start.checked_add(splice.removed).ok_or(Mismatch)?;
            if end > op.target_len {
                return Err(Mismatch);
            }
            let mut next = Self::default();
            next.retain(splice.start);
            next.delete(splice.removed);
            next.insert(&splice.inserted);
            next.retain(op.target_len - end);
            op = compose(&op, &next)?;
        }
        Ok(op)
    }

    /// The change as splices, each against the text the ones before it left.
    pub fn splices(&self) -> Vec<Splice> {
        let mut splices: Vec<Splice> = Vec::new();
        let mut at = 0u32;
        for component in &self.components {
            match component {
                Component::Retain(n) => at += n,
                Component::Delete(n) => match splices.last_mut() {
                    Some(last) if last.start + utf16_len(&last.inserted) == at => last.removed += n,
                    _ => splices.push(Splice {
                        start: at,
                        removed: *n,
                        inserted: String::new(),
                    }),
                },
                Component::Insert(text) => {
                    match splices.last_mut() {
                        Some(last) if last.start + utf16_len(&last.inserted) == at => {
                            last.inserted.push_str(text)
                        }
                        _ => splices.push(Splice {
                            start: at,
                            removed: 0,
                            inserted: text.clone(),
                        }),
                    }
                    at += utf16_len(text);
                }
            }
        }
        splices
    }

    /// The components, in order.
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    /// The length of the text the change applies to.
    pub fn base_len(&self) -> u32 {
        self.base_len
    }

    /// The length of the text the change leaves.
    pub fn target_len(&self) -> u32 {
        self.target_len
    }

    /// Whether the change leaves the text as it was.
    pub fn is_identity(&self) -> bool {
        self.components
            .iter()
            .all(|component| matches!(component, Component::Retain(_)))
    }

    /// Appends a retain, joining it to a retain before it.
    pub fn retain(&mut self, n: u32) {
        if n == 0 {
            return;
        }
        self.base_len += n;
        self.target_len += n;
        if let Some(Component::Retain(last)) = self.components.last_mut() {
            *last += n;
        } else {
            self.components.push(Component::Retain(n));
        }
    }

    /// Appends an insert. An insert sorts before a delete it follows, so
    /// equal changes have equal components.
    pub fn insert(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.target_len += utf16_len(text);
        match self.components.as_mut_slice() {
            [.., Component::Insert(last)] => last.push_str(text),
            [.., Component::Insert(before), Component::Delete(_)] => before.push_str(text),
            [.., Component::Delete(n)] => {
                let n = *n;
                let at = self.components.len() - 1;
                self.components[at] = Component::Insert(text.to_owned());
                self.components.push(Component::Delete(n));
            }
            _ => self.components.push(Component::Insert(text.to_owned())),
        }
    }

    /// Appends a delete, joining it to a delete before it.
    pub fn delete(&mut self, n: u32) {
        if n == 0 {
            return;
        }
        self.base_len += n;
        if let Some(Component::Delete(last)) = self.components.last_mut() {
            *last += n;
        } else {
            self.components.push(Component::Delete(n));
        }
    }

    /// Applies the change to `text`.
    pub fn apply(&self, text: &str) -> Result<String, Mismatch> {
        let units: Vec<u16> = text.encode_utf16().collect();
        if units.len() != self.base_len as usize {
            return Err(Mismatch);
        }
        let mut out: Vec<u16> = Vec::with_capacity(self.target_len as usize);
        let mut at = 0usize;
        for component in &self.components {
            match component {
                Component::Retain(n) => {
                    out.extend_from_slice(&units[at..at + *n as usize]);
                    at += *n as usize;
                }
                Component::Delete(n) => at += *n as usize,
                Component::Insert(text) => out.extend(text.encode_utf16()),
            }
        }
        Ok(String::from_utf16_lossy(&out))
    }

    /// The encoding the guest reads and writes: each component is a tag
    /// byte, then for a retain (0) or a delete (2) a little-endian `u32`
    /// count, and for an insert (1) a little-endian `u32` byte length and
    /// that many bytes of UTF-8.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for component in &self.components {
            match component {
                Component::Retain(n) => {
                    out.push(0);
                    out.extend_from_slice(&n.to_le_bytes());
                }
                Component::Insert(text) => {
                    out.push(1);
                    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
                    out.extend_from_slice(text.as_bytes());
                }
                Component::Delete(n) => {
                    out.push(2);
                    out.extend_from_slice(&n.to_le_bytes());
                }
            }
        }
        out
    }

    /// Reads the guest's encoding of a change to a text of `len` units,
    /// whose last retain the guest leaves out.
    pub fn decode(mut bytes: &[u8], len: u32) -> Result<Self, Mismatch> {
        let mut op = Self::default();
        while let Some((&tag, rest)) = bytes.split_first() {
            let (word, rest) = rest.split_first_chunk::<4>().ok_or(Mismatch)?;
            let n = u32::from_le_bytes(*word);
            bytes = rest;
            match tag {
                0 => op.retain(n),
                1 => {
                    let (text, rest) = bytes.split_at_checked(n as usize).ok_or(Mismatch)?;
                    bytes = rest;
                    op.insert(std::str::from_utf8(text).map_err(|_| Mismatch)?);
                }
                2 => op.delete(n),
                _ => return Err(Mismatch),
            }
        }
        let rest = len.checked_sub(op.base_len).ok_or(Mismatch)?;
        op.retain(rest);
        Ok(op)
    }
}

/// Walks an op's components, splitting them where the other op needs.
struct Cursor<'a> {
    components: &'a [Component],
    index: usize,
    /// How many UTF-16 units of the current component are taken.
    offset: u32,
}

impl<'a> Cursor<'a> {
    fn new(op: &'a Op) -> Self {
        Self {
            components: &op.components,
            index: 0,
            offset: 0,
        }
    }

    fn peek(&self) -> Option<&'a Component> {
        self.components.get(self.index)
    }

    /// The current component's length in UTF-16 units.
    fn total(&self) -> u32 {
        match self.peek() {
            Some(Component::Retain(n) | Component::Delete(n)) => *n,
            Some(Component::Insert(text)) => utf16_len(text),
            None => 0,
        }
    }

    /// The UTF-16 units left in the current component.
    fn remaining(&self) -> u32 {
        self.total() - self.offset
    }

    /// The next `n` units of the current insert.
    fn inserted(&self, n: u32) -> String {
        let Some(Component::Insert(text)) = self.peek() else {
            return String::new();
        };
        let units: Vec<u16> = text.encode_utf16().collect();
        let start = self.offset as usize;
        String::from_utf16_lossy(&units[start..start + n as usize])
    }

    fn advance(&mut self, n: u32) {
        self.offset += n;
        if self.offset >= self.total() {
            self.index += 1;
            self.offset = 0;
        }
    }
}

/// Rewrites `server` and `client`, two changes to the same text, so each
/// applies after the other: `server` then `client'` gives the text `client`
/// then `server'` gives. Where both insert at one place, the server's text
/// goes first.
pub fn transform(server: &Op, client: &Op) -> Result<(Op, Op), Mismatch> {
    if server.base_len != client.base_len {
        return Err(Mismatch);
    }
    let mut server_out = Op::default();
    let mut client_out = Op::default();
    let mut a = Cursor::new(server);
    let mut b = Cursor::new(client);
    loop {
        match (a.peek(), b.peek()) {
            (None, None) => break,
            (Some(Component::Insert(_)), _) => {
                let n = a.remaining();
                server_out.insert(&a.inserted(n));
                client_out.retain(n);
                a.advance(n);
            }
            (_, Some(Component::Insert(_))) => {
                let n = b.remaining();
                server_out.retain(n);
                client_out.insert(&b.inserted(n));
                b.advance(n);
            }
            (None, _) | (_, None) => return Err(Mismatch),
            (Some(left), Some(right)) => {
                let n = a.remaining().min(b.remaining());
                match (left, right) {
                    (Component::Retain(_), Component::Retain(_)) => {
                        server_out.retain(n);
                        client_out.retain(n);
                    }
                    // Both deleted the same text: neither needs to again.
                    (Component::Delete(_), Component::Delete(_)) => {}
                    (Component::Delete(_), _) => server_out.delete(n),
                    (_, Component::Delete(_)) => client_out.delete(n),
                    _ => return Err(Mismatch),
                }
                a.advance(n);
                b.advance(n);
            }
        }
    }
    Ok((server_out, client_out))
}

/// The change `first` then `second` make together.
pub fn compose(first: &Op, second: &Op) -> Result<Op, Mismatch> {
    if first.target_len != second.base_len {
        return Err(Mismatch);
    }
    let mut out = Op::default();
    let mut a = Cursor::new(first);
    let mut b = Cursor::new(second);
    loop {
        match (a.peek(), b.peek()) {
            (None, None) => break,
            // What `first` deleted, `second` never saw.
            (Some(Component::Delete(_)), _) => {
                let n = a.remaining();
                out.delete(n);
                a.advance(n);
            }
            // What `second` inserts, `first` never saw.
            (_, Some(Component::Insert(_))) => {
                let n = b.remaining();
                out.insert(&b.inserted(n));
                b.advance(n);
            }
            (None, _) | (_, None) => return Err(Mismatch),
            (Some(left), Some(right)) => {
                let n = a.remaining().min(b.remaining());
                match (left, right) {
                    (Component::Retain(_), Component::Retain(_)) => out.retain(n),
                    (Component::Retain(_), _) => out.delete(n),
                    // `second` keeps the text `first` inserted, or deletes
                    // it before anyone saw it.
                    (_, Component::Retain(_)) => out.insert(&a.inserted(n)),
                    _ => {}
                }
                a.advance(n);
                b.advance(n);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Changes to `text` made of splices at code-point boundaries, as the
    /// editor makes them.
    fn op_for(text: String) -> impl Strategy<Value = Op> {
        let edit = (any::<prop::sample::Index>(), 0usize..3, "[a-c😀é]{0,3}");
        proptest::collection::vec(edit, 0..4).prop_map(move |edits| {
            let mut current: Vec<char> = text.chars().collect();
            let mut splices = Vec::new();
            for (at, removed, inserted) in edits {
                let start = at.index(current.len() + 1);
                let end = (start + removed).min(current.len());
                let units = |chars: &[char]| chars.iter().map(|c| c.len_utf16() as u32).sum();
                splices.push(Splice {
                    start: units(&current[..start]),
                    removed: units(&current[start..end]),
                    inserted: inserted.clone(),
                });
                current.splice(start..end, inserted.chars());
            }
            Op::from_splices(utf16_len(&text), &splices).expect("splices fit")
        })
    }

    const TEXT: &str = "a😀bcé😀";

    proptest! {
        #[test]
        fn both_orders_reach_one_text((server, client) in (op_for(TEXT.into()), op_for(TEXT.into()))) {
            let (server_after, client_after) = transform(&server, &client).unwrap();
            let one = client_after.apply(&server.apply(TEXT).unwrap()).unwrap();
            let other = server_after.apply(&client.apply(TEXT).unwrap()).unwrap();
            prop_assert_eq!(one, other);
        }

        #[test]
        fn compose_applies_both((first, second) in op_for(TEXT.into()).prop_flat_map(|first| {
            let after = first.apply(TEXT).unwrap();
            (Just(first), op_for(after))
        })) {
            let both = compose(&first, &second).unwrap();
            prop_assert_eq!(
                both.apply(TEXT).unwrap(),
                second.apply(&first.apply(TEXT).unwrap()).unwrap()
            );
        }

        #[test]
        fn splices_round_trip(op in op_for(TEXT.into())) {
            let again = Op::from_splices(op.base_len(), &op.splices()).unwrap();
            prop_assert_eq!(again.apply(TEXT).unwrap(), op.apply(TEXT).unwrap());
        }
    }

    #[test]
    fn the_server_insert_goes_first() {
        let server = Op::from_splices(
            2,
            &[Splice {
                start: 1,
                removed: 0,
                inserted: "S".into(),
            }],
        )
        .unwrap();
        let client = Op::from_splices(
            2,
            &[Splice {
                start: 1,
                removed: 0,
                inserted: "C".into(),
            }],
        )
        .unwrap();
        let (server_after, client_after) = transform(&server, &client).unwrap();
        assert_eq!(client_after.apply("aSb").unwrap(), "aSCb");
        assert_eq!(server_after.apply("aCb").unwrap(), "aSCb");
    }

    #[test]
    fn the_guest_encoding_round_trips() {
        let op = Op::from_splices(
            5,
            &[Splice {
                start: 2,
                removed: 1,
                inserted: "é😀".into(),
            }],
        )
        .unwrap();
        assert_eq!(Op::decode(&op.encode(), 5).unwrap(), op);
    }
}
