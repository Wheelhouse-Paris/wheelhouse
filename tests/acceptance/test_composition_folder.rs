//! Acceptance tests for story 14-3-2: `subsystems/llm-wiki/` Composition Folder.
//!
//! These tests validate that the composition folder ships with all required files
//! and that the content meets the acceptance criteria (ADR-045, ADR-048).

use std::path::PathBuf;

/// Resolve the `subsystems/llm-wiki/` folder relative to the workspace root.
///
/// Uses `CARGO_MANIFEST_DIR` to locate the workspace root, walking up from
/// the crate directory to the repository root.
fn subsystem_folder() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // CARGO_MANIFEST_DIR for integration tests under tests/ is the workspace root
    manifest_dir.join("subsystems").join("llm-wiki")
}

// ---------------------------------------------------------------------------
// AC #1: Folder contains all required files
// ---------------------------------------------------------------------------

/// Given the framework repository
/// When an operator inspects `subsystems/llm-wiki/`
/// Then the folder contains all required files.
#[test]
fn ac1_composition_folder_contains_all_required_files() {
    let folder = subsystem_folder();
    assert!(folder.exists(), "subsystems/llm-wiki/ should exist");

    let required_files = [
        "composition.wh",
        "librarian-persona.md",
        "prompts/decision_en.md",
        "prompts/decision_fr.md",
        "schema-template-reader.md",
        "README.md",
    ];

    for file in &required_files {
        let path = folder.join(file);
        assert!(
            path.exists(),
            "required file '{}' should exist in subsystems/llm-wiki/",
            file
        );
        let content =
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("should read {file}"));
        assert!(
            !content.is_empty(),
            "required file '{}' should not be empty",
            file
        );
    }
}

// ---------------------------------------------------------------------------
// AC #1 (subset): schema-template-reader.md is reader-only
// ---------------------------------------------------------------------------

/// Given the schema-template-reader.md file
/// When its content is inspected
/// Then it contains read-only instructions and NO write instructions.
#[test]
fn ac1_schema_template_reader_is_read_only() {
    let schema_path = subsystem_folder().join("schema-template-reader.md");
    let content = std::fs::read_to_string(&schema_path).expect("should read schema template");

    // Must contain the read-only declaration
    assert!(
        content.contains("You CANNOT write to the Library"),
        "schema should contain read-only declaration"
    );

    // Must contain citation guidance
    assert!(
        content.contains("cite") || content.contains("citation") || content.contains("Cite"),
        "schema should contain citation guidance"
    );

    // Must NOT contain write instructions from the writer schema
    let forbidden_write_phrases = [
        "When writing",
        "Commit after every write",
        "Maintain index.md",
        "When to write to your Library",
    ];
    for phrase in &forbidden_write_phrases {
        assert!(
            !content.contains(phrase),
            "reader schema must NOT contain write instruction: '{phrase}'"
        );
    }
}

// ---------------------------------------------------------------------------
// AC #1 (subset): composition.wh documents template variables
// ---------------------------------------------------------------------------

/// Given the composition.wh file
/// When its content is inspected
/// Then it documents the template variables used by the composition loader.
#[test]
fn ac1_composition_wh_documents_template_variables() {
    let composition_path = subsystem_folder().join("composition.wh");
    let content = std::fs::read_to_string(&composition_path).expect("should read composition.wh");

    let required_variables = [
        "{{library_name}}",
        "{{topology_name}}",
        "{{members}}",
        "{{locales}}",
    ];
    for var in &required_variables {
        assert!(
            content.contains(var),
            "composition.wh should document template variable: {var}"
        );
    }
}

// ---------------------------------------------------------------------------
// AC #1 (subset): librarian-persona.md describes operational context
// ---------------------------------------------------------------------------

/// Given the librarian-persona.md file
/// When its content is inspected
/// Then it describes the librarian's role, decision framework, and monitoring.
#[test]
fn ac1_librarian_persona_describes_operational_context() {
    let persona_path = subsystem_folder().join("librarian-persona.md");
    let content = std::fs::read_to_string(&persona_path).expect("should read librarian-persona.md");

    // Must describe the librarian's role
    assert!(
        content.contains("Librarian") || content.contains("librarian"),
        "persona should mention the librarian"
    );

    // Must include decision reason codes
    assert!(
        content.contains("durable_fact_written"),
        "persona should reference V1 reason codes"
    );

    // Must include monitoring guidance
    assert!(
        content.contains("log") || content.contains("Log") || content.contains("monitoring"),
        "persona should include monitoring guidance"
    );
}

// ---------------------------------------------------------------------------
// AC #1 (subset): README.md contains deployment walkthrough
// ---------------------------------------------------------------------------

/// Given the README.md file
/// When its content is inspected
/// Then it contains a deployment walkthrough with example topology.
#[test]
fn ac1_readme_contains_deployment_walkthrough() {
    let readme_path = subsystem_folder().join("README.md");
    let content = std::fs::read_to_string(&readme_path).expect("should read README.md");

    // Must contain deploy commands
    assert!(
        content.contains("wh deploy plan"),
        "README should reference wh deploy plan"
    );
    assert!(
        content.contains("wh deploy apply"),
        "README should reference wh deploy apply"
    );

    // Must contain example topology
    assert!(
        content.contains("subsystems:"),
        "README should contain example subsystem declaration"
    );
    assert!(
        content.contains("subsystems/llm-wiki/"),
        "README should reference the subsystem path"
    );

    // Must document parameters
    assert!(
        content.contains("library_name"),
        "README should document library_name parameter"
    );
    assert!(
        content.contains("members"),
        "README should document members parameter"
    );
}

// ---------------------------------------------------------------------------
// AC #1 (subset): Decision prompts contain V1 reason codes
// ---------------------------------------------------------------------------

/// Given the decision prompt files
/// When their content is inspected
/// Then they contain the V1 reason codes.
#[test]
fn ac1_decision_prompts_contain_v1_reason_codes() {
    let folder = subsystem_folder();

    for (locale, path) in [
        ("en", "prompts/decision_en.md"),
        ("fr", "prompts/decision_fr.md"),
    ] {
        let prompt_path = folder.join(path);
        let content =
            std::fs::read_to_string(&prompt_path).unwrap_or_else(|_| panic!("should read {path}"));

        let required_codes = [
            "durable_fact_written",
            "update_existing_page",
            "dedup_merged",
            "transient_context",
            "pii_not_durable",
            "contains_secret_pattern",
        ];
        for code in &required_codes {
            assert!(
                content.contains(code),
                "{locale} decision prompt should contain reason code: {code}"
            );
        }
    }
}
