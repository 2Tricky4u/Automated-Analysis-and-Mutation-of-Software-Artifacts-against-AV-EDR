//! Executor trait for build-deploy-execute operations
//!
//! Abstracts the artifact lifecycle to enable testing and
//! potentially different execution backends.

use crate::run_queue::{PendingRun, RunResult};
use crate::worker_manager::WorkerManager;
use crate::worker_pool::WorkerState;
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use tracing::{debug, info};

/// Trait for build-deploy-execute operations
#[async_trait]
pub trait Executor: Send + Sync {
    /// Build artifact from PendingRun specification
    async fn build_artifact(&self, run: &PendingRun) -> Result<String>;

    /// Deploy artifact to worker
    async fn deploy_artifact(
        &self,
        artifact_id: &str,
        worker: &WorkerState,
        worker_manager: &WorkerManager,
    ) -> Result<()>;

    /// Execute artifact on worker
    async fn execute_on_worker(
        &self,
        run: &PendingRun,
        artifact_id: &str,
        worker: &WorkerState,
        worker_manager: &WorkerManager,
    ) -> Result<RunResult>;
}

/// Production executor using real build crate and worker communication
pub struct ProductionExecutor {
    output_dir: PathBuf,
}

impl ProductionExecutor {
    pub fn new() -> Self {
        let builder_config = build::BuilderConfig::default();
        Self {
            output_dir: builder_config.output_dir,
        }
    }

    pub fn with_output_dir(output_dir: PathBuf) -> Self {
        Self { output_dir }
    }
}

impl Default for ProductionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Executor for ProductionExecutor {
    async fn build_artifact(&self, run: &PendingRun) -> Result<String> {
        use build::{ArtifactBuilder, BuildInput, BuilderConfig, EncodingType, ModuleSelection};

        let builder_config = BuilderConfig::default();
        let artifact_builder = ArtifactBuilder::new(builder_config.clone())?;

        // Convert mutations to builder format
        let builder_mutations: Vec<build::mutator::MutationSpec> = run
            .mutations
            .iter()
            .map(|m| {
                let params = m
                    .params
                    .as_ref()
                    .and_then(|v| {
                        v.as_object().map(|obj| {
                            obj.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect::<std::collections::HashMap<String, String>>()
                        })
                    })
                    .unwrap_or_default();

                build::mutator::MutationSpec {
                    id: m.id.clone(),
                    params,
                }
            })
            .collect();

        // Build based on mode (modular or legacy)
        let built = if let Some(modular) = &run.modular_build {
            debug!(
                "[{}] Building artifact (modular: carrier={}, decoder={})",
                run.run_id, modular.modules.carrier, modular.modules.decoder
            );

            let modules = ModuleSelection {
                carrier: modular.modules.carrier.clone(),
                decoder: modular.modules.decoder.clone(),
                antiemulation: modular.modules.antiemulation.clone(),
                guardrail: modular.modules.guardrail.clone(),
                virtualprotect: modular.modules.virtualprotect.clone(),
                decoy: modular.modules.decoy.clone(),
            };

            let encoding =
                EncodingType::from_str(&modular.encoding).unwrap_or(EncodingType::Xor);

            artifact_builder
                .build(BuildInput::ModularTemplate {
                    modules,
                    payload: modular.payload.clone(),
                    encoding,
                    mutations: builder_mutations,
                    trace_mode: run.trace_mode.clone(),
                })
                .await?
        } else {
            debug!(
                "[{}] Building artifact (legacy: template={})",
                run.run_id, run.template_name
            );

            artifact_builder
                .build(BuildInput::SourceFile {
                    template_name: run.template_name.clone(),
                    source_file: run.source_file.clone(),
                    mutations: builder_mutations,
                    trace_mode: run.trace_mode.clone(),
                })
                .await?
        };

        info!(
            "[{}] Build complete: artifact_id={}, size={}",
            run.run_id, built.artifact_id, built.size_bytes
        );

        Ok(built.artifact_id)
    }

    async fn deploy_artifact(
        &self,
        artifact_id: &str,
        worker: &WorkerState,
        worker_manager: &WorkerManager,
    ) -> Result<()> {
        let artifact_path = self.output_dir.join(format!("{}.exe", artifact_id));

        if !artifact_path.exists() {
            return Err(anyhow::anyhow!(
                "Artifact {} not found at {:?}",
                artifact_id,
                artifact_path
            ));
        }

        debug!(
            "[{}] Deploying to worker {} at {}",
            artifact_id, worker.id, worker.address
        );

        worker_manager
            .send_artifact(&worker.id, artifact_id, &artifact_path)
            .await?;

        info!("[{}] Deployed to worker {}", artifact_id, worker.id);
        Ok(())
    }

    async fn execute_on_worker(
        &self,
        run: &PendingRun,
        artifact_id: &str,
        worker: &WorkerState,
        worker_manager: &WorkerManager,
    ) -> Result<RunResult> {
        use crate::automutate::common::SampleRequest;

        debug!(
            "[{}] Executing on worker {} (artifact: {})",
            run.run_id, worker.id, artifact_id
        );

        let request = SampleRequest {
            job_id: run.run_id.clone(),
            artifact_id: artifact_id.to_string(),
            timeout_seconds: 60,
            enable_etw: run.trace_mode != "off",
        };

        let exec_result = worker_manager
            .execute_artifact_stream(&worker.id, request)
            .await?;

        let detected = !exec_result.success;
        let exit_code = Some(exec_result.exit_code);

        info!(
            "[{}] Execution complete: success={}, detected={}, exit_code={:?}",
            run.run_id, exec_result.success, detected, exit_code
        );

        Ok(RunResult {
            run_id: run.run_id.clone(),
            success: exec_result.success,
            detected,
            exit_code,
            error: if exec_result.success {
                None
            } else {
                Some(exec_result.output.clone())
            },
        })
    }
}

#[cfg(test)]
pub struct MockExecutor {
    pub build_result: std::sync::Mutex<Option<Result<String>>>,
    pub deploy_result: std::sync::Mutex<Option<Result<()>>>,
    pub execute_result: std::sync::Mutex<Option<Result<RunResult>>>,
}

#[cfg(test)]
impl MockExecutor {
    pub fn new() -> Self {
        Self {
            build_result: std::sync::Mutex::new(None),
            deploy_result: std::sync::Mutex::new(None),
            execute_result: std::sync::Mutex::new(None),
        }
    }

    pub fn with_success(artifact_id: &str, run_result: RunResult) -> Self {
        Self {
            build_result: std::sync::Mutex::new(Some(Ok(artifact_id.to_string()))),
            deploy_result: std::sync::Mutex::new(Some(Ok(()))),
            execute_result: std::sync::Mutex::new(Some(Ok(run_result))),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl Executor for MockExecutor {
    async fn build_artifact(&self, _run: &PendingRun) -> Result<String> {
        self.build_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok("mock-artifact-id".to_string()))
    }

    async fn deploy_artifact(
        &self,
        _artifact_id: &str,
        _worker: &WorkerState,
        _worker_manager: &WorkerManager,
    ) -> Result<()> {
        self.deploy_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }

    async fn execute_on_worker(
        &self,
        run: &PendingRun,
        _artifact_id: &str,
        _worker: &WorkerState,
        _worker_manager: &WorkerManager,
    ) -> Result<RunResult> {
        self.execute_result.lock().unwrap().take().unwrap_or(Ok(RunResult {
            run_id: run.run_id.clone(),
            success: true,
            detected: false,
            exit_code: Some(0),
            error: None,
        }))
    }
}