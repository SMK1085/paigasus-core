// SPDX-License-Identifier: Apache-2.0

//! The decision recorder (SMA-755 § 5.3, § 5.8, P-8). One recorder serves every report site. It
//! owns the instance id, the per-org sequences, the clock, the entropy and the digest key.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, KeyInit, Mac};
use paigasus_gateway_core::decision_log::{ArgumentDigest, DecisionRecord, RecordHeader, ReportedCall, build_refusal, build_request, build_response, response_org};
use paigasus_gateway_core::{DecisionContext, RequestEntry, TenantMode};
use paigasus_kernel::mint_uuid7;
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use uuid::Uuid;

use crate::domain::decision_log::{DecisionLog, Entropy, OsEntropy, TracingDecisionLog};
use crate::domain::limits::{Clock, SystemClock};
use crate::domain::org_of;

/// The least key length in bytes (the SHA-256 output size).
const MIN_KEY_BYTES: usize = 32;

/// The key of the keyed argument digest, with its public id.
pub struct DigestKey {
    id: String,
    key: SecretString,
}

impl std::fmt::Debug for DigestKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DigestKey").field("id", &self.id).finish_non_exhaustive()
    }
}

impl DigestKey {
    /// # Errors
    /// The key is shorter than 32 bytes.
    pub fn new(id: String, key: SecretString) -> Result<Self, String> {
        if key.expose_secret().len() < MIN_KEY_BYTES {
            return Err(format!("the digest key must hold at least {MIN_KEY_BYTES} bytes"));
        }
        Ok(Self { id, key })
    }

    /// HMAC-SHA256 of the arguments. Never a plain hash (A1).
    #[must_use]
    pub fn digest(&self, arguments: &[u8]) -> ArgumentDigest {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(self.key.expose_secret().as_bytes()).expect("HMAC accepts a key of any length");
        mac.update(arguments);
        ArgumentDigest::new(mac.finalize().into_bytes().into(), self.id.clone())
    }
}

/// Builds the record of each report entry and hands it to the log.
pub struct DecisionRecorder {
    log: Arc<dyn DecisionLog>,
    clock: Arc<dyn Clock>,
    entropy: Arc<dyn Entropy>,
    digest: Option<DigestKey>,
    instance_id: Uuid,
    sequences: Mutex<HashMap<Option<Uuid>, u64>>,
}

impl DecisionRecorder {
    #[must_use]
    pub fn new(log: Arc<dyn DecisionLog>, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>, digest: Option<DigestKey>) -> Self {
        let instance_id = mint_uuid7(unix_ms(clock.now()), entropy.bytes10());
        Self {
            log,
            clock,
            entropy,
            digest,
            instance_id,
            sequences: Mutex::new(HashMap::new()),
        }
    }

    /// The recorder of the default configuration: the tracing line only, no digest, the system
    /// clock and the operating system entropy.
    #[must_use]
    pub fn tracing() -> Self {
        Self::tracing_with_digest(None)
    }

    /// The tracing-only recorder with the digest key of `[decision_log]` (Task 9). The tracing log
    /// ignores the digest, so no digest is computed until a Redis log takes its place.
    #[must_use]
    pub fn tracing_with_digest(digest: Option<DigestKey>) -> Self {
        Self::new(Arc::new(TracingDecisionLog), Arc::new(SystemClock), Arc::new(OsEntropy), digest)
    }

    #[must_use]
    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    #[must_use]
    pub fn log(&self) -> &Arc<dyn DecisionLog> {
        &self.log
    }

    /// The keyed digest of the arguments, when the log wants it and a key exists.
    #[must_use]
    pub fn digest(&self, arguments: &str) -> Option<ArgumentDigest> {
        if !self.log.wants_digest() {
            return None;
        }
        self.digest.as_ref().map(|k| k.digest(arguments.as_bytes()))
    }

    fn header(&self, org: Option<Uuid>) -> RecordHeader {
        let now = self.clock.now();
        let sequence = {
            let mut map = self.sequences.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let next = map.entry(org).or_insert(0);
            *next += 1;
            *next
        };
        RecordHeader::new(mint_uuid7(unix_ms(now), self.entropy.bytes10()), now, self.instance_id, sequence, org)
    }

    fn emit(&self, org: Option<Uuid>, build: impl FnOnce(RecordHeader) -> DecisionRecord) {
        self.log.record(build(self.header(org)));
    }

    pub fn record_response(&self, ctx: &DecisionContext, r: &ReportedCall) {
        self.emit(response_org(ctx).or_else(|| org_of(&ctx.scope)), |h| build_response(h, ctx, r));
    }

    pub fn record_request(&self, ctx: &DecisionContext, mode: Option<TenantMode>, e: &RequestEntry) {
        self.emit(org_of(&ctx.scope), |h| build_request(h, ctx, mode, e));
    }

    pub fn record_refusal(&self, ctx: &DecisionContext, mode: Option<TenantMode>) {
        self.emit(org_of(&ctx.scope), |h| build_refusal(h, ctx, mode));
    }
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::decision_log::MemoryDecisionLog;
    use crate::test_support::{FixedClock, ORG_A, at};
    use paigasus_gateway_core::{Basis, BlockCode, ReportDecision, ReportEntry};
    use sha2::Digest;

    struct FixedEntropy(u8);

    impl Entropy for FixedEntropy {
        fn bytes10(&self) -> [u8; 10] {
            [self.0; 10]
        }
    }

    const ORG_B: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000b2);

    fn scope(org: Uuid) -> String {
        format!("prn:pgs:iam:::organization/{org}")
    }

    fn ctx(scope: &str) -> DecisionContext {
        DecisionContext::new("prn:pgs:iam:::user/u", scope, None)
    }

    fn reported() -> ReportedCall {
        ReportedCall::new(
            ReportEntry {
                choice: 0,
                position: 0,
                name: "t".into(),
                decision: ReportDecision::Block,
                basis: Some(Basis::Decider),
                code: Some(BlockCode::Policy),
                enforced: Some(true),
                reason: None,
            },
            None,
            None,
        )
    }

    fn recorder(log: &Arc<MemoryDecisionLog>, key: Option<DigestKey>) -> DecisionRecorder {
        DecisionRecorder::new(log.clone(), Arc::new(FixedClock(at("2026-11-18T10:00:00Z"))), Arc::new(FixedEntropy(0xAB)), key)
    }

    fn key() -> DigestKey {
        DigestKey::new("k1".into(), "k".repeat(32).into()).unwrap()
    }

    #[test]
    fn sequences_start_at_1_per_org() {
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        let (a, b) = (scope(ORG_A), scope(ORG_B));
        for s in [a.as_str(), a.as_str(), b.as_str(), "", a.as_str()] {
            rec.record_response(&ctx(s), &reported());
        }
        let got: Vec<(Option<Uuid>, u64)> = log.records().iter().map(|r| (r.org, r.sequence)).collect();
        assert_eq!(got, vec![(Some(ORG_A), 1), (Some(ORG_A), 2), (Some(ORG_B), 1), (None, 1), (Some(ORG_A), 3)]);
        assert!(log.records().iter().all(|r| r.instance_id == rec.instance_id()));
    }

    #[test]
    fn request_and_refusal_records_share_the_sequence() {
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        let s = scope(ORG_A);
        rec.record_response(&ctx(&s), &reported());
        rec.record_refusal(&ctx(&s), None);
        let seqs: Vec<u64> = log.records().iter().map(|r| r.sequence).collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[test]
    fn the_decision_id_is_a_uuid7_from_the_clock_and_the_entropy() {
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        rec.record_response(&ctx(&scope(ORG_A)), &reported());
        let id = log.records()[0].decision_id;
        let ms = u64::try_from(at("2026-11-18T10:00:00Z").duration_since(UNIX_EPOCH).unwrap().as_millis()).unwrap();
        assert_eq!(id, mint_uuid7(ms, [0xAB; 10]));
        assert_eq!(id.get_version_num(), 7);
        assert_eq!(rec.instance_id(), id, "the instance id comes from the same clock and entropy");
    }

    #[test]
    fn the_digest_is_hmac_not_sha256() {
        let args = r#"{"secret":"s3cr3t-canary"}"#;
        let d = key().digest(args.as_bytes());
        // `printf '%s' '{"secret":"s3cr3t-canary"}' | openssl dgst -sha256 -hmac <32 x k>`
        let want = "a5a8105ba35977481bd4bf8edc32b49774ac718def3dae8db86a122eb6376bf2";
        let got: String = d.hmac_sha256.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got, want);
        assert_eq!(d.key_id, "k1");
        let plain: [u8; 32] = Sha256::digest(args.as_bytes()).into();
        assert_ne!(d.hmac_sha256, plain);
    }

    #[test]
    fn a_short_digest_key_is_refused() {
        assert!(DigestKey::new("id".into(), "short".into()).is_err());
        assert!(DigestKey::new("id".into(), "k".repeat(31).into()).is_err());
        assert!(DigestKey::new("id".into(), "k".repeat(32).into()).is_ok());
    }

    #[test]
    fn no_digest_without_wants_digest() {
        let off = Arc::new(MemoryDecisionLog::new(false));
        assert!(recorder(&off, Some(key())).digest("{}").is_none());
        let on = Arc::new(MemoryDecisionLog::new(true));
        assert!(recorder(&on, Some(key())).digest("{}").is_some());
        assert!(recorder(&on, None).digest("{}").is_none());
    }
}
