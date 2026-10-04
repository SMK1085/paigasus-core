// SPDX-License-Identifier: Apache-2.0

//! The in-memory DPoP replay store (SMA-700 § 4.5, D3). `AppState` holds ONE instance behind one
//! `Arc`, shared by `Introspect` and the `IsAuthorized` follow-up. It is correct only with one IAM
//! replica; the chart pins IAM to one (R2). A restart clears it (R1).
//!
//! Every call first removes the expired heads of the removal index, so no call scans the whole
//! map. An entry is removed at the first call with `now > max(expires_at, follow_up_deadline)`.
//! Time comes only from the `now` argument (the `Clock` port). One `Mutex` protects the state;
//! it is never held across an `.await` (the port is synchronous).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};

use paigasus_iam_core::{NewProof, ProofKey, RecordOutcome, RedeemOutcome, ReplayStore};

/// One recorded proof. The `subject` and `jkt` hashes are kept to release the counts.
struct Entry {
    subject: [u8; 16],
    jkt: [u8; 16],
    remove_at: i64,
    follow_up_deadline: i64,
    follow_up_digest: [u8; 32],
    redeemed: bool,
}

/// The live removal seconds of one `jkt` hash or one subject hash, as a min-heap. The count is
/// the length. The minimum is the oldest removal, which a quota refusal reports. `purge` removes
/// entries in ascending removal order over the whole store, so a released entry is always the
/// minimum of its owner.
#[derive(Default)]
struct Owner {
    removals: BinaryHeap<Reverse<i64>>,
}

impl Owner {
    fn add(&mut self, remove_at: i64) {
        self.removals.push(Reverse(remove_at));
    }

    /// Removes the oldest entry. `true` when the owner has no entry left.
    fn release(&mut self, remove_at: i64) -> bool {
        let oldest = self.removals.pop().map(|Reverse(t)| t);
        debug_assert_eq!(oldest, Some(remove_at));
        self.removals.is_empty()
    }
}

#[derive(Default)]
struct State {
    entries: HashMap<ProofKey, Entry>,
    /// The keys by removal second (the expiry index).
    by_removal: BTreeMap<i64, Vec<ProofKey>>,
    keys: HashMap<[u8; 16], Owner>,
    subjects: HashMap<[u8; 16], Owner>,
}

impl State {
    /// Removes every entry whose removal second is before `now`. Reads only the expired heads.
    fn purge(&mut self, now: i64) {
        while let Some(head) = self.by_removal.first_entry() {
            if *head.key() >= now {
                break;
            }
            for key in head.remove() {
                if let Some(entry) = self.entries.remove(&key) {
                    release(&mut self.keys, entry.jkt, entry.remove_at);
                    release(&mut self.subjects, entry.subject, entry.remove_at);
                }
            }
        }
    }
}

fn release(owners: &mut HashMap<[u8; 16], Owner>, id: [u8; 16], remove_at: i64) {
    if let Some(owner) = owners.get_mut(&id)
        && owner.release(remove_at)
    {
        owners.remove(&id);
    }
}

/// `Some(retry_after_secs)` when `id` already has `quota` live entries. The oldest of them goes at
/// the first call with `now > remove_at`, so the wait is `remove_at + 1 - now`, at least 1.
fn quota_hit(owners: &HashMap<[u8; 16], Owner>, id: [u8; 16], quota: usize, now: i64) -> Option<u32> {
    let owner = owners.get(&id)?;
    if owner.removals.len() < quota {
        return None;
    }
    let oldest = owner.removals.peek().map_or(now, |Reverse(t)| *t);
    let wait = oldest.saturating_add(1).saturating_sub(now).max(1);
    Some(u32::try_from(wait).unwrap_or(u32::MAX))
}

/// The `ReplayStore` v1 implementation (SMA-700 § 4.5).
pub struct InMemoryReplayStore {
    state: Mutex<State>,
    capacity: usize,
    per_key_quota: usize,
    per_subject_quota: usize,
}

impl InMemoryReplayStore {
    /// `IamConfig::validate` has refused a zero and a quota above the capacity.
    #[must_use]
    pub fn new(capacity: usize, per_key_quota: usize, per_subject_quota: usize) -> Self {
        InMemoryReplayStore {
            state: Mutex::new(State::default()),
            capacity,
            per_key_quota,
            per_subject_quota,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ReplayStore for InMemoryReplayStore {
    fn record(&self, entry: NewProof, now: i64) -> RecordOutcome {
        let mut state = self.lock();
        state.purge(now);
        if state.entries.contains_key(&entry.key) {
            return RecordOutcome::Replayed;
        }
        // Decision P4: a quota before the global capacity, the key quota before the subject quota.
        if let Some(retry_after_secs) = quota_hit(&state.keys, entry.jkt, self.per_key_quota, now) {
            return RecordOutcome::QuotaExceeded { retry_after_secs };
        }
        if let Some(retry_after_secs) = quota_hit(&state.subjects, entry.subject, self.per_subject_quota, now) {
            return RecordOutcome::QuotaExceeded { retry_after_secs };
        }
        if state.entries.len() >= self.capacity {
            return RecordOutcome::CapacityFull { entries: state.entries.len() };
        }
        let remove_at = entry.expires_at.max(entry.follow_up_deadline);
        state.keys.entry(entry.jkt).or_default().add(remove_at);
        state.subjects.entry(entry.subject).or_default().add(remove_at);
        state.by_removal.entry(remove_at).or_default().push(entry.key);
        state.entries.insert(
            entry.key,
            Entry {
                subject: entry.subject,
                jkt: entry.jkt,
                remove_at,
                follow_up_deadline: entry.follow_up_deadline,
                follow_up_digest: entry.follow_up_digest,
                redeemed: false,
            },
        );
        RecordOutcome::Fresh
    }

    fn redeem_follow_up(&self, key: ProofKey, proof_digest: [u8; 32], now: i64) -> RedeemOutcome {
        let mut state = self.lock();
        state.purge(now);
        match state.entries.get_mut(&key) {
            Some(entry) if !entry.redeemed && now <= entry.follow_up_deadline && entry.follow_up_digest == proof_digest => {
                entry.redeemed = true;
                RedeemOutcome::Redeemed
            }
            _ => RedeemOutcome::Refused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A proof with one-byte ids, so each test names its keys, `jkt`s and subjects plainly.
    fn proof(key: u8, jkt: u8, subject: u8, expires_at: i64, follow_up_deadline: i64) -> NewProof {
        NewProof {
            key: ProofKey([key; 16]),
            subject: [subject; 16],
            jkt: [jkt; 16],
            expires_at,
            follow_up_deadline,
            follow_up_digest: [key; 32],
        }
    }

    fn roomy() -> InMemoryReplayStore {
        InMemoryReplayStore::new(100, 100, 100)
    }

    #[test]
    fn a_new_proof_is_fresh_and_the_same_key_again_is_a_replay() {
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 101), RecordOutcome::Replayed);
        assert_eq!(store.record(proof(2, 1, 1, 160, 160), 101), RecordOutcome::Fresh, "another key is not a replay");
    }

    #[test]
    fn an_entry_is_live_at_its_deadline_and_gone_after_the_later_deadline() {
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 160), RecordOutcome::Replayed, "live at now == expires_at");
        assert_eq!(store.record(proof(1, 1, 1, 220, 220), 161), RecordOutcome::Fresh, "removed after the later deadline");
        // The later of the two deadlines decides: here the ticket deadline (190) outlives expires_at (160).
        let store = roomy();
        assert_eq!(store.record(proof(2, 1, 1, 160, 190), 160), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 1, 1, 160, 190), 190), RecordOutcome::Replayed, "kept until the ticket deadline");
        assert_eq!(store.record(proof(2, 1, 1, 260, 260), 191), RecordOutcome::Fresh);
    }

    #[test]
    fn the_follow_up_has_30_s_when_recorded_at_expires_at() {
        // § 4.5: follow_up_deadline = max(expires_at, now + 30). Recorded at now == expires_at == 160.
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 160, 190), 160), RecordOutcome::Fresh);
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [1; 32], 190), RedeemOutcome::Redeemed);
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 160, 190), 160), RecordOutcome::Fresh);
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [1; 32], 191), RedeemOutcome::Refused, "after the deadline");
    }

    #[test]
    fn the_30_s_floor_keeps_the_entry_until_the_later_deadline() {
        // PF-C3: recorded at now == expires_at, so follow_up_deadline = now + 30.
        let now = 160;
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, now, now + 30), now), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(1, 1, 1, now, now + 30), now + 1), RecordOutcome::Replayed, "the entry is kept past expires_at");
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [1; 32], now + 30), RedeemOutcome::Redeemed, "redeem at the deadline");
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, now, now + 30), now), RecordOutcome::Fresh);
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [1; 32], now + 31), RedeemOutcome::Refused, "refused after the deadline");
    }

    #[test]
    fn the_key_quota_reports_the_time_until_its_oldest_entry_goes() {
        let store = InMemoryReplayStore::new(100, 2, 100);
        assert_eq!(store.record(proof(1, 7, 1, 150, 150), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 7, 2, 140, 140), 100), RecordOutcome::Fresh);
        // The oldest entry of jkt 7 goes at the first call with now > 140, so 141 - 100 = 41.
        assert_eq!(store.record(proof(3, 7, 3, 160, 160), 100), RecordOutcome::QuotaExceeded { retry_after_secs: 41 });
        assert_eq!(store.record(proof(3, 8, 3, 160, 160), 100), RecordOutcome::Fresh, "another jkt is not limited");
    }

    #[test]
    fn the_subject_quota_reports_the_time_until_its_oldest_entry_goes() {
        let store = InMemoryReplayStore::new(100, 100, 2);
        assert_eq!(store.record(proof(1, 1, 9, 130, 130), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 2, 9, 170, 170), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(3, 3, 9, 160, 160), 100), RecordOutcome::QuotaExceeded { retry_after_secs: 31 });
    }

    #[test]
    fn a_full_capacity_is_reported_with_the_entry_count() {
        let store = InMemoryReplayStore::new(2, 100, 100);
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 2, 2, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(3, 3, 3, 160, 160), 100), RecordOutcome::CapacityFull { entries: 2 });
    }

    #[test]
    fn a_quota_wins_over_a_full_capacity() {
        // Decision P4: the client over its own quota gets a 429, not an outage answer.
        let store = InMemoryReplayStore::new(1, 1, 100);
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 1, 2, 160, 160), 100), RecordOutcome::QuotaExceeded { retry_after_secs: 61 });
    }

    #[test]
    fn expired_heads_are_removed_before_a_limit_is_reported() {
        let store = InMemoryReplayStore::new(1, 1, 1);
        assert_eq!(store.record(proof(1, 1, 1, 110, 110), 100), RecordOutcome::Fresh);
        assert_eq!(
            store.record(proof(2, 1, 1, 170, 170), 111),
            RecordOutcome::Fresh,
            "the entry that went at 111 frees the key, the subject and the capacity"
        );
    }

    #[test]
    fn counts_go_to_zero_and_are_removed() {
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 110, 110), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 1, 2, 120, 120), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(3, 3, 3, 500, 500), 121), RecordOutcome::Fresh, "purges keys 1 and 2");
        let state = store.lock();
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.by_removal.len(), 1, "no empty index bucket is kept");
        assert!(!state.keys.contains_key(&[1; 16]), "a jkt count at zero is removed");
        assert!(!state.subjects.contains_key(&[1; 16]) && !state.subjects.contains_key(&[2; 16]), "subject counts at zero are removed");
        assert_eq!(state.keys.len(), 1);
        assert_eq!(state.subjects.len(), 1);
    }

    #[test]
    fn a_follow_up_is_redeemed_once_and_only_with_the_same_digest() {
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [9; 32], 101), RedeemOutcome::Refused, "another digest");
        assert_eq!(
            store.redeem_follow_up(ProofKey([1; 16]), [1; 32], 101),
            RedeemOutcome::Redeemed,
            "a digest mismatch did not use the ticket"
        );
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [1; 32], 102), RedeemOutcome::Refused, "a ticket is used once");
        assert_eq!(store.redeem_follow_up(ProofKey([2; 16]), [2; 32], 102), RedeemOutcome::Refused, "no ticket for an unknown key");
        // A redeemed entry still blocks a replay of the proof.
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 103), RecordOutcome::Replayed);
    }
}
