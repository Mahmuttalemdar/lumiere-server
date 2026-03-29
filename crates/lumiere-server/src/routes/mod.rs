pub mod attachments;
pub mod auth;
pub mod channels;
pub mod messages;
pub mod moderation;
pub mod reactions;
pub mod roles;
pub mod servers;
pub mod typing;
pub mod users;
pub mod webhooks;

use lumiere_models::error::FieldError;

/// Convert validator errors into our FieldError vec
pub fn validation_errors(errors: validator::ValidationErrors) -> Vec<FieldError> {
    errors
        .field_errors()
        .into_iter()
        .flat_map(|(field, errs)| {
            errs.iter().map(move |e| FieldError {
                field: field.to_string(),
                message: e
                    .message
                    .as_ref()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| format!("Invalid {}", field)),
            })
        })
        .collect()
}
