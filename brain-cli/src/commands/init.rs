use std::path::PathBuf;

use dialoguer::theme::ColorfulTheme;
use dialoguer::{MultiSelect, Select};
use rustyline::DefaultEditor;

use brain_core::config::{Config, EmbeddingConfig, IndexConfig, ServerConfig, VaultConfig};

use super::{config_dir, default_config_path};
use crate::output;

fn prompt_input(rl: &mut DefaultEditor, label: &str, default: &str) -> anyhow::Result<String> {
    let prompt = if default.is_empty() {
        format!("  {}: ", label)
    } else {
        format!("  {} [{}]: ", label, default)
    };
    let input = rl.readline(&prompt)?;
    let input = input.trim().to_string();
    if input.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(input)
    }
}

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
        anyhow::bail!("init requires interactive input and cannot be used with --json");
    }

    // Load existing config as defaults (if any)
    let existing = load_existing_config();
    let is_reconfigure = existing.is_some();

    println!();
    println!(
        "  {} {}",
        console::style("brain-mcp").bold(),
        if is_reconfigure {
            console::style("reconfigure (existing values shown as defaults)").dim()
        } else {
            console::style("setup wizard").dim()
        }
    );
    println!();

    let mut rl = DefaultEditor::new()?;
    let theme = ColorfulTheme::default();

    // Defaults from existing config or sensible fallbacks
    let def_vault = existing.as_ref().map_or("~/brain", |c| c.vault.path.as_str());
    let def_model = existing.as_ref().map_or("text-embedding-3-small", |c| c.embedding.model.as_str());
    let def_api_key_env = existing.as_ref()
        .and_then(|c| c.embedding.api_key_env.as_deref())
        .unwrap_or("OPENAI_API_KEY");
    let def_port = existing.as_ref().map_or(47200, |c| c.server.http_port);
    let def_grace = existing.as_ref().map_or(60, |c| c.server.grace_period_seconds);
    let def_provider = existing.as_ref().map_or("openai", |c| c.embedding.provider.as_str());

    // 1. Vault path
    let vault_path = prompt_input(&mut rl, "Vault path", def_vault)?;

    // 2. Categories
    let existing_cats: Vec<String> = existing.as_ref()
        .map(|c| c.vault.categories.clone())
        .unwrap_or_default();
    let cat_defaults: Vec<bool> = ALL_CATEGORIES.iter()
        .map(|cat| {
            if existing_cats.is_empty() {
                true // first run: all on
            } else {
                existing_cats.iter().any(|c| c == cat)
            }
        })
        .collect();
    let chosen = MultiSelect::with_theme(&theme)
        .with_prompt("  Categories (space to toggle)")
        .items(ALL_CATEGORIES)
        .defaults(&cat_defaults)
        .interact()?;
    let categories: Vec<String> = chosen
        .iter()
        .map(|&i| ALL_CATEGORIES[i].to_string())
        .collect();

    // 3. Embedding provider
    let mut providers = vec!["OpenAI".to_string()];

    #[cfg(feature = "local-embeddings")]
    providers.push("Local ONNX (all-MiniLM-L6-v2, ~90MB download)".to_string());

    #[cfg(not(feature = "local-embeddings"))]
    providers
        .push("Local ONNX (not available — rebuild with --features local-embeddings)".to_string());

    let default_provider_idx = match def_provider {
        "onnx" => 1,
        _ => 0,
    };
    let provider_refs: Vec<&str> = providers.iter().map(|s| s.as_str()).collect();
    let provider_idx = Select::with_theme(&theme)
        .with_prompt("  Embedding provider")
        .items(&provider_refs)
        .default(default_provider_idx)
        .interact()?;

    let (provider, model, api_key_env, model_path) = match provider_idx {
        0 => {
            let model = prompt_input(&mut rl, "OpenAI model", def_model)?;
            let env_var = prompt_input(&mut rl, "API key env var", def_api_key_env)?;
            ("openai".to_string(), model, Some(env_var), None)
        }
        #[cfg(feature = "local-embeddings")]
        1 => {
            let model_dir = config_dir().join("models/all-MiniLM-L6-v2");
            std::fs::create_dir_all(&model_dir)?;

            let model_path = model_dir.join("model.onnx");
            let tokenizer_path = model_dir.join("tokenizer.json");

            if !model_path.exists() {
                download_file(
                    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx",
                    &model_path,
                    "Downloading model.onnx",
                ).await?;
            } else {
                println!("{}", output::success("model.onnx already downloaded"));
            }

            if !tokenizer_path.exists() {
                download_file(
                    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json",
                    &tokenizer_path,
                    "Downloading tokenizer.json",
                ).await?;
            } else {
                println!("{}", output::success("tokenizer.json already downloaded"));
            }

            (
                "onnx".to_string(),
                "all-MiniLM-L6-v2".to_string(),
                None,
                Some(model_dir.to_string_lossy().to_string()),
            )
        }
        #[cfg(not(feature = "local-embeddings"))]
        1 => {
            eprintln!("  ONNX support not compiled in. Rebuild with:");
            eprintln!("    cargo install --path brain-cli --features local-embeddings");
            std::process::exit(1);
        }
        _ => unreachable!(),
    };

    // 4. HTTP port
    let http_port: u16 = prompt_input(&mut rl, "HTTP port", &def_port.to_string())?
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid port number"))?;

    // 5. Grace period
    let grace_period: u64 = prompt_input(&mut rl, "Grace period (seconds)", &def_grace.to_string())?
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid number"))?;

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

    // Write config (always write — user chose to run init)
    let config_dir = config_dir();
    std::fs::create_dir_all(&config_dir)?;
    let config_path = default_config_path();
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
    if resolved.embedding.provider == "openai" {
        println!(
            "    1. Set your {} env var",
            console::style("OPENAI_API_KEY").bold()
        );
        println!("    2. Run {}", console::style("brain-mcp serve").bold());
    } else {
        println!("    Run {}", console::style("brain-mcp serve").bold());
    }
    println!();

    Ok(())
}

fn load_existing_config() -> Option<Config> {
    let path = default_config_path();
    let raw = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&raw).ok()
}

#[cfg(feature = "local-embeddings")]
async fn download_file(url: &str, dest: &std::path::Path, label: &str) -> anyhow::Result<()> {
    use futures_util::StreamExt;
    use indicatif::{ProgressBar, ProgressStyle};
    use tokio::io::AsyncWriteExt;

    let client = reqwest::Client::new();
    let resp = client.get(url).send().await?;

    if !resp.status().is_success() {
        anyhow::bail!("Download failed: HTTP {}", resp.status());
    }

    let total_size = resp.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("  {msg} [{bar:30}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("=> "),
    );
    pb.set_message(label.to_string());

    let mut file = tokio::fs::File::create(dest).await?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        pb.inc(chunk.len() as u64);
    }
    file.flush().await?;
    pb.finish_with_message(format!("{label} done"));
    Ok(())
}
