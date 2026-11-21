use crate::{AppError, Result, fail};
use ort::{
    environment::Environment,
    session::{
        Session,
        builder::{AutoDevicePolicy, SessionBuilder},
    },
};
use std::path::Path;

#[path = "provider_profile.rs"]
pub mod profiles;

pub(crate) fn cpu(builder: SessionBuilder) -> Result<SessionBuilder> {
    // Override ort's default MaxEfficiency policy for a CPU-labeled session.
    builder
        .with_auto_device(AutoDevicePolicy::PreferCPU)
        .map_err(|e| AppError::Provider(e.to_string()))
}

pub fn session(
    env: &Environment,
    path: &Path,
    requested: &str,
    threads: usize,
    required: bool,
) -> Result<(Session, String)> {
    session_profiled(env, path, requested, threads, required, None)
}

pub(crate) fn session_profiled(
    env: &Environment,
    path: &Path,
    requested: &str,
    threads: usize,
    required: bool,
    profile: Option<&Path>,
) -> Result<(Session, String)> {
    let selected = if requested == "auto" {
        if cfg!(feature = "cuda") {
            "cuda"
        } else if cfg!(feature = "directml") {
            "directml"
        } else {
            "cpu"
        }
    } else {
        requested
    };
    if required && selected == "cpu" {
        return Err(AppError::Provider(
            "Required acceleration unavailable; this package selects CPU".into(),
        ));
    }
    let attempt = build(env, path, selected, threads, required, profile);
    match attempt {
        Ok(s) => Ok((s, selected.into())),
        Err(e) => match fallback_warning(requested, selected, required, &e.to_string())? {
            Some(warning) => {
                eprintln!("{warning}");
                Ok((
                    build(env, path, "cpu", threads, false, profile)?,
                    "cpu".into(),
                ))
            }
            None => Err(e),
        },
    }
}

pub fn is_oom_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "out of memory",
        "cudaerrormemoryallocation",
        "e_outofmemory",
        "failed to allocate memory",
        "bfc_arena",
        "alloc_failed",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

pub fn fallback_warning(
    requested: &str,
    selected: &str,
    required: bool,
    error: &str,
) -> Result<Option<String>> {
    if required && is_oom_error(error) {
        return Err(AppError::Provider(format!(
            "Required GPU out of memory on {selected}; refusing silent CPU fallback ({error})"
        )));
    }
    if requested == "auto" && !required && selected != "cpu" {
        let kind = if is_oom_error(error) {
            "gpu_oom_fallback"
        } else {
            "gpu_unavailable_fallback"
        };
        Ok(Some(format!(
            "WARNING: event={kind} provider={selected} reason={error}; using CPU"
        )))
    } else {
        Ok(None)
    }
}

fn build(
    env: &Environment,
    path: &Path,
    provider: &str,
    threads: usize,
    required: bool,
    profile: Option<&Path>,
) -> Result<Session> {
    let mut builder = Session::builder(env)
        .map_err(|e| fail(e.to_string()))?
        .with_intra_threads(threads)
        .map_err(|e| fail(e.to_string()))?;
    match provider {
        "cpu" => builder = cpu(builder)?,
        #[cfg(feature = "cuda")]
        "cuda" => {
            builder = builder
                .with_execution_providers([ort::ep::CUDA::default().build().error_on_failure()])
                .map_err(|e| AppError::Provider(e.to_string()))?;
        }
        #[cfg(feature = "directml")]
        "directml" => {
            builder = builder
                .with_parallel_execution(false)
                .map_err(|e| fail(e.to_string()))?
                .with_memory_pattern(false)
                .map_err(|e| fail(e.to_string()))?
                .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
                .map_err(|e| AppError::Provider(e.to_string()))?;
        }
        _ => {
            return Err(AppError::Provider(format!(
                "{provider} was not enabled for this native runtime package"
            )));
        }
    }
    if required {
        // Not `with_disable_cpu_fallback`: ORT always forces small shape subgraphs onto CPU,
        // which that switch rejects outright. Record placement and enforce our own policy.
        builder = builder
            .with_config_entry("session.record_ep_graph_assignment_info", "1")
            .map_err(|e| fail(e.to_string()))?;
    }
    if let Some(path) = profile {
        builder = builder
            .with_profiling(path)
            .map_err(|e| fail(e.to_string()))?;
    }
    let session = builder
        .commit_from_file(path)
        .map_err(|e| AppError::Provider(e.to_string()))?;
    if required {
        require_accelerated(&assignment(&session)?)?;
    }
    Ok(session)
}

/// Shape-computation operators that ORT's `fallback_cpu_capability` heuristic places on CPU
/// even when the accelerator supports them. Required acceleration admits only these on CPU.
pub const CPU_SHAPE_OPS: &[&str] = &[
    "Shape",
    "Size",
    "Slice",
    "Concat",
    "Gather",
    "Unsqueeze",
    "Squeeze",
    "Reshape",
    "Cast",
    "Range",
    "ConstantOfShape",
];

/// Admitted CPU placement under required acceleration: transfers or shape-only operators.
pub fn cpu_admitted(operation: &str) -> bool {
    operation.starts_with("Memcpy") || CPU_SHAPE_OPS.contains(&operation)
}

/// `assigned` holds (execution provider, operator type) per node. Requires accelerated compute
/// and rejects every CPU compute node outside the shape-operator allowlist.
pub(crate) fn require_accelerated(assigned: &[(String, String)]) -> Result<()> {
    let accelerated = assigned
        .iter()
        .filter(|(ep, op)| ep != "CPUExecutionProvider" && !op.starts_with("Memcpy"))
        .count();
    let rejected: std::collections::BTreeSet<&str> = assigned
        .iter()
        .filter(|(ep, op)| ep == "CPUExecutionProvider" && !cpu_admitted(op))
        .map(|(_, op)| op.as_str())
        .collect();
    if accelerated == 0 || !rejected.is_empty() {
        let names: Vec<&str> = rejected.into_iter().take(8).collect();
        return Err(AppError::Provider(format!(
            "Required acceleration not met: {accelerated} accelerated nodes; CPU compute operators [{}]",
            names.join(", ")
        )));
    }
    Ok(())
}

/// Reads ORT's recorded node-to-provider assignment (ORT >= 1.24, enabled per session above).
fn assignment(session: &Session) -> Result<Vec<(String, String)>> {
    use ort::{AsPointer, sys::OrtStatusPtr};
    use std::ffi::{CStr, c_char};
    let api = ort::api();
    let check = |status: OrtStatusPtr| -> Result<()> {
        if status.0.is_null() {
            return Ok(());
        }
        // SAFETY: non-null status returned by ORT; the message is copied before release.
        let message = unsafe { CStr::from_ptr((api.GetErrorMessage)(status.0)) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: the status is owned by the caller and released exactly once.
        unsafe { (api.ReleaseStatus)(status.0) };
        Err(AppError::Provider(format!(
            "Execution provider assignment unavailable: {message}"
        )))
    };
    let text = |value: *const c_char| -> String {
        if value.is_null() {
            return String::new();
        }
        // SAFETY: ORT returns null-terminated UTF-8 owned by the live session info.
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    };
    let mut subgraphs = std::ptr::null();
    let mut count = 0usize;
    // SAFETY: the session pointer is live for this borrow; outputs are valid locals.
    check(unsafe {
        (api.Session_GetEpGraphAssignmentInfo)(session.ptr(), &mut subgraphs, &mut count)
    })?;
    let mut assigned = Vec::new();
    for index in 0..count {
        // SAFETY: ORT returned `count` subgraph pointers owned by the session.
        let subgraph = unsafe { *subgraphs.add(index) };
        let mut ep = std::ptr::null();
        // SAFETY: valid subgraph from ORT; output is a local.
        check(unsafe { (api.EpAssignedSubgraph_GetEpName)(subgraph, &mut ep) })?;
        let ep = text(ep);
        let (mut nodes, mut node_count) = (std::ptr::null(), 0usize);
        // SAFETY: valid subgraph from ORT; outputs are locals.
        check(unsafe { (api.EpAssignedSubgraph_GetNodes)(subgraph, &mut nodes, &mut node_count) })?;
        for node_index in 0..node_count {
            // SAFETY: ORT returned `node_count` node pointers owned by the subgraph.
            let node = unsafe { *nodes.add(node_index) };
            let mut op = std::ptr::null();
            // SAFETY: valid node from ORT; output is a local.
            check(unsafe { (api.EpAssignedNode_GetOperatorType)(node, &mut op) })?;
            assigned.push((ep.clone(), text(op)));
        }
    }
    Ok(assigned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(ep, op)| ((*ep).into(), (*op).into()))
            .collect()
    }

    #[test]
    fn required_acceleration_admits_only_cpu_shape_operators() {
        let cuda = "CUDAExecutionProvider";
        let cpu = "CPUExecutionProvider";
        assert!(
            require_accelerated(&nodes(&[(cuda, "Conv"), (cpu, "Slice"), (cpu, "Concat")])).is_ok()
        );
        assert!(require_accelerated(&nodes(&[(cuda, "Conv"), (cuda, "MemcpyFromHost")])).is_ok());
        let error = require_accelerated(&nodes(&[(cuda, "Conv"), (cpu, "Slice"), (cpu, "MatMul")]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("MatMul") && !error.contains("Slice"));
        assert!(require_accelerated(&nodes(&[(cpu, "Shape"), (cuda, "MemcpyToHost")])).is_err());
        assert!(require_accelerated(&[]).is_err());
    }

    #[test]
    fn oom_classification_and_bounded_fallback_policy_fail_closed_when_required() {
        assert!(is_oom_error(
            "CUDAExecutionProvider: cudaErrorMemoryAllocation"
        ));
        assert!(is_oom_error("DirectML E_OUTOFMEMORY while creating tensor"));
        assert!(is_oom_error("BFC_ARENA failed to allocate memory"));
        assert!(!is_oom_error("Missing shared library libcudart.so"));

        let oom_warn = fallback_warning("auto", "cuda", false, "out of memory")
            .unwrap()
            .unwrap();
        assert!(oom_warn.contains("event=gpu_oom_fallback"));
        let unavail_warn = fallback_warning("auto", "directml", false, "driver unavailable")
            .unwrap()
            .unwrap();
        assert!(unavail_warn.contains("event=gpu_unavailable_fallback"));
        assert!(
            fallback_warning("cuda", "cuda", false, "out of memory")
                .unwrap()
                .is_none()
        );
        assert!(
            fallback_warning("auto", "cpu", false, "out of memory")
                .unwrap()
                .is_none()
        );

        let err = fallback_warning("auto", "cuda", true, "CUDA out of memory")
            .unwrap_err()
            .to_string();
        assert!(err.contains("GPU out of memory") && err.contains("refusing silent CPU fallback"));
        assert!(
            fallback_warning("cuda", "cuda", true, "driver missing")
                .unwrap()
                .is_none()
        );
    }
}
