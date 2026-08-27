use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};

/// Credentials committed to the repository.
///
/// Motivated by the failure mode that defines this product category: a sweep of 20k+ launched apps
/// found 11% exposing database credentials, service-role keys included.
pub struct CommittedSecrets;

/// Filenames that hold real values rather than placeholders. `.example`/`.sample` variants are the
/// documented convention for committing a template and are explicitly allowed.
const ENV_FILES: &[&str] = &[".env", ".env.local", ".env.production", ".env.prod"];

/// Substrings that identify a live credential rather than a reference to one.
const KEY_MARKERS: &[&str] = &[
    "-----BEGIN RSA PRIVATE KEY-----",
    "-----BEGIN OPENSSH PRIVATE KEY-----",
    "-----BEGIN PRIVATE KEY-----",
];

/// Path fragments that make key material far more likely to be a fixture than a live credential.
///
/// Found by running this check against a real repository: a secret-redaction test necessarily
/// contains something shaped like a secret. Reporting that as critical is the kind of false
/// positive that teaches people to stop reading the report, so it is downgraded rather than
/// suppressed — a real key hidden under `tests/` should still surface.
const FIXTURE_MARKERS: &[&str] = &[
    "/tests/",
    "/test/",
    "_test.",
    ".test.",
    "test_",
    "/fixtures/",
    "/testdata/",
    "/__tests__/",
];

impl Check for CommittedSecrets {
    fn id(&self) -> &'static str {
        "security/committed-secrets"
    }

    fn dimension(&self) -> Dimension {
        Dimension::Security
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        let mut findings = Vec::new();

        for name in ENV_FILES {
            if ctx.has(name) {
                findings.push(
                    Finding::new(
                        self.id(),
                        self.dimension(),
                        Severity::Critical,
                        format!("`{name}` is committed to the repository"),
                        "Environment files hold live credentials. Anything committed here is in \
                         the git history permanently, and is readable by every agent, CI job and \
                         collaborator with repo access.",
                        Fix::Assisted {
                            description: format!(
                                "Move the values to `wrangler secret` (per environment), add \
                                 `{name}` to .gitignore, commit a `{name}.example` with empty \
                                 placeholders, and rotate every credential it contained."
                            ),
                        },
                    )
                    .at(*name),
                );
            }
        }

        for path in ctx.files() {
            // Cheap guard: only read files small enough to plausibly be a key.
            let Some(contents) = ctx.read(path.as_str()) else {
                continue;
            };
            if contents.len() > 64_000 {
                continue;
            }
            if KEY_MARKERS.iter().any(|m| contents.contains(m)) {
                let looks_like_fixture = FIXTURE_MARKERS.iter().any(|m| path.as_str().contains(m));

                let (severity, detail, fix) = if looks_like_fixture {
                    (
                        Severity::Low,
                        "This looks like test fixture data rather than a live credential, so it is \
                         reported quietly. Confirm it is synthetic.",
                        Fix::Manual {
                            description: "Confirm the key is synthetic. If it was ever real, \
                                          rotate it and purge it from git history."
                                .to_string(),
                        },
                    )
                } else {
                    (
                        Severity::Critical,
                        "A private key in version control must be treated as compromised.",
                        Fix::Manual {
                            description: "Rotate the key, then purge it from git history."
                                .to_string(),
                        },
                    )
                };

                findings.push(
                    Finding::new(
                        self.id(),
                        self.dimension(),
                        severity,
                        format!("Private key material committed at `{path}`"),
                        detail,
                        fix,
                    )
                    .at(path.to_owned()),
                );
            }
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    #[test]
    fn flags_committed_env_file() {
        let (_dir, ctx) = fixture(&[(".env", "API_KEY=live_abc123")]);
        assert_eq!(CommittedSecrets.run(&ctx).len(), 1);
    }

    #[test]
    fn allows_example_env_file() {
        let (_dir, ctx) = fixture(&[(".env.example", "API_KEY=")]);
        assert!(CommittedSecrets.run(&ctx).is_empty());
    }

    #[test]
    fn key_material_in_a_test_fixture_is_downgraded_not_suppressed() {
        let (_dir, ctx) = fixture(&[(
            "backend/tests/test_redaction.py",
            "SAMPLE = \"-----BEGIN RSA PRIVATE KEY-----\"",
        )]);
        let findings = CommittedSecrets.run(&ctx);
        assert_eq!(
            findings.len(),
            1,
            "a real key hidden under tests/ must still surface"
        );
        assert_eq!(findings[0].severity, Severity::Low);
    }

    #[test]
    fn flags_private_key_material() {
        let (_dir, ctx) = fixture(&[("deploy/id_rsa", "-----BEGIN RSA PRIVATE KEY-----\nx\n")]);
        let findings = CommittedSecrets.run(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Critical);
    }
}
