//! The policy change record (§8).

use serde::{Deserialize, Serialize};

use super::{AccountHead, ByAccount, unknown_value};
use crate::codec::{Decode, DecodeError, Encode, Reader, SortSpec, Sorted, Writer};
use crate::tag::Tag;
use crate::types::{Digest, Id};

/// The most entries an `access` list holds (§27).
pub const MAX_ACCESS: usize = 1_024;

crate::byte_enum! {
    /// A member's role (§8).
    pub enum Role {
        /// 1: the one owner.
        Owner = 1,
        /// 2: an admin.
        Admin = 2,
        /// 3: an editor.
        Editor = 3,
        /// 4: a viewer.
        Viewer = 4,
    }
}

crate::byte_enum! {
    /// A restricted domain grant's level; each includes the ones below it.
    pub enum Level {
        /// 1: read.
        Read = 1,
        /// 2: write, which includes read.
        Write = 2,
        /// 3: manage, which includes write and read.
        Manage = 3,
    }
}

crate::wire_struct! {
    /// `access`: one account's grant in a restricted domain.
    pub struct Access {
        /// The grantee account.
        pub account: Id,
        /// Its level.
        pub level: Level,
    }
}

impl SortSpec<Access> for ByAccount {
    fn key(item: &Access) -> Vec<u8> {
        item.account.0.to_vec()
    }
}

/// An access list, sorted by account, so it names each account once.
pub type AccessList = Sorted<Access, ByAccount, MAX_ACCESS>;

/// A restricted domain's marker file content (§9): one line,
/// `domain: <base64url domain ID>`, followed by a line feed.
pub fn marker_content(domain: &Id) -> String {
    format!("domain: {}\n", domain.to_base64url())
}

/// The domain a marker file names, when its bytes are exactly what
/// [`marker_content`] writes; anything else is no marker content.
pub fn parse_marker(bytes: &[u8]) -> Option<Id> {
    let text = std::str::from_utf8(bytes).ok()?;
    let encoded = text.strip_prefix("domain: ")?.strip_suffix('\n')?;
    let domain = Id::from_base64url(encoded).ok()?;
    (!domain.is_zero()).then_some(domain)
}

/// Sorts anchors and kit workspaces by workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ByWorkspace;

/// The kind-specific body of a policy change.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicyKind {
    /// 1: the workspace's first change; its account becomes the owner.
    Genesis {
        /// The workspace domain's ID.
        workspace_domain: Id,
    },
    /// 2: records newer account chain heads.
    AccountHeads,
    /// 3: adds a member through an accepted invitation.
    AddMember {
        /// The new member.
        account: Id,
        /// Its role.
        role: Role,
        /// The invitation it accepted.
        invitation: Id,
    },
    /// 4: changes a member's role.
    SetRole {
        /// The member.
        account: Id,
        /// The new role.
        role: Role,
    },
    /// 5: removes a member.
    RemoveMember {
        /// The member.
        account: Id,
    },
    /// 6: creates a restricted domain.
    CreateDomain {
        /// The new domain.
        domain: Id,
        /// The `rid` of its marker.
        marker_rid: Id,
        /// Its access list.
        access: AccessList,
    },
    /// 7: replaces a restricted domain's access list.
    SetAccess {
        /// The domain.
        domain: Id,
        /// The new list.
        access: AccessList,
    },
    /// 8: deletes a restricted domain.
    DeleteDomain {
        /// The domain.
        domain: Id,
    },
    /// 9: makes another member the owner; the old owner becomes an admin.
    TransferOwner {
        /// The new owner.
        account: Id,
    },
    /// 10: accepts a member account's new chain after a reset.
    AcceptReset {
        /// The reset account.
        account: Id,
    },
}

impl PolicyKind {
    /// The kind byte.
    pub fn code(&self) -> u8 {
        match self {
            PolicyKind::Genesis { .. } => 1,
            PolicyKind::AccountHeads => 2,
            PolicyKind::AddMember { .. } => 3,
            PolicyKind::SetRole { .. } => 4,
            PolicyKind::RemoveMember { .. } => 5,
            PolicyKind::CreateDomain { .. } => 6,
            PolicyKind::SetAccess { .. } => 7,
            PolicyKind::DeleteDomain { .. } => 8,
            PolicyKind::TransferOwner { .. } => 9,
            PolicyKind::AcceptReset { .. } => 10,
        }
    }

    /// The snake-case name of the kind.
    pub fn name(&self) -> &'static str {
        match self {
            PolicyKind::Genesis { .. } => "genesis",
            PolicyKind::AccountHeads => "account_heads",
            PolicyKind::AddMember { .. } => "add_member",
            PolicyKind::SetRole { .. } => "set_role",
            PolicyKind::RemoveMember { .. } => "remove_member",
            PolicyKind::CreateDomain { .. } => "create_domain",
            PolicyKind::SetAccess { .. } => "set_access",
            PolicyKind::DeleteDomain { .. } => "delete_domain",
            PolicyKind::TransferOwner { .. } => "transfer_owner",
            PolicyKind::AcceptReset { .. } => "accept_reset",
        }
    }
}

/// The signed body of a policy change (§8).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PolicyChange {
    /// The workspace.
    pub workspace: Id,
    /// 1 for genesis, then +1 per change.
    pub revision: u64,
    /// The digest of revision − 1; 32 zero bytes at revision 1.
    pub previous: Digest,
    /// The signing device.
    pub signer: Id,
    /// The account chain heads the change relies on, sorted by account.
    pub heads: Sorted<AccountHead, ByAccount>,
    /// The kind and its body.
    #[serde(flatten)]
    pub kind: PolicyKind,
}

impl Encode for PolicyChange {
    fn encode(&self, w: &mut Writer) {
        w.tag(Tag::Policy);
        w.put(&self.workspace);
        w.put(&self.revision);
        w.put(&self.previous);
        w.put(&self.signer);
        w.put(&self.heads);
        w.u8(self.kind.code());
        match &self.kind {
            PolicyKind::Genesis { workspace_domain } => w.put(workspace_domain),
            PolicyKind::AccountHeads => {}
            PolicyKind::AddMember {
                account,
                role,
                invitation,
            } => {
                w.put(account);
                w.put(role);
                w.put(invitation);
            }
            PolicyKind::SetRole { account, role } => {
                w.put(account);
                w.put(role);
            }
            PolicyKind::RemoveMember { account }
            | PolicyKind::TransferOwner { account }
            | PolicyKind::AcceptReset { account } => w.put(account),
            PolicyKind::CreateDomain {
                domain,
                marker_rid,
                access,
            } => {
                w.put(domain);
                w.put(marker_rid);
                w.put(access);
            }
            PolicyKind::SetAccess { domain, access } => {
                w.put(domain);
                w.put(access);
            }
            PolicyKind::DeleteDomain { domain } => w.put(domain),
        }
    }
}

impl Decode for PolicyChange {
    const MIN_SIZE: usize = Tag::Policy.encoded_len() + 16 + 8 + 32 + 16 + 4 + 1;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.tag(Tag::Policy)?;
        let workspace = r.get()?;
        let revision = r.get()?;
        let previous = r.get()?;
        let signer = r.get()?;
        let heads = r.get()?;
        let kind_offset = r.offset();
        let kind = match r.u8()? {
            1 => PolicyKind::Genesis {
                workspace_domain: r.get()?,
            },
            2 => PolicyKind::AccountHeads,
            3 => PolicyKind::AddMember {
                account: r.get()?,
                role: r.get()?,
                invitation: r.get()?,
            },
            4 => PolicyKind::SetRole {
                account: r.get()?,
                role: r.get()?,
            },
            5 => PolicyKind::RemoveMember { account: r.get()? },
            6 => PolicyKind::CreateDomain {
                domain: r.get()?,
                marker_rid: r.get()?,
                access: r.get()?,
            },
            7 => PolicyKind::SetAccess {
                domain: r.get()?,
                access: r.get()?,
            },
            8 => PolicyKind::DeleteDomain { domain: r.get()? },
            9 => PolicyKind::TransferOwner { account: r.get()? },
            10 => PolicyKind::AcceptReset { account: r.get()? },
            _ => return Err(unknown_value(r, kind_offset)),
        };
        Ok(Self {
            workspace,
            revision,
            previous,
            signer,
            heads,
            kind,
        })
    }
}
