//! A text change as the host and the guest pass it: components that retain,
//! insert or delete UTF-16 units from the start of the text. Text past the
//! last component stays.
//!
//! Encoding: each component is a tag byte, then for a retain (0) or a delete
//! (2) a little-endian `u32` count, and for an insert (1) a little-endian
//! `u32` byte length and that many bytes of UTF-8.

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

/// A text change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Op(Vec<Component>);

impl Op {
    /// The components, in order.
    pub fn components(&self) -> &[Component] {
        &self.0
    }

    /// Appends a component.
    pub fn push(&mut self, component: Component) {
        self.0.push(component);
    }

    /// How many UTF-16 units of the text before the change the components
    /// reach.
    pub fn base_len(&self) -> u64 {
        self.0
            .iter()
            .map(|component| match component {
                Component::Retain(n) | Component::Delete(n) => u64::from(*n),
                Component::Insert(_) => 0,
            })
            .sum()
    }

    /// The bytes of text the change inserts.
    pub fn byte_len(&self) -> usize {
        self.0
            .iter()
            .map(|component| match component {
                Component::Insert(text) => text.len(),
                _ => 0,
            })
            .sum()
    }

    /// How many UTF-16 units the change deletes.
    pub fn deleted_len(&self) -> u64 {
        self.0
            .iter()
            .map(|component| match component {
                Component::Delete(n) => u64::from(*n),
                _ => 0,
            })
            .sum()
    }

    /// The encoding the host reads.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for component in &self.0 {
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

    /// Reads the encoding the host writes.
    pub fn decode(mut bytes: &[u8]) -> Option<Self> {
        let mut op = Op::default();
        while let Some((&tag, rest)) = bytes.split_first() {
            let (word, rest) = rest.split_first_chunk::<4>()?;
            let n = u32::from_le_bytes(*word);
            bytes = rest;
            op.push(match tag {
                0 => Component::Retain(n),
                1 => {
                    let (text, rest) = bytes.split_at_checked(n as usize)?;
                    bytes = rest;
                    Component::Insert(std::str::from_utf8(text).ok()?.to_owned())
                }
                2 => Component::Delete(n),
                _ => return None,
            });
        }
        Some(op)
    }
}
