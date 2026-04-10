//! `wh library` command anchor — Risk R1 launch-blocker schema template (Story 13-19).
//!
//! This module is the canonical home for the default `.wh-schema.md` template that the
//! wh framework injects at `/workspace/.wh-schema.md` for every new Library. The template
//! is the agent's implicit L5 context layer (relationship to ADR-033) and the artifact
//! that mitigates Risk R1 (the "return moment" UX failure mode where the agent never
//! writes autonomously).
//!
//! ## What this module ships in story 13-19
//!
//! - `DEFAULT_SCHEMA_TEMPLATE`: a `pub const &'static str` embedded at compile time via
//!   `include_str!` from `crates/wh-cli/templates/library/wh-schema.md.tmpl`. This is
//!   the canonical source of truth for the default Library schema.
//! - Structural unit tests asserting the template contains the design elements committed
//!   in the planning draft (`_bmad-output/planning-artifacts/wh/library-default-schema-template.md`):
//!   required sections, FR39 disclaimer, Categories A/B/C/D, the search-when-relevant gate,
//!   and an allow-listed set of `{{...}}` placeholders.
//!
//! ## What this module deliberately does NOT do
//!
//! - It does NOT register a clap subcommand. `wh library init`, `wh library status`,
//!   `wh library ingest`, `wh library list`, `wh library lint` all land in stories
//!   13-22..13-26 and will add their `Cli`/`run` definitions here.
//! - It does NOT perform placeholder substitution. That's owned by 13-22 (`wh library init`).
//! - It does NOT materialize the template to disk or set `chmod 444`. That's owned by
//!   13-20 (schema injection at boot).
//!
//! ## Why `include_str!`
//!
//! The wh binary is shipped as a single self-contained release artifact (story 1.6).
//! Embedding the template at compile time guarantees it travels with every binary and
//! removes any runtime path-resolution failure mode. Updating the template requires a
//! rebuild — that is the intended semantics.

/// Default Library schema template — read-only at runtime, embedded at compile time.
///
/// Path constant downstream stories (13-20, 13-22) will read to obtain the canonical
/// bytes that get materialized at `/workspace/.wh-schema.md` for new Libraries.
///
/// This string contains unresolved `{{agent_name}}` and `{{domain_description}}`
/// placeholders. The consumer (`wh library init` in story 13-22) is responsible for
/// substitution before writing the file to disk.
///
/// Risk: R1 (LAUNCH BLOCKER). Editing this template's *wording* changes the empirical
/// behavior of the agent's autonomous-write loop and re-opens the R1 usability gate.
/// Edit only with deliberate intent and re-run the R1 validation.
pub const DEFAULT_SCHEMA_TEMPLATE: &str = include_str!("../../templates/library/wh-schema.md.tmpl");

#[cfg(test)]
mod tests {
    use super::DEFAULT_SCHEMA_TEMPLATE;
    use regex::Regex;
    use std::collections::BTreeSet;

    /// AC-3: top-level heading is "# Your Library".
    #[test]
    fn test_template_top_heading() {
        let first_non_empty = DEFAULT_SCHEMA_TEMPLATE
            .lines()
            .find(|l| !l.trim().is_empty())
            .expect("template must not be empty");
        assert_eq!(
            first_non_empty, "# Your Library",
            "first non-empty line must be the top heading; got {first_non_empty:?}"
        );
    }

    /// AC-3: all eight required level-2 sections are present.
    #[test]
    fn test_template_required_sections_present() {
        let required_headings = [
            "## What your Library is for",
            "## When to search your Library",
            "## When to write to your Library (important)",
            "## When **not** to write",
            "## How to write a Library page",
            "## Maintaining `index.md`",
            "## Disclaimer (always include in responses derived from Library content)",
            "## Your domain (configurable)",
        ];
        for heading in required_headings {
            assert!(
                DEFAULT_SCHEMA_TEMPLATE.contains(heading),
                "template must contain required heading: {heading}"
            );
        }
    }

    /// AC-4: FR39 disclaimer literal substring is present, plus the
    /// "LLM-generated summaries" framing sentence.
    #[test]
    fn test_template_contains_disclaimer_text() {
        let disclaimer = "This comes from my notes — please verify against the original source if it's critical.";
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains(disclaimer),
            "template must contain the FR39 disclaimer literal"
        );
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("LLM-generated summaries"),
            "template must contain the LLM-generated-summaries framing"
        );
    }

    /// AC-5: all four labeled write-trigger categories present.
    #[test]
    fn test_template_contains_write_categories() {
        let categories = [
            "Category A — Durable facts about the user, their work, or their domain",
            "Category B — Decisions, commitments, and policies the user has established",
            "Category C — Things you figured out on your own that would save future-you time",
            "Category D — Corrections to prior Library content",
        ];
        for cat in categories {
            assert!(
                DEFAULT_SCHEMA_TEMPLATE.contains(cat),
                "template must contain write-trigger category: {cat}"
            );
        }
    }

    /// AC-5: explicit non-trigger section is present.
    #[test]
    fn test_template_has_explicit_non_triggers_section() {
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("## When **not** to write"),
            "template must contain an explicit non-triggers section"
        );
    }

    /// AC-6: required `{{...}}` placeholders are present.
    #[test]
    fn test_template_contains_required_placeholders() {
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("{{agent_name}}"),
            "template must contain {{{{agent_name}}}} placeholder"
        );
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("{{domain_description}}"),
            "template must contain {{{{domain_description}}}} placeholder"
        );
    }

    /// AC-6: every `{{...}}` token in the template is in the allow-list. A future
    /// edit that introduces an undocumented placeholder fails this test, forcing the
    /// edit to also update story 13-22 (the consumer that performs substitution).
    #[test]
    fn test_template_no_undocumented_placeholders() {
        let allow_list: BTreeSet<&str> = ["agent_name", "domain_description"].into_iter().collect();
        let re = Regex::new(r"\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}")
            .expect("placeholder regex must compile");
        let found: BTreeSet<String> = re
            .captures_iter(DEFAULT_SCHEMA_TEMPLATE)
            .map(|c| c[1].to_string())
            .collect();
        for token in &found {
            assert!(
                allow_list.contains(token.as_str()),
                "undocumented placeholder {{{{{token}}}}} found in template — \
                 if this is intentional, update both this allow-list AND \
                 story 13-22 (wh library init substitution logic)"
            );
        }
    }

    /// AC-7: search-when-relevant gate language is present.
    #[test]
    fn test_template_search_gate_language() {
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("At the start of every turn"),
            "template must instruct the agent at the start of every turn"
        );
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("**do not search**"),
            "template must explicitly tell the agent not to search for off-domain queries"
        );
    }
}
