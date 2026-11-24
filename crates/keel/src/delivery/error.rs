//! Thin routes translate typed delivery failures without guessing HTTP meaning from message text.
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// Domain failures preserve the distinction between bad intent, stale state and unconfirmed I/O.
#[derive(Clone, Debug)]
pub(crate) enum Failure {
    Invalid(String),
    Conflict(String),
    Missing(String),
    Unavailable(String),
}
impl Failure {
    /// API clients retain receipts on unavailable outcomes instead of retrying a new operation.
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Invalid(s) | Self::Conflict(s) | Self::Missing(s) | Self::Unavailable(s) => s,
        }
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Missing(_) => StatusCode::NOT_FOUND,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        };
        (status, self.message().to_string()).into_response()
    }
}
impl From<crate::execution::ReserveError> for Failure {
    fn from(error: crate::execution::ReserveError) -> Self {
        match error {
            crate::execution::ReserveError::Invalid => {
                Self::Invalid("Invalid execution receipt ID.".into())
            }
            crate::execution::ReserveError::Conflict => {
                Self::Conflict("This receipt was already used for different inputs.".into())
            }
            crate::execution::ReserveError::Storage(message) => Self::Unavailable(message),
        }
    }
}
