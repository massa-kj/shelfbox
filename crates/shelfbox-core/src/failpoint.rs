//! Test-only interruption hooks for durable mutation boundaries.
//!
//! Production builds always use a no-op hook.  Tests install a hook in the
//! current thread and may return an error immediately after a durable
//! mutation.  Callers must not compensate for that error: it models a process
//! that stopped after persistence and before its next instruction.

use crate::{
    domain::{copy_safety::PersistentMutation, operation_record::OperationPhase},
    error::Result,
};

/// Named durable and non-durable mutation boundaries used by crash tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failpoint {
    OperationRecordCreated,
    OperationPhaseUpdated(OperationPhase),
    ArtifactPathRecorded,
    TempIdentityRecorded,
    RecordDeleted,
    PersistentMutation(PersistentMutation),
    KeepStoreManifestSaved,
    DirectionlessRelinkMaterialized,
    DirectionlessRelinkManifestSaved,
    RepoRepairTargetExcludeUpdated,
    /// Runs after operation-level preconditions and before the journal repeats
    /// Git/exclude and artifact checks to authorize commit.
    WritePreconditionsValidated,
}

/// Runs the injected hook after a mutation boundary.
#[cfg(not(test))]
pub(crate) fn after(_point: Failpoint) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod test_hook {
    use std::cell::RefCell;

    use super::*;

    type Hook = Box<dyn FnMut(&Failpoint) -> Result<()> + 'static>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = RefCell::new(None);
    }

    /// Restores the previous hook when it leaves scope, keeping parallel tests
    /// isolated from each other.
    pub(crate) struct TestHookGuard {
        previous: Option<Hook>,
    }

    impl Drop for TestHookGuard {
        fn drop(&mut self) {
            HOOK.with(|slot| {
                *slot.borrow_mut() = self.previous.take();
            });
        }
    }

    pub(crate) fn install_test_hook(
        hook: impl FnMut(&Failpoint) -> Result<()> + 'static,
    ) -> TestHookGuard {
        HOOK.with(|slot| TestHookGuard {
            previous: (*slot.borrow_mut()).replace(Box::new(hook)),
        })
    }

    pub(crate) fn after(point: Failpoint) -> Result<()> {
        HOOK.with(|slot| match slot.borrow_mut().as_mut() {
            Some(hook) => hook(&point),
            None => Ok(()),
        })
    }
}

#[cfg(test)]
pub(crate) use test_hook::after;
#[cfg(test)]
pub(crate) use test_hook::install_test_hook;

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    #[test]
    fn installed_hook_is_thread_local_and_runs_after_each_named_boundary() {
        let observed = Rc::new(RefCell::new(Vec::new()));
        let expected = observed.clone();
        let _guard = install_test_hook(move |point| {
            expected.borrow_mut().push(point.clone());
            Ok(())
        });

        let points = [
            Failpoint::OperationRecordCreated,
            Failpoint::OperationPhaseUpdated(OperationPhase::ManifestSaved),
            Failpoint::ArtifactPathRecorded,
            Failpoint::TempIdentityRecorded,
            Failpoint::RecordDeleted,
            Failpoint::PersistentMutation(PersistentMutation::PlaintextWrite),
            Failpoint::KeepStoreManifestSaved,
            Failpoint::DirectionlessRelinkMaterialized,
            Failpoint::DirectionlessRelinkManifestSaved,
            Failpoint::RepoRepairTargetExcludeUpdated,
            Failpoint::WritePreconditionsValidated,
        ];
        for point in points.clone() {
            after(point).unwrap();
        }

        assert_eq!(*observed.borrow(), points);
    }
}
