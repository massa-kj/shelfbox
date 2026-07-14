//! D6 operation-facing contract tests.
//!
//! These fake adapters model what an operation may observe: typed actions,
//! inspection facts, durable phases, and opaque permits. No test helper uses a
//! temp path, file identity, symlink helper, or transfer algorithm.

use std::{cell::RefCell, rc::Rc};

use crate::{
    domain::{
        copy_safety::{ArtifactScope, WRITE_PRECONDITION_CHECKS},
        materialization::MaterializationStrategy,
        path::{RepoRelativePath, StoreRelativePath},
    },
    error::Result,
    fs::{
        canonical_transfer::{
            CanonicalEntryKind, CanonicalInspectionPurpose, CanonicalTransfer,
            CanonicalTransferAction, CanonicalTransferCommitOutcome, CanonicalTransferFacts,
            CanonicalTransferInspectionRequest, PreparedCanonicalTransfer,
        },
        materializer::{
            ArtifactLease, CommitContext, CommitPermit, DurableOperationPhase, InspectionPurpose,
            MaterializationAction, MaterializationCommitOutcome, MaterializationFacts,
            MaterializationInspectionRequest, MaterializationLocation, Materializer,
            MutationJournal, PreparedMaterialization, RepoEntryKind, WritableArtifactLease,
            WritePreconditionGuard,
        },
    },
};

#[derive(Default)]
struct FakeJournal {
    events: Rc<RefCell<Vec<&'static str>>>,
    next_token: u64,
}

impl FakeJournal {
    fn with_events(events: Rc<RefCell<Vec<&'static str>>>) -> Self {
        Self {
            events,
            next_token: 1,
        }
    }

    fn event(&self, event: &'static str) {
        self.events.borrow_mut().push(event);
    }
}

impl MutationJournal for FakeJournal {
    fn acquire_artifact_lease(&mut self, scope: ArtifactScope) -> Result<ArtifactLease> {
        self.event(match scope {
            ArtifactScope::RepoSide => "lease_repo_side",
            ArtifactScope::StoreSide => "lease_store_side",
        });
        let token = self.next_token;
        self.next_token += 1;
        Ok(ArtifactLease::for_test(scope, token))
    }

    fn authorize_plaintext_write(
        &mut self,
        _lease: ArtifactLease,
    ) -> Result<WritableArtifactLease> {
        self.event("plaintext_authorized");
        let token = self.next_token;
        self.next_token += 1;
        Ok(WritableArtifactLease::for_test(token))
    }

    fn record_phase(&mut self, phase: DurableOperationPhase) -> Result<()> {
        self.event(match phase {
            DurableOperationPhase::MaterializationPrepared => "phase_materialization_prepared",
            DurableOperationPhase::CanonicalTransferPrepared => "phase_transfer_prepared",
            DurableOperationPhase::CommitAuthorized => "phase_commit_authorized",
            DurableOperationPhase::MaterializationCommitted => "phase_materialization_committed",
            DurableOperationPhase::CanonicalTransferCommitted => "phase_transfer_committed",
            DurableOperationPhase::PostCommitValidated => "phase_post_commit_validated",
        });
        Ok(())
    }

    fn issue_commit_permit(&mut self, guard: WritePreconditionGuard) -> Result<CommitPermit> {
        assert_eq!(guard.required_checks(), WRITE_PRECONDITION_CHECKS);
        self.event("commit_permit_issued");
        let token = self.next_token;
        self.next_token += 1;
        Ok(CommitPermit::for_test(token))
    }

    fn cleanup_prepared_artifact(&mut self, _context: CommitContext) -> Result<()> {
        self.event("artifact_cleaned");
        Ok(())
    }
}

struct FakeMaterializer {
    events: Rc<RefCell<Vec<&'static str>>>,
    actions: Vec<MaterializationAction>,
}

impl FakeMaterializer {
    fn new(events: Rc<RefCell<Vec<&'static str>>>) -> Self {
        Self {
            events,
            actions: Vec::new(),
        }
    }

    fn event(&self, event: &'static str) {
        self.events.borrow_mut().push(event);
    }
}

impl Materializer for FakeMaterializer {
    fn inspect(&self, request: MaterializationInspectionRequest) -> Result<MaterializationFacts> {
        self.event(match request.purpose {
            InspectionPurpose::Planning => "materializer_inspect_planning",
            InspectionPurpose::PreCommit => "materializer_inspect_pre_commit",
            InspectionPurpose::PostCommit => "materializer_inspect_post_commit",
        });
        Ok(MaterializationFacts::for_test(RepoEntryKind::RegularFile))
    }

    fn prepare(
        &mut self,
        action: MaterializationAction,
        journal: &mut dyn MutationJournal,
    ) -> Result<PreparedMaterialization> {
        let lease = journal.acquire_artifact_lease(ArtifactScope::RepoSide)?;
        let _writable = journal.authorize_plaintext_write(lease)?;
        self.event("materializer_prepared");
        self.actions.push(action);
        Ok(PreparedMaterialization::for_test(101))
    }

    fn commit(
        &mut self,
        _prepared: PreparedMaterialization,
        _permit: CommitPermit,
    ) -> Result<MaterializationCommitOutcome> {
        self.event("materializer_committed");
        Ok(MaterializationCommitOutcome::Applied)
    }

    fn abort(
        &mut self,
        prepared: PreparedMaterialization,
        journal: &mut dyn MutationJournal,
    ) -> Result<()> {
        journal.cleanup_prepared_artifact(prepared.commit_context())
    }
}

struct FakeCanonicalTransfer {
    events: Rc<RefCell<Vec<&'static str>>>,
    actions: Vec<CanonicalTransferAction>,
}

impl FakeCanonicalTransfer {
    fn new(events: Rc<RefCell<Vec<&'static str>>>) -> Self {
        Self {
            events,
            actions: Vec::new(),
        }
    }

    fn event(&self, event: &'static str) {
        self.events.borrow_mut().push(event);
    }
}

impl CanonicalTransfer for FakeCanonicalTransfer {
    fn inspect(
        &self,
        request: CanonicalTransferInspectionRequest,
    ) -> Result<CanonicalTransferFacts> {
        self.event(match request.purpose {
            CanonicalInspectionPurpose::Planning => "transfer_inspect_planning",
            CanonicalInspectionPurpose::PreCommit => "transfer_inspect_pre_commit",
            CanonicalInspectionPurpose::PostCommit => "transfer_inspect_post_commit",
        });
        Ok(CanonicalTransferFacts::for_test(
            CanonicalEntryKind::RegularFile,
        ))
    }

    fn prepare(
        &mut self,
        action: CanonicalTransferAction,
        journal: &mut dyn MutationJournal,
    ) -> Result<PreparedCanonicalTransfer> {
        let lease = journal.acquire_artifact_lease(ArtifactScope::StoreSide)?;
        let _writable = journal.authorize_plaintext_write(lease)?;
        self.event("transfer_prepared");
        self.actions.push(action);
        Ok(PreparedCanonicalTransfer::for_test(202))
    }

    fn commit(
        &mut self,
        _prepared: PreparedCanonicalTransfer,
        _permit: CommitPermit,
    ) -> Result<CanonicalTransferCommitOutcome> {
        self.event("transfer_committed");
        Ok(CanonicalTransferCommitOutcome::Applied)
    }

    fn abort(
        &mut self,
        prepared: PreparedCanonicalTransfer,
        journal: &mut dyn MutationJournal,
    ) -> Result<()> {
        journal.cleanup_prepared_artifact(prepared.commit_context())
    }
}

fn orchestrate_materialization(
    materializer: &mut dyn Materializer,
    journal: &mut dyn MutationJournal,
    action: MaterializationAction,
) -> Result<()> {
    let location = action
        .location()
        .expect("test operation requires a mutating materialization action")
        .clone();
    let prepared = materializer.prepare(action, journal)?;
    journal.record_phase(DurableOperationPhase::MaterializationPrepared)?;

    let pre_commit_facts = materializer.inspect(MaterializationInspectionRequest {
        location: location.clone(),
        purpose: InspectionPurpose::PreCommit,
    })?;
    let permit = journal.issue_commit_permit(
        pre_commit_facts.write_precondition_guard(prepared.commit_context()),
    )?;
    journal.record_phase(DurableOperationPhase::CommitAuthorized)?;
    materializer.commit(prepared, permit)?;
    journal.record_phase(DurableOperationPhase::MaterializationCommitted)?;

    materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::PostCommit,
    })?;
    journal.record_phase(DurableOperationPhase::PostCommitValidated)
}

fn orchestrate_canonical_transfer(
    transfer: &mut dyn CanonicalTransfer,
    journal: &mut dyn MutationJournal,
    action: CanonicalTransferAction,
) -> Result<()> {
    let prepared = transfer.prepare(action.clone(), journal)?;
    journal.record_phase(DurableOperationPhase::CanonicalTransferPrepared)?;

    let pre_commit_facts = transfer.inspect(CanonicalTransferInspectionRequest {
        action: action.clone(),
        purpose: CanonicalInspectionPurpose::PreCommit,
    })?;
    let permit = journal.issue_commit_permit(
        pre_commit_facts.write_precondition_guard(prepared.commit_context()),
    )?;
    journal.record_phase(DurableOperationPhase::CommitAuthorized)?;
    transfer.commit(prepared, permit)?;
    journal.record_phase(DurableOperationPhase::CanonicalTransferCommitted)?;

    transfer.inspect(CanonicalTransferInspectionRequest {
        action,
        purpose: CanonicalInspectionPurpose::PostCommit,
    })?;
    journal.record_phase(DurableOperationPhase::PostCommitValidated)
}

#[test]
fn operation_orchestrates_a_typed_materialization_action_and_durable_phases() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut materializer = FakeMaterializer::new(events.clone());
    let mut journal = FakeJournal::with_events(events.clone());
    let location = MaterializationLocation::new(
        RepoRelativePath::new("config/secret.env").unwrap(),
        StoreRelativePath::new("items/config/secret.env").unwrap(),
    );
    let action = MaterializationAction::Create {
        location,
        strategy: MaterializationStrategy::Copy,
    };

    orchestrate_materialization(&mut materializer, &mut journal, action.clone()).unwrap();

    assert_eq!(materializer.actions, vec![action]);
    assert_eq!(
        events.borrow().as_slice(),
        [
            "lease_repo_side",
            "plaintext_authorized",
            "materializer_prepared",
            "phase_materialization_prepared",
            "materializer_inspect_pre_commit",
            "commit_permit_issued",
            "phase_commit_authorized",
            "materializer_committed",
            "phase_materialization_committed",
            "materializer_inspect_post_commit",
            "phase_post_commit_validated",
        ]
    );
}

#[test]
fn operation_orchestrates_a_strategy_neutral_canonical_transfer_and_durable_phases() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut transfer = FakeCanonicalTransfer::new(events.clone());
    let mut journal = FakeJournal::with_events(events.clone());
    let expected =
        CanonicalTransferFacts::for_test(CanonicalEntryKind::RegularFile).expected_source();
    let action = CanonicalTransferAction::Move {
        source: StoreRelativePath::new("items/old.env").unwrap(),
        destination: StoreRelativePath::new("items/new.env").unwrap(),
        expected_source: expected,
        expected_destination: CanonicalTransferFacts::for_test(CanonicalEntryKind::RegularFile)
            .expected_destination(),
    };

    orchestrate_canonical_transfer(&mut transfer, &mut journal, action.clone()).unwrap();

    assert_eq!(transfer.actions, vec![action]);
    assert_eq!(
        events.borrow().as_slice(),
        [
            "lease_store_side",
            "plaintext_authorized",
            "transfer_prepared",
            "phase_transfer_prepared",
            "transfer_inspect_pre_commit",
            "commit_permit_issued",
            "phase_commit_authorized",
            "transfer_committed",
            "phase_transfer_committed",
            "transfer_inspect_post_commit",
            "phase_post_commit_validated",
        ]
    );
}

#[test]
fn prepared_handles_have_no_transfer_or_artifact_details() {
    let materialization = format!("{:?}", PreparedMaterialization::for_test(1));
    let transfer = format!("{:?}", PreparedCanonicalTransfer::for_test(2));

    for rendered in [materialization, transfer] {
        assert!(rendered.contains("opaque"));
        assert!(!rendered.contains("path"));
        assert!(!rendered.contains("identity"));
        assert!(!rendered.contains("symlink"));
        assert!(!rendered.contains("copy"));
    }
}
