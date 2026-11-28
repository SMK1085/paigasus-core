// SPDX-License-Identifier: Apache-2.0

//! The decision recorder (SMA-755 § 5.3, § 5.8, P-8). One recorder serves every report site. It
//! owns the instance id, the per-org sequences, the clock, the entropy and the digest key.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, KeyInit, Mac};
use paigasus_gateway_core::decision_log::{ArgumentDigest, CedarDetail, DecisionRecord, RecordHeader, ReportedCall, build_refusal, build_request, build_response, build_screen, response_org};
use paigasus_gateway_core::{DecisionContext, RequestEntry, ScreenRecord, TenantMode};
use paigasus_kernel::mint_uuid7;
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use uuid::Uuid;

use crate::domain::decision_log::{DecisionLog, Entropy, OsEntropy, TracingDecisionLog, log_line};
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

    /// A fresh decision id: a UUIDv7 from the clock and the entropy of this recorder.
    #[must_use]
    pub fn mint_decision_id(&self) -> Uuid {
        mint_uuid7(unix_ms(self.clock.now()), self.entropy.bytes10())
    }

    /// `id` is the decision id of the record. `None` mints a fresh one.
    fn header(&self, org: Option<Uuid>, id: Option<Uuid>) -> RecordHeader {
        // The clock is read inside the sequence lock, so `decided_at` order equals `sequence` order per org.
        let (now, sequence) = {
            let mut map = self.sequences.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = self.clock.now();
            let next = map.entry(org).or_insert(0);
            *next += 1;
            (now, *next)
        };
        RecordHeader::new(id.unwrap_or_else(|| mint_uuid7(unix_ms(now), self.entropy.bytes10())), now, self.instance_id, sequence, org)
    }

    /// Build the record, write the tracing line when `line` is set, and hand the record to the log.
    fn emit(&self, org: Option<Uuid>, id: Option<Uuid>, line: bool, build: impl FnOnce(RecordHeader) -> DecisionRecord) {
        let record = build(self.header(org, id));
        if line {
            log_line(&record);
        }
        self.log.record(record);
    }

    /// The tracing line is written exactly when the decider ran for this entry: `r.detail` is set (R11).
    pub fn record_response(&self, ctx: &DecisionContext, r: &ReportedCall) {
        self.emit(response_org(ctx).or_else(|| org_of(&ctx.scope)), r.decision_id, r.detail.is_some(), |h| build_response(h, ctx, r));
    }

    /// One request-side entry. `cedar` is the detail of the entry (SMA-855 § 6.11.2), `None` for an
    /// entry that no decision with findings made. The tracing line is written exactly when the
    /// entry has a detail: a Cedar decision with findings made it (§ 6.11.2, as R11).
    pub fn record_request(&self, ctx: &DecisionContext, mode: Option<TenantMode>, e: &RequestEntry, cedar: Option<&CedarDetail>) {
        self.emit(org_of(&ctx.scope), None, cedar.is_some(), |h| build_request(h, ctx, mode, e, cedar));
    }

    /// One tool result with findings that the gateway did not replace: a `SCREEN_RESULT` record
    /// with applied `Allow` (SMA-855 § 6.11.2). It always has a detail, so it always writes the
    /// tracing line.
    pub fn record_screen(&self, ctx: &DecisionContext, mode: Option<TenantMode>, r: &ScreenRecord) {
        self.emit(org_of(&ctx.scope), None, true, |h| build_screen(h, ctx, mode, r));
    }

    pub fn record_refusal(&self, ctx: &DecisionContext, mode: Option<TenantMode>) {
        self.emit(org_of(&ctx.scope), None, false, |h| build_refusal(h, ctx, mode));
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
    use paigasus_gateway_core::decision_log::{DecisionDetail, RecordBasis};
    use paigasus_gateway_core::{Basis, BlockCode, ReportDecision, ReportEntry};
    use paigasus_logging::test_support::capture_logs_at;
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
    fn a_reported_decision_id_is_the_record_id() {
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        let id = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000c3);
        rec.record_response(&ctx(&scope(ORG_A)), &reported().with_decision_id(Some(id)));
        rec.record_response(&ctx(&scope(ORG_A)), &reported());
        let ids: Vec<Uuid> = log.records().iter().map(|r| r.decision_id).collect();
        assert_eq!(ids[0], id);
        assert_ne!(ids[1], id);
        assert_eq!(ids[1].get_version_num(), 7, "an entry without an id gets a fresh UUIDv7");
        assert_eq!(rec.mint_decision_id().get_version_num(), 7);
    }

    fn entry_of(decision: ReportDecision, basis: Option<Basis>, code: Option<BlockCode>, enforced: Option<bool>) -> ReportEntry {
        ReportEntry {
            choice: 0,
            position: 0,
            name: "Get_Weather".into(),
            decision,
            basis,
            code,
            enforced,
            reason: None,
        }
    }

    fn with_detail(entry: ReportEntry, basis: RecordBasis) -> ReportedCall {
        ReportedCall::new(entry, None, Some(Arc::new(DecisionDetail::new(basis, None, None))))
    }

    /// The lines of the tracing capture that hold the decision message.
    fn decision_lines(text: &str) -> Vec<String> {
        text.lines().filter(|l| l.contains("tool-policy decision")).map(ToOwned::to_owned).collect()
    }

    fn has_field(line: &str, name: &str, value: &str) -> bool {
        line.contains(&format!("{name}={value}")) || line.contains(&format!("{name}=\"{value}\""))
    }

    #[test]
    fn a_decided_entry_writes_one_line_with_the_applied_verdict() {
        let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        let s = scope(ORG_A);
        let block = with_detail(entry_of(ReportDecision::Block, Some(Basis::Decider), Some(BlockCode::Policy), Some(true)), RecordBasis::Policy);
        rec.record_response(&DecisionContext::new("prn:pgs:iam:::user/u", &s, Some("req-1".into())), &block);
        let text = logs.text();
        let lines = decision_lines(&text);
        assert_eq!(lines.len(), 1, "{text}");
        for (name, value) in [("request_id", "req-1"), ("tool", "Get_Weather"), ("verdict", "block"), ("basis", "policy")] {
            assert!(has_field(&lines[0], name, value), "{name}={value} in {text}");
        }
        for field in ["mode=", "determining=", "errored="] {
            assert!(text.contains(field), "{field} in {text}");
        }
    }

    #[test]
    fn the_line_lists_the_finding_kinds_and_severities_and_no_evidence() {
        use paigasus_gateway_core::decision_log::{CedarDetail, FindingRecord};
        use paigasus_kernel::detect::{Evidence, Finding, Severity};
        use paigasus_kernel::tool_policy::{ActionGroup, Declared, Effect};
        const CANARY: &str = "canary-91c2.example";
        let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        let host = Finding::new("unseen_host", "unseen_host", Severity::Medium, "v1").with_evidence(Evidence::host("r", Some("/u"), CANARY.to_owned()));
        let cedar = CedarDetail::new(
            Effect::Deny,
            vec![],
            vec![],
            false,
            vec![FindingRecord::from_finding(&host), FindingRecord::new("name_collision", None, "", "")],
            Some(Severity::High),
            Declared::Yes,
            ActionGroup::Catalog,
            std::time::Duration::ZERO,
        );
        let call = ReportedCall::new(
            entry_of(ReportDecision::Block, Some(Basis::Decider), Some(BlockCode::Policy), Some(true)),
            None,
            Some(Arc::new(DecisionDetail::new(RecordBasis::Policy, Some(cedar), None))),
        );
        rec.record_response(&ctx(&scope(ORG_A)), &call);
        let text = logs.text();
        let lines = decision_lines(&text);
        assert_eq!(lines.len(), 1, "{text}");
        assert!(has_field(&lines[0], "findings", "name_collision:unknown,unseen_host:medium"), "{text}");
        assert!(!text.contains(CANARY), "{text}");
    }

    /// SMA-855 D37 (carried from Task 11): a finding with a bad kind from a detector never reaches the
    /// line or a metric label. The pipeline replaces it before any record is built.
    #[tokio::test]
    async fn a_bad_kind_from_a_detector_never_reaches_the_line_or_a_label() {
        use crate::adapters::detectors::BuiltInDetector;
        use crate::application::detectors::DetectorPipeline;
        use crate::application::tool_policy::testing::FixedDetector;
        use crate::application::tool_policy::{FINDING_KINDS, finding_label};
        use crate::domain::detectors::{DetectBatch, Detector, Subject};
        use paigasus_gateway_core::decision_log::{CedarDetail, FindingRecord};
        use paigasus_kernel::detect::{Finding, Severity, ToolInput};
        use paigasus_kernel::tool_policy::{ActionGroup, Declared, Effect, ToolInvocation};
        const CANARY: &str = "canary-3b7e";
        let bad = [
            format!("a,{CANARY}"),
            format!("a:{CANARY}"),
            format!("a\n{CANARY}=x"),
            format!("sk_live_{CANARY}"),
            format!("Bad-{CANARY}"),
        ];
        let hook: Arc<dyn Detector> = Arc::new(FixedDetector("hook_a", bad.iter().map(|k| Finding::new("hook_a", k.as_str(), Severity::Medium, "v1")).collect()));
        let pipeline = DetectorPipeline::new(Arc::new(BuiltInDetector::new(65536)), vec![hook], None, std::time::Duration::from_millis(500));
        let subject = Subject::Call {
            index: 0,
            invocation: ToolInvocation::new("t", ToolInput::Json("{}".into()), Declared::Yes),
            schema: None,
        };
        let out = pipeline
            .run(
                DetectBatch::for_calls(
                    None,
                    None,
                    vec![subject],
                    None,
                    crate::domain::detectors::Caller {
                        opt_in: Arc::from(vec!["hook_a".to_owned()]),
                        ..crate::domain::detectors::Caller::none()
                    },
                ),
                tokio::time::Instant::now() + std::time::Duration::from_secs(60),
            )
            .await;
        let found = &out.findings[0];
        for f in found {
            assert!(FINDING_KINDS.contains(&finding_label(&f.kind)), "{f:?}");
            assert!(!f.kind.contains(CANARY) && !f.detector.contains(CANARY), "{f:?}");
        }
        let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        let cedar = CedarDetail::new(
            Effect::Deny,
            vec![],
            vec![],
            false,
            found.iter().map(FindingRecord::from_finding).collect(),
            Some(Severity::High),
            Declared::Yes,
            ActionGroup::Catalog,
            std::time::Duration::ZERO,
        );
        let call = ReportedCall::new(
            entry_of(ReportDecision::Block, Some(Basis::Decider), Some(BlockCode::Policy), Some(true)),
            None,
            Some(Arc::new(DecisionDetail::new(RecordBasis::Policy, Some(cedar), None))),
        );
        rec.record_response(&ctx(&scope(ORG_A)), &call);
        let text = logs.text();
        let lines = decision_lines(&text);
        assert_eq!(lines.len(), 1, "{text}");
        assert!(has_field(&lines[0], "findings", "detector_error:high"), "{text}");
        assert!(!text.contains(CANARY), "{text}");
    }

    #[test]
    fn a_line_without_findings_has_an_empty_findings_field() {
        let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        let c = with_detail(entry_of(ReportDecision::Allow, Some(Basis::Decider), None, None), RecordBasis::Policy);
        rec.record_response(&ctx(&scope(ORG_A)), &c);
        let text = logs.text();
        assert!(decision_lines(&text)[0].contains("findings="), "{text}");
    }

    #[test]
    fn an_observed_hold_writes_one_line_with_verdict_observe() {
        let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        let observed = with_detail(entry_of(ReportDecision::Allow, Some(Basis::Decider), Some(BlockCode::Policy), Some(false)), RecordBasis::Policy);
        rec.record_response(&ctx(&scope(ORG_A)), &observed);
        let text = logs.text();
        let lines = decision_lines(&text);
        assert_eq!(lines.len(), 1, "{text}");
        assert!(has_field(&lines[0], "verdict", "observe"), "{text}");
    }

    #[test]
    fn an_unavailable_policy_decision_writes_one_line_with_basis_unavailable() {
        let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        let c = with_detail(entry_of(ReportDecision::Block, Some(Basis::Decider), Some(BlockCode::Policy), Some(true)), RecordBasis::Unavailable);
        rec.record_response(&ctx(&scope(ORG_A)), &c);
        let text = logs.text();
        let lines = decision_lines(&text);
        assert_eq!(lines.len(), 1, "{text}");
        assert!(has_field(&lines[0], "basis", "unavailable"), "{text}");
    }

    #[test]
    fn an_unenforceable_a_block_all_and_a_request_record_write_no_line() {
        let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = recorder(&log, None);
        let c = ctx(&scope(ORG_A));
        // A provider-executed tool (web_search_call): reported, never decided, so no detail.
        rec.record_response(&c, &ReportedCall::new(entry_of(ReportDecision::Unenforceable, None, None, None), None, None));
        // A hold-limit `block_all` entry: the decider did not run, so no detail.
        rec.record_response(
            &c,
            &ReportedCall::new(entry_of(ReportDecision::Block, Some(Basis::OnFailure), Some(BlockCode::Policy), Some(true)), None, None),
        );
        rec.record_refusal(&c, None);
        assert_eq!(log.records().len(), 3, "each entry is still recorded");
        assert_eq!(decision_lines(&logs.text()), Vec::<String>::new(), "{}", logs.text());
    }

    /// A clock that records whether the sequence lock of the recorder was held while it was read.
    struct LockProbe {
        recorder: std::sync::OnceLock<Arc<DecisionRecorder>>,
        reads_with_lock_free: std::sync::atomic::AtomicUsize,
    }

    impl Clock for LockProbe {
        fn now(&self) -> SystemTime {
            if let Some(r) = self.recorder.get() {
                // `try_lock` fails while another holder has the lock. The recorder holds it in `header`.
                if r.sequences.try_lock().is_ok() {
                    self.reads_with_lock_free.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
            at("2026-11-18T10:00:00Z")
        }
    }

    #[test]
    fn the_clock_is_read_inside_the_sequence_lock() {
        let probe = Arc::new(LockProbe {
            recorder: std::sync::OnceLock::new(),
            reads_with_lock_free: std::sync::atomic::AtomicUsize::new(0),
        });
        let log = Arc::new(MemoryDecisionLog::new(false));
        let rec = Arc::new(DecisionRecorder::new(log, probe.clone(), Arc::new(FixedEntropy(1)), None));
        probe.recorder.set(Arc::clone(&rec)).ok().expect("set once");
        rec.record_response(&ctx(&scope(ORG_A)), &reported());
        assert_eq!(
            probe.reads_with_lock_free.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "decided_at must be read under the sequence lock"
        );
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
