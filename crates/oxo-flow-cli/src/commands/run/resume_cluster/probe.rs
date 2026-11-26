//! The scheduler seam `resume` re-attach talks through, plus the route
//! (backend/queue) a recorded job group was submitted with.
//!
//! [`ClusterJobProbe`] is intentionally three operations wide: poll a set of
//! ids, read one accounting record, cancel one id. The production
//! implementation delegates to the same [`oxo_flow_core::backend::ExecutorBackend`]
//! the driver uses,
//! so the two surfaces cannot drift on scheduler state mapping; unit tests
//! inject a fake probe and never touch a live scheduler.

use anyhow::Result;
use oxo_flow_core::backend::cluster::ClusterExecutor;
use oxo_flow_core::backend::{BackendJobStatus, ExecutorBackend, TerminalRecord};
use oxo_flow_core::cluster::{ClusterBackend, ClusterJobConfig};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

/// A future the reconciliation can await inside a `&dyn` probe — the same
/// boxed-future shape the driver's hooks use, so the CLI needs no
/// `async_trait` dependency for its test seam.
pub(crate) type ProbeFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The scheduler operations re-attach needs, and nothing else.
pub(crate) trait ClusterJobProbe: Send + Sync {
    /// Live/terminal statuses for the given ids; ids the scheduler does not
    /// list are simply absent (read as [`BackendJobStatus::Unknown`]).
    fn poll_jobs<'a>(
        &'a self,
        ids: &'a [String],
    ) -> ProbeFuture<'a, Result<HashMap<String, BackendJobStatus>>>;

    /// Terminal record from the accounting store, or `None` when the store
    /// has nothing for the id.
    fn terminal_record<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Option<TerminalRecord>>;

    /// Cancel one scheduler job (an array element cancels through its base).
    fn cancel_job<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Result<()>>;

    /// Whether the backend can answer for individual array elements; when
    /// false, polling targets the array base id.
    fn polls_elements_directly(&self) -> bool;
}

/// [`ClusterJobProbe`] over the production
/// [`oxo_flow_core::backend::ExecutorBackend`].
pub(crate) struct BackendProbe {
    backend: Arc<dyn ExecutorBackend>,
}

impl BackendProbe {
    pub(crate) fn new(backend: Arc<dyn ExecutorBackend>) -> Self {
        Self { backend }
    }
}

impl ClusterJobProbe for BackendProbe {
    fn poll_jobs<'a>(
        &'a self,
        ids: &'a [String],
    ) -> ProbeFuture<'a, Result<HashMap<String, BackendJobStatus>>> {
        Box::pin(async move {
            self.backend
                .poll(ids)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
    }

    fn terminal_record<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Option<TerminalRecord>> {
        Box::pin(async move { self.backend.terminal_status(id).await })
    }

    fn cancel_job<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Result<()>> {
        Box::pin(async move {
            self.backend
                .cancel(id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
    }

    fn polls_elements_directly(&self) -> bool {
        self.backend.polls_elements_directly()
    }
}

/// The scheduler route one group of recorded jobs was submitted through.
///
/// A checkpoint's `cluster_run` records the LATEST cluster invocation's
/// profile/queue, which need not be the route an older recorded job used. The
/// probe is therefore built from each job's own recorded `backend`, and this
/// route carries the queue/account only when it provably belongs to that
/// backend (the group matches `cluster_run.backend`). The command strings
/// quote the full route so a copy-pasted recovery command addresses the right
/// scheduler.
#[derive(Debug, Clone)]
pub(crate) struct SchedulerRoute {
    pub(crate) backend: String,
    pub(crate) queue: Option<String>,
}

impl SchedulerRoute {
    pub(crate) fn new(backend: impl Into<String>, queue: Option<String>) -> Self {
        Self {
            backend: backend.into(),
            queue,
        }
    }

    fn queue_arg(&self) -> String {
        match &self.queue {
            Some(queue) => format!("--queue {queue} "),
            None => String::new(),
        }
    }

    /// `oxo-flow cluster status -b <backend> [--queue <q>] <id>`
    pub(crate) fn status_command(&self, job_id: &str) -> String {
        format!(
            "oxo-flow cluster status -b {} {}{job_id}",
            self.backend,
            self.queue_arg()
        )
    }

    /// `oxo-flow cluster cancel -b <backend> [--queue <q>] <id>`
    pub(crate) fn cancel_command(&self, job_id: &str) -> String {
        format!(
            "oxo-flow cluster cancel -b {} {}{job_id}",
            self.backend,
            self.queue_arg()
        )
    }
}

/// Build the production probe for one recorded backend.
///
/// `queue`/`account` must be passed only when they belong to this backend
/// (callers do this by comparing with the checkpoint's recorded cluster run);
/// a value borrowed from a different backend's route would be worse than
/// none.
pub(crate) fn recorded_probe(
    backend_name: &str,
    queue: Option<&str>,
    account: Option<&str>,
) -> Result<BackendProbe> {
    let backend = ClusterBackend::from_str(backend_name).map_err(|_| {
        anyhow::anyhow!(
            "checkpoint records cluster backend '{backend_name}', which this build does not know — expected {}",
            oxo_flow_core::cluster::SUPPORTED_BACKENDS_HINT
        )
    })?;
    let executor = ClusterExecutor::new(
        backend,
        ClusterJobConfig {
            backend,
            queue: queue.map(str::to_string),
            account: account.map(str::to_string),
            walltime: None,
            extra_args: Vec::new(),
            name_scope: None,
            volume: None,
        },
    );
    Ok(BackendProbe::new(Arc::new(executor)))
}
