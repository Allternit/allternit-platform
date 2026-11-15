//! Model download pipeline (verified through `allternit-model-source`).

pub mod huggingface;
pub mod task;

pub use huggingface::{
    is_weight_file, select_files, should_download, validate_repo_id, HuggingFaceError,
};
pub use task::{
    build_cached_model, check_local_files, spawn_download_task, verify_local, FileCheck,
    VerifyReport,
};
