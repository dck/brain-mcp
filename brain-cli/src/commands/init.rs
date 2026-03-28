use std::path::PathBuf;

use dialoguer::{Confirm, Input, MultiSelect, Select};

use brain_core::config::{Config, EmbeddingConfig, IndexConfig, ServerConfig, VaultConfig};

use super::{config_dir, default_config_path};
use crate::output;

const ALL_CATEGORIES: &[&str] = &[
    "procedures",
    "decisions",
    "learnings",
    "concepts",
    "projects",
];

const PROCEDURE_TEMPLATE: &str = r#"---
title: "{{title}}"
id: "{{id}}"
tags:
{{tags}}
created_at: "{{created_at}}"
category: {{category}}
---

## Context

{{content}}

## Steps

1.

## Notes

"#;

const DECISION_TEMPLATE: &str = r#"---
title: "{{title}}"
id: "{{id}}"
tags:
{{tags}}
created_at: "{{created_at}}"
category: {{category}}
---

## Context

{{content}}

## Options Considered

1.

## Decision

## Consequences

"#;

const LEARNING_TEMPLATE: &str = r#"---
title: "{{title}}"
id: "{{id}}"
tags:
{{tags}}
created_at: "{{created_at}}"
category: {{category}}
---

## What I Learned

{{content}}

## Why It Matters

## References

"#;

const CONCEPT_TEMPLATE: &str = r#"---
title: "{{title}}"
id: "{{id}}"
tags:
{{tags}}
created_at: "{{created_at}}"
category: {{category}}
---

## Definition

{{content}}

## Examples

## Related Concepts

"#;

pub async fn run(json_output: bool) -> anyhow::Result<()> {
    if json_output {
        // Non-interactive mode is not supported with --json; just error.
        anyhow::bail!("init requires interactive input and cannot be used with --json");
    }

    println!();
    println!(
        "  {} {}",
        console::style("brain-mcp").bold(),
        console::style("setup wizard").dim()
    );
    println!();

    // 1. Vault path
    let vault_path: String = Input::new()
        .with_prompt("  Vault path")
        .default("~/brain".into())
        .interact_text()?;

    // 2. Categories
    let defaults = vec![true; ALL_CATEGORIES.len()];
    let chosen = MultiSelect::new()
        .with_prompt("  Categories (space to toggle)")
        .items(ALL_CATEGORIES)
        .defaults(&defaults)
        .interact()?;
    let categories: Vec<String> = chosen
        .iter()
        .map(|&i| ALL_CATEGORIES[i].to_string())
        .collect();

    // 3. Embedding provider
    let providers = &[
        "OpenAI",
        "Voyage (not yet supported)",
        "Local ONNX (not yet supported)",
    ];
    let provider_idx = Select::new()
        .with_prompt("  Embedding provider")
        .items(providers)
        .default(0)
        .interact()?;

    let (provider, model, api_key_env, model_path) = match provider_idx {
        0 => {
            let model: String = Input::new()
                .with_prompt("  OpenAI model")
                .default("text-embedding-3-small".into())
                .interact_text()?;
            let env_var: String = Input::new()
                .with_prompt("  API key env var")
                .default("OPENAI_API_KEY".into())
                .interact_text()?;
            ("openai".to_string(), model, Some(env_var), None)
        }
        _ => {
            eprintln!("  Only OpenAI is supported at this time.");
            std::process::exit(1);
        }
    };

    // 4. HTTP port
    let http_port: u16 = Input::new()
        .with_prompt("  HTTP port")
        .default(47200)
        .interact_text()?;

    // 5. Grace period
    let grace_period: u64 = Input::new()
        .with_prompt("  Grace period (seconds)")
        .default(60)
        .interact_text()?;

    // Build config
    let config = Config {
        vault: VaultConfig {
            path: vault_path.clone(),
            templates_dir: "_templates".into(),
            categories: categories.clone(),
        },
        embedding: EmbeddingConfig {
            provider,
            model,
            api_key_env,
            model_path,
        },
        index: IndexConfig {
            backend: "sqlite-vec".into(),
            path: "~/.config/brain-mcp/index.db".into(),
        },
        server: ServerConfig {
            http_port,
            grace_period_seconds: grace_period,
        },
    };

    // Write config
    let config_dir = config_dir();
    std::fs::create_dir_all(&config_dir)?;
    let config_path = default_config_path();
    if config_path.exists() {
        let overwrite = Confirm::new()
            .with_prompt("  Config already exists. Overwrite?")
            .default(false)
            .interact()?;
        if !overwrite {
            println!("{}", output::info_line("Skipped", "config file unchanged"));
            return Ok(());
        }
    }
    let toml_str = toml::to_string_pretty(&config)?;
    std::fs::write(&config_path, &toml_str)?;
    println!(
        "{}",
        output::success(&format!("Config written to {}", config_path.display()))
    );

    // Create vault directories + templates
    let resolved = config.resolve_paths();
    let vault_root = PathBuf::from(&resolved.vault.path);
    for cat in &categories {
        let dir = vault_root.join(cat);
        std::fs::create_dir_all(&dir)?;
    }
    let tpl_dir = vault_root.join("_templates");
    std::fs::create_dir_all(&tpl_dir)?;

    // Write default templates for known categories
    for cat in &categories {
        let content = match cat.as_str() {
            "procedures" => Some(PROCEDURE_TEMPLATE),
            "decisions" => Some(DECISION_TEMPLATE),
            "learnings" => Some(LEARNING_TEMPLATE),
            "concepts" => Some(CONCEPT_TEMPLATE),
            _ => None,
        };
        if let Some(tpl) = content {
            let path = tpl_dir.join(format!("{cat}.md"));
            if !path.exists() {
                std::fs::write(&path, tpl)?;
            }
        }
    }

    println!(
        "{}",
        output::success(&format!("Vault created at {}", vault_root.display()))
    );

    println!();
    println!("  {}", console::style("Next steps:").bold());
    println!(
        "    1. Set your {} env var",
        console::style("OPENAI_API_KEY").bold()
    );
    println!("    2. Run {}", console::style("brain-mcp serve").bold());
    println!();

    Ok(())
}
