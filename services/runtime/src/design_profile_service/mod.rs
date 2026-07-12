mod diff;
mod lifecycle;
mod reports;
mod run_context;

#[cfg(test)]
mod tests;

pub use diff::ProfileDiffChange;
pub use lifecycle::{
    CreateProfileCommand, DesignProfileService, ListProfilesQuery, UpdateProfileCommand,
};
pub use run_context::{PreparedRunProfile, RunProfileContextQuery};

use crate::types::DesignProfileValidationIssue;
use std::{error::Error, fmt};

#[derive(Debug, Clone, PartialEq)]
pub enum DesignProfileServiceError {
    InvalidRequest(String),
    NotFound(String),
    Conflict(String),
    ActivationConflict {
        message: String,
        current_version: u32,
        validation_issues: Vec<DesignProfileValidationIssue>,
    },
    Internal(String),
}

impl fmt::Display for DesignProfileServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::Internal(message) => formatter.write_str(message),
            Self::ActivationConflict { message, .. } => formatter.write_str(message),
        }
    }
}

impl Error for DesignProfileServiceError {}

fn store_error(error: anyhow::Error) -> DesignProfileServiceError {
    let message = error.to_string();
    if message.contains("design profile not found") {
        DesignProfileServiceError::NotFound(message)
    } else if message.contains("invalid design profile") {
        DesignProfileServiceError::InvalidRequest(message)
    } else {
        DesignProfileServiceError::Conflict(message)
    }
}
