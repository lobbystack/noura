//! The key upkeep that waits its turn (docs/workspace-format/sync-v2.md
//! §8, §9). A device records a member account's later head after a random
//! 0 to 60 seconds for its own account and 60 to 120 for another account's,
//! and a key holder rotates a domain whose rotation is pending, or answers
//! a re-wrap request, after a random 0 to 60 seconds. The device the work
//! concerns most therefore goes first, and each other device finds the
//! work done when its turn comes and skips it.
//!
//! [`SyncService::upkeep_waiting`] lists what waits in a workspace,
//! [`Upkeep::plan`] draws each task's turn and says which are due, and
//! [`SyncService::run_upkeep`] carries one out.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use sync_protocol::messages::RewrapRequest;
use sync_protocol::types::Id;

use super::heads::random_wait;
use super::{RemoteWorkspace, SyncError, SyncService};

/// How long a key holder waits before it rotates a pending domain or
/// answers a re-wrap request (§9).
pub const KEY_HOLDER_WAIT: Range<Duration> = Duration::ZERO..Duration::from_secs(60);

/// A piece of upkeep that waits its turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Task {
    /// Record a member account's later head in a workspace (§8).
    Head {
        /// The workspace.
        workspace: Id,
        /// The account whose head waits.
        account: Id,
    },
    /// Rotate the workspace domain, whose rotation is pending (§9).
    Rotate {
        /// The workspace.
        workspace: Id,
    },
    /// Answer another device's re-wrap request (§9).
    Rewrap {
        /// The workspace.
        workspace: Id,
        /// The requesting device.
        device: Id,
        /// The epoch it asks for.
        epoch: u64,
    },
}

impl Task {
    /// The workspace the task concerns.
    pub fn workspace(&self) -> Id {
        match self {
            Task::Head { workspace, .. }
            | Task::Rotate { workspace }
            | Task::Rewrap { workspace, .. } => *workspace,
        }
    }
}

/// When each waiting task falls due.
#[derive(Debug, Default)]
pub struct Upkeep {
    due: BTreeMap<Task, Instant>,
    answered: BTreeSet<Task>,
}

impl Upkeep {
    /// Takes the tasks that wait now, each with its wait, and returns those
    /// due at `now`. A task seen for the first time draws its turn within
    /// its wait, and a task no longer listed is forgotten. A due task waits
    /// again if it's still listed next time, as a refused head push does,
    /// except a re-wrap answer: a device answers each request once.
    pub fn plan(&mut self, waiting: Vec<(Task, Range<Duration>)>, now: Instant) -> Vec<Task> {
        let listed: BTreeSet<Task> = waiting.iter().map(|(task, _)| *task).collect();
        self.answered.retain(|task| listed.contains(task));
        let mut due = BTreeMap::new();
        let mut ready = Vec::new();
        for (task, wait) in waiting {
            if self.answered.contains(&task) {
                continue;
            }
            let at = self
                .due
                .get(&task)
                .copied()
                .unwrap_or_else(|| now + random_wait(&wait));
            if at <= now {
                if matches!(task, Task::Rewrap { .. }) {
                    self.answered.insert(task);
                }
                ready.push(task);
            } else {
                due.insert(task, at);
            }
        }
        self.due = due;
        ready
    }
}

impl SyncService {
    /// What waits in `remote` after the device takes its policy and keys
    /// again: each head pending there (§8), the workspace domain's rotation
    /// while it's pending and the device reads the domain, and each re-wrap
    /// request the device may answer (§9).
    pub async fn upkeep_waiting(
        &mut self,
        remote: &mut RemoteWorkspace,
    ) -> Result<Vec<(Task, Range<Duration>)>, SyncError> {
        self.refresh_policy(remote).await?;
        self.refresh_keys(remote).await?;
        let workspace = remote.workspace;
        let mut waiting = Vec::new();
        for head in self.pending_heads(remote).await? {
            waiting.push((
                Task::Head {
                    workspace,
                    account: head.account,
                },
                head.wait,
            ));
        }
        let reads = remote
            .policy
            .head_snapshot()
            .can_read(&self.credentials.keys.device(), &remote.domain);
        if reads && self.rotation_pending(remote) {
            waiting.push((Task::Rotate { workspace }, KEY_HOLDER_WAIT));
        }
        for request in self.open_rewraps(remote).await? {
            waiting.push((
                Task::Rewrap {
                    workspace,
                    device: request.device,
                    epoch: request.epoch,
                },
                KEY_HOLDER_WAIT,
            ));
        }
        Ok(waiting)
    }

    /// Carries out `task` in `remote` once its turn came. Each step checks
    /// first whether another device did the work meanwhile: a head that
    /// another change recorded, or a rotation that no longer waits.
    pub async fn run_upkeep(
        &mut self,
        remote: &mut RemoteWorkspace,
        task: &Task,
    ) -> Result<(), SyncError> {
        match task {
            Task::Head { account, .. } => {
                let pending = self
                    .pending_heads(remote)
                    .await?
                    .into_iter()
                    .find(|head| head.account == *account);
                if let Some(pending) = pending {
                    self.push_pending_head(remote, &pending).await?;
                }
            }
            Task::Rotate { .. } => {
                self.rotate_if_pending(remote).await?;
            }
            Task::Rewrap { device, epoch, .. } => {
                let request = RewrapRequest {
                    device: *device,
                    epoch: *epoch,
                };
                self.answer_rewrap(remote, &request).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head() -> Task {
        Task::Head {
            workspace: Id([1; 16]),
            account: Id([2; 16]),
        }
    }

    fn rewrap() -> Task {
        Task::Rewrap {
            workspace: Id([1; 16]),
            device: Id([3; 16]),
            epoch: 2,
        }
    }

    const ANOTHER_ACCOUNT: Range<Duration> = Duration::from_secs(60)..Duration::from_secs(120);

    #[test]
    fn a_task_falls_due_within_its_wait() {
        let mut upkeep = Upkeep::default();
        let start = Instant::now();
        let waiting = vec![(head(), ANOTHER_ACCOUNT)];
        assert!(upkeep.plan(waiting.clone(), start).is_empty());
        assert!(
            upkeep
                .plan(waiting.clone(), start + Duration::from_secs(59))
                .is_empty(),
            "another account's head waits at least 60 seconds"
        );
        assert_eq!(
            upkeep.plan(waiting, start + Duration::from_secs(120)),
            vec![head()]
        );
    }

    #[test]
    fn a_task_no_longer_listed_is_forgotten_and_draws_again() {
        let mut upkeep = Upkeep::default();
        let start = Instant::now();
        assert!(
            upkeep
                .plan(vec![(head(), ANOTHER_ACCOUNT)], start)
                .is_empty()
        );
        assert!(
            upkeep
                .plan(Vec::new(), start + Duration::from_secs(30))
                .is_empty()
        );
        let later = start + Duration::from_secs(150);
        assert!(
            upkeep
                .plan(vec![(head(), ANOTHER_ACCOUNT)], later)
                .is_empty(),
            "the head came back, so it waits its turn again"
        );
    }

    #[test]
    fn a_head_waits_again_but_a_rewrap_is_answered_once() {
        let mut upkeep = Upkeep::default();
        let now = Instant::now();
        let at_once = Duration::ZERO..Duration::ZERO;
        let waiting = vec![(head(), at_once.clone()), (rewrap(), at_once)];
        assert_eq!(upkeep.plan(waiting.clone(), now), vec![head(), rewrap()]);
        assert_eq!(
            upkeep.plan(waiting, now),
            vec![head()],
            "the device answered that request"
        );
        assert!(upkeep.plan(Vec::new(), now).is_empty());
        assert_eq!(
            upkeep.plan(vec![(rewrap(), Duration::ZERO..Duration::ZERO)], now),
            vec![rewrap()],
            "a request that closed and came back gets an answer again"
        );
    }
}
