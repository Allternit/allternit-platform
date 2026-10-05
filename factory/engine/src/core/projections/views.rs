use std::path::PathBuf;

use anyhow::Result;

use crate::core::io::{ensure_dir, write_json_atomic};
use crate::prompt::PromptTimeline;
use crate::wih::types::WihState;
use crate::work::types::DagState;

pub fn write_dag_view(root_dir: &PathBuf, dag: &DagState) -> Result<PathBuf> {
    let base = root_dir
        .join(".allternit/work/dags")
        .join(&dag.dag_id)
        .join("view");
    ensure_dir(&base)?;
    let path = base.join("dag.current.json");
    write_json_atomic(&path, dag)?;
    Ok(path)
}

pub fn write_wih_view(root_dir: &PathBuf, dag_id: &str, wih: &WihState) -> Result<PathBuf> {
    let status_dir = if wih.status == "CLOSED" {
        "closed"
    } else {
        "open"
    };
    let base = root_dir
        .join(".allternit/work/dags")
        .join(dag_id)
        .join("wih")
        .join(status_dir);
    ensure_dir(&base)?;
    let path = base.join(format!("{}.json", wih.wih_id));
    write_json_atomic(&path, wih)?;
    Ok(path)
}

pub fn write_prompt_view(root_dir: &PathBuf, prompt: &PromptTimeline) -> Result<PathBuf> {
    let base = root_dir.join(".allternit/ledger/prompt/view");
    ensure_dir(&base)?;
    let path = base.join(format!("{}.current.json", prompt.prompt_id));
    write_json_atomic(&path, prompt)?;
    Ok(path)
}

pub fn write_elicitation_view(root_dir: &PathBuf, elicitation: &crate::core::types::ElicitationRequest) -> Result<PathBuf> {
    let base = root_dir.join(".allternit/ledger/elicitation/active");
    ensure_dir(&base)?;
    let path = base.join(format!("{}.json", elicitation.elicitation_id));
    write_json_atomic(&path, elicitation)?;
    Ok(path)
}

pub fn write_sampling_view(root_dir: &PathBuf, sampling: &crate::core::types::SamplingRequest) -> Result<PathBuf> {
    let base = root_dir.join(".allternit/ledger/sampling/active");
    ensure_dir(&base)?;
    let path = base.join(format!("{}.json", sampling.sampling_id));
    write_json_atomic(&path, sampling)?;
    Ok(path)
}
