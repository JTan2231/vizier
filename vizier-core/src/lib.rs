pub mod agent;
pub mod agent_prompt;
pub mod auditor;
pub mod config;
pub mod display;
pub mod file_tracking;
pub mod observer;
pub mod scheduler;
pub mod tools;
pub mod tree;
pub mod vcs;
pub mod walker;
pub mod workflow_template;

pub use vizier_kernel::prompts::{
    COMMIT_PROMPT, DOCUMENTATION_PROMPT, IMPLEMENTATION_PLAN_PROMPT, MERGE_CONFLICT_PROMPT,
    REVIEW_PROMPT, SYSTEM_PROMPT_BASE,
};
