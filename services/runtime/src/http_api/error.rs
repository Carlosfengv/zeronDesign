use super::ErrorResponse;
use axum::{http::StatusCode, Json};
use serde_json::{json, Value};

pub(super) fn design_profile_service_error(
    error: crate::design_profile_service::DesignProfileServiceError,
) -> (StatusCode, Json<ErrorResponse>) {
    use crate::design_profile_service::DesignProfileServiceError;
    match error {
        DesignProfileServiceError::InvalidRequest(message) => bad_request(message),
        DesignProfileServiceError::NotFound(message) => not_found(message),
        DesignProfileServiceError::Conflict(message)
        | DesignProfileServiceError::ActivationConflict { message, .. } => {
            conflict_error(anyhow::anyhow!(message))
        }
        DesignProfileServiceError::Internal(message) => internal_error(anyhow::anyhow!(message)),
    }
}

pub(super) fn design_profile_activation_error(
    error: crate::design_profile_service::DesignProfileServiceError,
) -> (StatusCode, Json<Value>) {
    if let crate::design_profile_service::DesignProfileServiceError::ActivationConflict {
        message,
        current_version,
        validation_issues,
    } = error
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": message,
                "currentVersion": current_version,
                "validationIssues": validation_issues,
            })),
        );
    }
    error_response_as_value(design_profile_service_error(error))
}

pub(super) fn run_lifecycle_error(
    error: crate::run_lifecycle::RunLifecycleError,
) -> (StatusCode, Json<ErrorResponse>) {
    match error {
        crate::run_lifecycle::RunLifecycleError::InvalidRequest(message) => bad_request(message),
        crate::run_lifecycle::RunLifecycleError::NotFound(message) => not_found(message),
        crate::run_lifecycle::RunLifecycleError::Conflict(message) => {
            conflict_error(anyhow::anyhow!(message))
        }
        crate::run_lifecycle::RunLifecycleError::Internal(message) => {
            internal_error(anyhow::anyhow!(message))
        }
    }
}

pub(super) fn not_found(error: String) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error,
            error_code: None,
        }),
    )
}

pub(super) fn bad_request(error: String) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error,
            error_code: None,
        }),
    )
}

pub(super) fn unauthorized(error: String) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error,
            error_code: None,
        }),
    )
}

pub(super) fn forbidden(error: String) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::FORBIDDEN,
        Json(ErrorResponse {
            error,
            error_code: None,
        }),
    )
}

pub(super) fn error_response_as_value(
    error: (StatusCode, Json<ErrorResponse>),
) -> (StatusCode, Json<Value>) {
    (error.0, Json(json!({ "error": error.1.error })))
}

pub(super) fn design_profile_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let message = error.to_string();
    if message.contains("design profile not found") {
        not_found(message)
    } else if message.contains("invalid design profile") {
        bad_request(message)
    } else {
        conflict_error(anyhow::anyhow!(message))
    }
}

pub(super) fn design_source_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let message = error.to_string();
    if message.contains("design source artifact not found") {
        not_found(message)
    } else if message.contains("invalid design source artifact") {
        bad_request(message)
    } else {
        internal_error(anyhow::anyhow!(message))
    }
}

pub(super) fn conflict_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::CONFLICT,
        Json(ErrorResponse {
            error: error.to_string(),
            error_code: None,
        }),
    )
}

pub(super) fn internal_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: error.to_string(),
            error_code: None,
        }),
    )
}
