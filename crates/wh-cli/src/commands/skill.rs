//! `wh skill` — manage skills in the topology's skills repository.

use clap::Subcommand;

/// Skill management subcommands.
#[derive(Debug, Subcommand)]
pub enum SkillCommand {
    /// Initialize a new skill in the skills repository.
    Create {
        /// Name of the skill to create (e.g., "summarize", "research").
        name: String,
        /// Path to the topology folder (defaults to current directory).
        #[arg(long, default_value = ".")]
        path: String,
    },
}

/// Execute a skill subcommand.
pub fn run(command: SkillCommand) -> i32 {
    match command {
        SkillCommand::Create { name, path } => run_create(&name, &path),
    }
}

fn run_create(name: &str, topology_path: &str) -> i32 {
    // Load topology to find skills_repo
    let path = std::path::Path::new(topology_path);
    let topology = match wh_broker::deploy::lint::lint(path) {
        Ok(linted) => linted.topology().clone(),
        Err(e) => {
            eprintln!("Error: cannot read topology: {e}");
            return 1;
        }
    };

    let skills_repo = match &topology.skills_repo {
        Some(repo) => repo.clone(),
        None => {
            eprintln!("Error: no skills_repo defined in topology");
            return 1;
        }
    };

    // Create the skill directory structure in the repo
    let repo_path = std::path::Path::new(&skills_repo);
    if !repo_path.is_dir() {
        eprintln!("Error: skills_repo path does not exist: {skills_repo}");
        return 1;
    }

    let skill_dir = repo_path.join(name);
    if skill_dir.exists() {
        eprintln!(
            "Error: skill '{name}' already exists at {}",
            skill_dir.display()
        );
        return 1;
    }

    // Create skill directory and canonical files
    let steps_dir = skill_dir.join("steps");
    if let Err(e) = std::fs::create_dir_all(&steps_dir) {
        eprintln!("Error: cannot create skill directory: {e}");
        return 1;
    }

    let skill_md = format!(
        "---\nname: {name}\ndescription: \"\"\ntags: []\n---\n\n# {name}\n\nDescribe what this skill does.\n"
    );
    if let Err(e) = std::fs::write(skill_dir.join("skill.md"), &skill_md) {
        eprintln!("Error: cannot write skill.md: {e}");
        return 1;
    }

    let step_md = format!(
        "---\nstep: 1\ntitle: Default step\n---\n\nDescribe the first step of the {name} skill.\n"
    );
    if let Err(e) = std::fs::write(steps_dir.join("01-default.md"), &step_md) {
        eprintln!("Error: cannot write step file: {e}");
        return 1;
    }

    // Git add and commit
    let git_result = std::process::Command::new("git")
        .args(["add", name])
        .current_dir(repo_path)
        .output();

    if let Ok(out) = git_result {
        if out.status.success() {
            let _ = std::process::Command::new("git")
                .args(["commit", "-m", &format!("skill: create {name}")])
                .current_dir(repo_path)
                .output();
        }
    }

    eprintln!("Skill '{name}' created at {}", skill_dir.display());
    eprintln!("Run `wh topology apply` to refresh the skills volume.");
    0
}
