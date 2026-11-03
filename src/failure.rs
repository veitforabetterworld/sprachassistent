//! Errors with a code. The admin area shows a sentence of its own for each code (apps/web,
//! `errors.ts`); the detail is technical and goes to the log and, where it helps, along with the code.

use std::fmt;

#[derive(Debug)]
pub struct Failure {
    pub code: &'static str,
    pub detail: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.detail.is_empty() {
            f.write_str(self.code)
        } else {
            f.write_str(&self.detail)
        }
    }
}

impl std::error::Error for Failure {}

pub fn failure(code: &'static str, detail: impl Into<String>) -> anyhow::Error {
    Failure {
        code,
        detail: detail.into(),
    }
    .into()
}

/// The code anywhere in the chain, also below added context.
pub fn code_of(e: &anyhow::Error) -> Option<&'static str> {
    e.chain()
        .find_map(|cause| cause.downcast_ref::<Failure>())
        .map(|f| f.code)
}

/// The code for an HTTP status from a model provider.
pub fn llm_status_code(status: u16) -> &'static str {
    match status {
        401 | 403 => "llm.unauthorized",
        404 => "llm.model_not_found",
        413 => "llm.too_large",
        429 => "llm.at_limit",
        400 | 422 => "llm.bad_request",
        500..=599 => "llm.unavailable",
        _ => "llm.failed",
    }
}

/// One code for the failures of all providers that were tried. A real error such as a bad key wins
/// over a limit, so it does not hide behind "try again later".
pub fn llm_summary(codes: &[&'static str]) -> &'static str {
    const ORDER: [&str; 8] = [
        "llm.unauthorized",
        "llm.model_not_found",
        "llm.bad_request",
        "llm.too_large",
        "llm.unreachable",
        "llm.empty_reply",
        "llm.failed",
        "llm.timeout",
    ];
    if codes.is_empty() {
        return "llm.not_configured";
    }
    if codes.iter().all(|c| is_transient(c)) {
        return if codes.contains(&"llm.at_limit") {
            "llm.at_limit"
        } else if codes.contains(&"llm.timeout") {
            "llm.timeout"
        } else {
            "llm.unavailable"
        };
    }
    ORDER
        .into_iter()
        .find(|code| codes.contains(code))
        .unwrap_or("llm.failed")
}

/// Limit, overload or no answer in time: worth trying again a little later.
pub fn is_transient(code: &str) -> bool {
    matches!(
        code,
        "llm.at_limit" | "llm.too_large" | "llm.unavailable" | "llm.timeout"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_survives_context() {
        let e = failure("llm.timeout", "keine Antwort nach 10 s").context("Groq");
        assert_eq!(code_of(&e), Some("llm.timeout"));
        assert_eq!(code_of(&anyhow::anyhow!("x")), None);
    }

    #[test]
    fn a_real_error_wins_over_a_limit() {
        assert_eq!(
            llm_summary(&["llm.at_limit", "llm.unavailable"]),
            "llm.at_limit"
        );
        assert_eq!(
            llm_summary(&["llm.at_limit", "llm.unauthorized"]),
            "llm.unauthorized"
        );
        assert_eq!(llm_summary(&[]), "llm.not_configured");
        assert_eq!(llm_status_code(503), "llm.unavailable");
    }
}
