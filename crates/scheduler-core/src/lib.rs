pub mod branch;
pub mod error;
pub mod job;
pub mod run;
pub mod schedule;
pub mod validation;

pub use branch::{BranchTemplateContext, render_branch_template};
pub use error::{CoreError, ValidationError};
pub use job::*;
pub use run::*;
pub use schedule::*;
pub use validation::{ValidationContext, validate_job_spec};

pub fn job_spec_json_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(job::JobSpec)
}
