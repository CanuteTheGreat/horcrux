//! Job and CronJob operations
//!
//! CRUD operations for Kubernetes Jobs and CronJobs.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{CreateCronJobRequest, CreateJobRequest, CronJobInfo, JobInfo, JobStatus};

// ============================================================================
// Job Operations
// ============================================================================

/// List Jobs in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_jobs(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<JobInfo>> {
    use k8s_openapi::api::batch::v1::Job;
    use kube::api::{Api, ListParams};

    let jobs: Api<Job> = Api::namespaced(client.inner().clone(), namespace);
    let list = jobs.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(job_to_info).collect())
}

/// Get a specific Job
#[cfg(feature = "kubernetes")]
pub async fn get_job(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<JobInfo> {
    use k8s_openapi::api::batch::v1::Job;
    use kube::api::Api;

    let jobs: Api<Job> = Api::namespaced(client.inner().clone(), namespace);
    let job = jobs.get(name).await?;

    Ok(job_to_info(job))
}

/// Create a new Job
#[cfg(feature = "kubernetes")]
pub async fn create_job(
    client: &K8sClient,
    request: &CreateJobRequest,
) -> K8sResult<JobInfo> {
    use k8s_openapi::api::batch::v1::{Job, JobSpec};
    use k8s_openapi::api::core::v1::{Container, EnvVar, PodSpec, PodTemplateSpec};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let jobs: Api<Job> = Api::namespaced(client.inner().clone(), &request.namespace);

    // Build environment variables
    let env: Option<Vec<EnvVar>> = if request.env.is_empty() {
        None
    } else {
        Some(
            request
                .env
                .iter()
                .map(|(k, v)| EnvVar {
                    name: k.clone(),
                    value: Some(v.clone()),
                    ..Default::default()
                })
                .collect(),
        )
    };

    // Build container
    let container = Container {
        name: request.name.clone(),
        image: Some(request.image.clone()),
        command: request.command.clone(),
        args: request.args.clone(),
        env,
        ..Default::default()
    };

    // Build pod spec
    let pod_spec = PodSpec {
        containers: vec![container],
        restart_policy: Some(
            request
                .restart_policy
                .clone()
                .unwrap_or_else(|| "Never".to_string()),
        ),
        ..Default::default()
    };

    // Build job
    let job = Job {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            namespace: Some(request.namespace.clone()),
            labels: if request.labels.is_empty() {
                None
            } else {
                Some(request.labels.clone())
            },
            ..Default::default()
        },
        spec: Some(JobSpec {
            completions: request.completions,
            parallelism: request.parallelism,
            backoff_limit: request.backoff_limit,
            active_deadline_seconds: request.active_deadline_seconds,
            ttl_seconds_after_finished: request.ttl_seconds_after_finished,
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: if request.labels.is_empty() {
                        None
                    } else {
                        Some(request.labels.clone())
                    },
                    ..Default::default()
                }),
                spec: Some(pod_spec),
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    let created = jobs.create(&PostParams::default(), &job).await?;
    Ok(job_to_info(created))
}

/// Delete a Job
#[cfg(feature = "kubernetes")]
pub async fn delete_job(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    propagation_policy: Option<&str>,
) -> K8sResult<()> {
    use k8s_openapi::api::batch::v1::Job;
    use kube::api::{Api, DeleteParams};

    let jobs: Api<Job> = Api::namespaced(client.inner().clone(), namespace);

    let dp = match propagation_policy {
        Some("Orphan") => DeleteParams {
            propagation_policy: Some(kube::api::PropagationPolicy::Orphan),
            ..Default::default()
        },
        Some("Foreground") => DeleteParams {
            propagation_policy: Some(kube::api::PropagationPolicy::Foreground),
            ..Default::default()
        },
        Some(_) => DeleteParams {
            propagation_policy: Some(kube::api::PropagationPolicy::Background),
            ..Default::default()
        },
        None => DeleteParams::default(),
    };

    jobs.delete(name, &dp).await?;
    Ok(())
}

#[cfg(feature = "kubernetes")]
fn job_to_info(job: k8s_openapi::api::batch::v1::Job) -> JobInfo {
    let metadata = job.metadata;
    let spec = job.spec.unwrap_or_default();
    let status = job.status.unwrap_or_default();

    // Determine job status
    let job_status = if status.succeeded.unwrap_or(0) > 0
        && status.succeeded == spec.completions
    {
        JobStatus::Complete
    } else if status.failed.unwrap_or(0) > 0 {
        JobStatus::Failed
    } else if status.active.unwrap_or(0) > 0 {
        JobStatus::Running
    } else if spec.suspend.unwrap_or(false) {
        JobStatus::Suspended
    } else if status.start_time.is_none() {
        JobStatus::Pending
    } else {
        JobStatus::Unknown
    };

    JobInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        status: job_status,
        completions: spec.completions,
        succeeded: status.succeeded.unwrap_or(0),
        failed: status.failed.unwrap_or(0),
        active: status.active.unwrap_or(0),
        parallelism: spec.parallelism,
        backoff_limit: spec.backoff_limit,
        labels: metadata.labels.unwrap_or_default(),
        start_time: status.start_time.map(|t| t.0.to_rfc3339()),
        completion_time: status.completion_time.map(|t| t.0.to_rfc3339()),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// CronJob Operations
// ============================================================================

/// List CronJobs in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_cronjobs(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<CronJobInfo>> {
    use k8s_openapi::api::batch::v1::CronJob;
    use kube::api::{Api, ListParams};

    let cronjobs: Api<CronJob> = Api::namespaced(client.inner().clone(), namespace);
    let list = cronjobs.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(cronjob_to_info).collect())
}

/// Get a specific CronJob
#[cfg(feature = "kubernetes")]
pub async fn get_cronjob(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<CronJobInfo> {
    use k8s_openapi::api::batch::v1::CronJob;
    use kube::api::Api;

    let cronjobs: Api<CronJob> = Api::namespaced(client.inner().clone(), namespace);
    let cronjob = cronjobs.get(name).await?;

    Ok(cronjob_to_info(cronjob))
}

/// Create a new CronJob
#[cfg(feature = "kubernetes")]
pub async fn create_cronjob(
    client: &K8sClient,
    request: &CreateCronJobRequest,
) -> K8sResult<CronJobInfo> {
    use k8s_openapi::api::batch::v1::{CronJob, CronJobSpec, JobSpec, JobTemplateSpec};
    use k8s_openapi::api::core::v1::{Container, EnvVar, PodSpec, PodTemplateSpec};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let cronjobs: Api<CronJob> = Api::namespaced(client.inner().clone(), &request.namespace);

    // Build environment variables
    let env: Option<Vec<EnvVar>> = if request.env.is_empty() {
        None
    } else {
        Some(
            request
                .env
                .iter()
                .map(|(k, v)| EnvVar {
                    name: k.clone(),
                    value: Some(v.clone()),
                    ..Default::default()
                })
                .collect(),
        )
    };

    // Build container
    let container = Container {
        name: request.name.clone(),
        image: Some(request.image.clone()),
        command: request.command.clone(),
        args: request.args.clone(),
        env,
        ..Default::default()
    };

    // Build pod spec
    let pod_spec = PodSpec {
        containers: vec![container],
        restart_policy: Some(
            request
                .restart_policy
                .clone()
                .unwrap_or_else(|| "OnFailure".to_string()),
        ),
        ..Default::default()
    };

    // Build cronjob
    let cronjob = CronJob {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            namespace: Some(request.namespace.clone()),
            labels: if request.labels.is_empty() {
                None
            } else {
                Some(request.labels.clone())
            },
            ..Default::default()
        },
        spec: Some(CronJobSpec {
            schedule: request.schedule.clone(),
            suspend: Some(request.suspend),
            concurrency_policy: request.concurrency_policy.clone(),
            successful_jobs_history_limit: request.successful_jobs_history_limit,
            failed_jobs_history_limit: request.failed_jobs_history_limit,
            starting_deadline_seconds: request.starting_deadline_seconds,
            job_template: JobTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: if request.labels.is_empty() {
                        None
                    } else {
                        Some(request.labels.clone())
                    },
                    ..Default::default()
                }),
                spec: Some(JobSpec {
                    template: PodTemplateSpec {
                        metadata: Some(ObjectMeta {
                            labels: if request.labels.is_empty() {
                                None
                            } else {
                                Some(request.labels.clone())
                            },
                            ..Default::default()
                        }),
                        spec: Some(pod_spec),
                    },
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    let created = cronjobs.create(&PostParams::default(), &cronjob).await?;
    Ok(cronjob_to_info(created))
}

/// Delete a CronJob
#[cfg(feature = "kubernetes")]
pub async fn delete_cronjob(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::batch::v1::CronJob;
    use kube::api::{Api, DeleteParams};

    let cronjobs: Api<CronJob> = Api::namespaced(client.inner().clone(), namespace);
    cronjobs.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

/// Suspend or resume a CronJob
#[cfg(feature = "kubernetes")]
pub async fn suspend_cronjob(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    suspend: bool,
) -> K8sResult<CronJobInfo> {
    use k8s_openapi::api::batch::v1::CronJob;
    use kube::api::{Api, Patch, PatchParams};

    let cronjobs: Api<CronJob> = Api::namespaced(client.inner().clone(), namespace);

    let patch = serde_json::json!({
        "spec": {
            "suspend": suspend
        }
    });

    let patched = cronjobs
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(cronjob_to_info(patched))
}

/// Trigger a CronJob to run immediately by creating a Job from it
#[cfg(feature = "kubernetes")]
pub async fn trigger_cronjob(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<JobInfo> {
    use k8s_openapi::api::batch::v1::{CronJob, Job};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let cronjobs: Api<CronJob> = Api::namespaced(client.inner().clone(), namespace);
    let jobs: Api<Job> = Api::namespaced(client.inner().clone(), namespace);

    // Get the cronjob
    let cronjob = cronjobs.get(name).await?;
    let spec = cronjob.spec.ok_or_else(|| {
        crate::kubernetes::error::K8sError::Internal("CronJob has no spec".to_string())
    })?;

    // Create a job from the cronjob template
    let job_name = format!("{}-manual-{}", name, chrono::Utc::now().timestamp());
    let job_template = spec.job_template;

    // Build owner reference
    let owner_ref = k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference {
        api_version: "batch/v1".to_string(),
        kind: "CronJob".to_string(),
        name: name.to_string(),
        uid: cronjob.metadata.uid.unwrap_or_default(),
        controller: Some(true),
        block_owner_deletion: Some(true),
    };

    let job = Job {
        metadata: ObjectMeta {
            name: Some(job_name),
            namespace: Some(namespace.to_string()),
            labels: job_template.metadata.as_ref().and_then(|m| m.labels.clone()),
            annotations: Some(
                [("cronjob.kubernetes.io/triggered-manually".to_string(), "true".to_string())]
                    .into_iter()
                    .collect(),
            ),
            owner_references: Some(vec![owner_ref]),
            ..Default::default()
        },
        spec: job_template.spec,
        ..Default::default()
    };

    let created = jobs.create(&PostParams::default(), &job).await?;
    Ok(job_to_info(created))
}

#[cfg(feature = "kubernetes")]
fn cronjob_to_info(cronjob: k8s_openapi::api::batch::v1::CronJob) -> CronJobInfo {
    let metadata = cronjob.metadata;
    let spec = cronjob.spec.unwrap_or_default();
    let status = cronjob.status.unwrap_or_default();

    CronJobInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        schedule: spec.schedule,
        suspend: spec.suspend.unwrap_or(false),
        concurrency_policy: spec.concurrency_policy.unwrap_or_else(|| "Allow".to_string()),
        successful_jobs_history_limit: spec.successful_jobs_history_limit,
        failed_jobs_history_limit: spec.failed_jobs_history_limit,
        active_jobs: status.active.map(|a| a.len() as i32).unwrap_or(0),
        last_schedule_time: status.last_schedule_time.map(|t| t.0.to_rfc3339()),
        last_successful_time: status.last_successful_time.map(|t| t.0.to_rfc3339()),
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// Stubs for when kubernetes feature is disabled
// ============================================================================

#[cfg(not(feature = "kubernetes"))]
pub async fn list_jobs(_client: &K8sClient, _namespace: &str) -> K8sResult<Vec<JobInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_job(_client: &K8sClient, _namespace: &str, _name: &str) -> K8sResult<JobInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_job(_client: &K8sClient, _request: &CreateJobRequest) -> K8sResult<JobInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_job(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
    _propagation_policy: Option<&str>,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_cronjobs(_client: &K8sClient, _namespace: &str) -> K8sResult<Vec<CronJobInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_cronjob(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<CronJobInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_cronjob(
    _client: &K8sClient,
    _request: &CreateCronJobRequest,
) -> K8sResult<CronJobInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_cronjob(_client: &K8sClient, _namespace: &str, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn suspend_cronjob(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
    _suspend: bool,
) -> K8sResult<CronJobInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn trigger_cronjob(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<JobInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
