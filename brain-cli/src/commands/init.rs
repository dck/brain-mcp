use std::path::PathBuf;

use dialoguer::theme::ColorfulTheme;
use dialoguer::{MultiSelect, Select};
use rustyline::DefaultEditor;

use brain_core::config::{
    Config, EmbeddingConfig, IndexConfig, SearchConfig, ServerConfig, VaultConfig,
};

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

const PROCEDURE_TEMPLATE: &str = r#"## Context

{{content}}

## Steps

1.

## Notes

"#;

const DECISION_TEMPLATE: &str = r#"## Context

{{content}}

## Options Considered

1.

## Decision

## Consequences

"#;

const LEARNING_TEMPLATE: &str = r#"## What I Learned

{{content}}

## Why It Matters

## References

"#;

const CONCEPT_TEMPLATE: &str = r#"## Definition

{{content}}

## Examples

## Related Concepts

"#;

#[cfg(feature = "local-embeddings")]
const MINILM_REPO: &str = "sentence-transformers/all-MiniLM-L6-v2";
#[cfg(feature = "local-embeddings")]
const MINILM_REVISION: &str = "c9745ed1d9f207416be6d2e6f8de32d1f16199bf";
#[cfg(feature = "local-embeddings")]
const MINILM_FILES: &[(&str, &str, &str)] = &[
    (
        "onnx/model.onnx",
        "model.onnx",
        "6fd5d72fe4589f189f8ebc006442dbb529bb7ce38f8082112682524616046452",
    ),
    (
        "tokenizer.json",
        "tokenizer.json",
        "be50c3628f2bf5bb5e3a7f17b1f74611b2561a3a27eeab05e5aa30f411572037",
    ),
];

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
    let def_vault = existing
        .as_ref()
        .map_or("~/brain", |c| c.vault.path.as_str());
    let def_model = existing
        .as_ref()
        .map_or("text-embedding-3-small", |c| c.embedding.model.as_str());
    let def_api_key_env = existing
        .as_ref()
        .and_then(|c| c.embedding.api_key_env.as_deref())
        .unwrap_or("OPENAI_API_KEY");
    let def_port = existing.as_ref().map_or(47200, |c| c.server.http_port);
    let def_grace = existing
        .as_ref()
        .map_or(60, |c| c.server.grace_period_seconds);
    let def_provider = existing
        .as_ref()
        .map_or("openai", |c| c.embedding.provider.as_str());

    // 1. Vault path
    let vault_path = prompt_input(&mut rl, "Vault path", def_vault)?;

    // 2. Categories
    let existing_cats: Vec<String> = existing
        .as_ref()
        .map(|c| c.vault.categories.clone())
        .unwrap_or_default();
    let cat_defaults: Vec<bool> = ALL_CATEGORIES
        .iter()
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
    providers.push(
        "Local ONNX (not available — this build has no local-embeddings feature)".to_string(),
    );

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

            let client = reqwest::Client::new();
            for (remote, local, sha256) in MINILM_FILES {
                let url = format!(
                    "https://huggingface.co/{MINILM_REPO}/resolve/{MINILM_REVISION}/{remote}"
                );
                ensure_model_file(&client, &url, &model_dir.join(local), sha256, local).await?;
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
            eprintln!("  ONNX support not compiled in. Reinstall with default features:");
            eprintln!("    cargo install --path brain-cli");
            std::process::exit(1);
        }
        _ => unreachable!(),
    };

    // 4. HTTP port
    let http_port: u16 = prompt_input(&mut rl, "HTTP port", &def_port.to_string())?
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid port number"))?;

    // 5. Grace period
    let grace_period: u64 =
        prompt_input(&mut rl, "Grace period (seconds)", &def_grace.to_string())?
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
        search: SearchConfig::default(),
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
    let mut step = 1;
    if resolved.embedding.provider == "openai" {
        println!(
            "    {step}. Set your {} env var",
            console::style("OPENAI_API_KEY").bold()
        );
        step += 1;
    }
    println!(
        "    {step}. Register with Claude Code:\n       {}",
        console::style(
            "claude mcp add --scope user --transport stdio brain-mcp -- brain-mcp serve --stdio"
        )
        .bold()
    );
    step += 1;
    println!(
        "    {step}. Recommended: add a SessionStart hook so every session starts with a memory index\n       (models rarely call memory_search unprompted). In ~/.claude/settings.json:"
    );
    println!("{}", console::style(SESSION_START_HOOK_SNIPPET).dim());
    println!();

    Ok(())
}

const SESSION_START_HOOK_SNIPPET: &str = r#"       {
         "hooks": {
           "SessionStart": [{
             "matcher": "startup|resume|clear",
             "hooks": [{
               "type": "command",
               "command": "brain-mcp recall --project \"$(basename \"$PWD\")\"",
               "timeout": 10
             }]
           }]
         }
       }"#;

fn load_existing_config() -> Option<Config> {
    let path = default_config_path();
    let raw = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&raw).ok()
}

#[cfg(feature = "local-embeddings")]
async fn ensure_model_file(
    client: &reqwest::Client,
    url: &str,
    dest: &std::path::Path,
    sha256: &str,
    label: &str,
) -> anyhow::Result<()> {
    if dest.exists() {
        if sha256_file(dest)? == sha256 {
            println!(
                "{}",
                output::success(&format!("{label} already downloaded (checksum ok)"))
            );
            return Ok(());
        }
        println!(
            "{}",
            output::error(&format!("{label} checksum mismatch, downloading again"))
        );
    }
    download_verified(client, url, dest, sha256, label).await
}

#[cfg(feature = "local-embeddings")]
async fn download_verified(
    client: &reqwest::Client,
    url: &str,
    dest: &std::path::Path,
    sha256: &str,
    label: &str,
) -> anyhow::Result<()> {
    use futures_util::StreamExt;
    use indicatif::{ProgressBar, ProgressStyle};
    use tokio::io::AsyncWriteExt;

    let part = dest.with_file_name(format!(
        "{}.part",
        dest.file_name().unwrap().to_string_lossy()
    ));

    let result: anyhow::Result<()> = async {
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

        let mut file = tokio::fs::File::create(&part).await?;
        let mut hasher = hmac_sha256::Hash::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            hasher.update(&chunk);
            pb.inc(chunk.len() as u64);
        }
        file.flush().await?;
        pb.finish_with_message(format!("{label} done"));

        let actual = to_hex(&hasher.finalize());
        if actual != sha256 {
            std::fs::remove_file(&part)?;
            anyhow::bail!("Checksum mismatch for {label}: expected {sha256}, got {actual}");
        }

        std::fs::rename(&part, dest)?;
        Ok(())
    }
    .await;

    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

#[cfg(feature = "local-embeddings")]
fn sha256_file(path: &std::path::Path) -> anyhow::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = hmac_sha256::Hash::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

#[cfg(feature = "local-embeddings")]
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(all(test, feature = "local-embeddings"))]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[tokio::test]
    async fn download_verified_writes_file_on_matching_hash() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/f"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f");
        let client = reqwest::Client::new();
        download_verified(
            &client,
            &format!("{}/f", server.uri()),
            &dest,
            HELLO_SHA256,
            "f",
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello");
        assert!(!dest.with_file_name("f.part").exists());
    }

    #[tokio::test]
    async fn download_verified_rejects_bad_hash() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/f"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f");
        let client = reqwest::Client::new();
        let err = download_verified(&client, &format!("{}/f", server.uri()), &dest, "00", "f")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Checksum mismatch"));
        assert!(!dest.exists());
        assert!(!dest.with_file_name("f.part").exists());
    }

    #[tokio::test]
    async fn download_verified_http_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/f"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f");
        let client = reqwest::Client::new();
        let result = download_verified(
            &client,
            &format!("{}/f", server.uri()),
            &dest,
            HELLO_SHA256,
            "f",
        )
        .await;

        assert!(result.is_err());
        assert!(!dest.exists());
        assert!(!dest.with_file_name("f.part").exists());
    }

    #[tokio::test]
    async fn ensure_model_file_skips_verified_existing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/f"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
            .expect(0)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f");
        std::fs::write(&dest, "hello").unwrap();
        let client = reqwest::Client::new();
        ensure_model_file(
            &client,
            &format!("{}/f", server.uri()),
            &dest,
            HELLO_SHA256,
            "f",
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello");
    }

    #[tokio::test]
    async fn ensure_model_file_replaces_corrupt_existing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/f"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
            .expect(1)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f");
        std::fs::write(&dest, "garbage").unwrap();
        let client = reqwest::Client::new();
        ensure_model_file(
            &client,
            &format!("{}/f", server.uri()),
            &dest,
            HELLO_SHA256,
            "f",
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello");
    }

    #[test]
    fn sha256_file_matches_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, "hello").unwrap();
        assert_eq!(sha256_file(&path).unwrap(), HELLO_SHA256);
    }
}
