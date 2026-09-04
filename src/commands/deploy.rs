use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::api::models::{CreateStaticDeploymentRequest, StaticManifestFile};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::{QuomeError, Result};
use crate::manifest::{build_manifest, content_type_for, ManifestEntry};
use crate::ui;

const UPLOAD_WORKERS: usize = 8;
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const POLL_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Parser)]
pub struct DeployArgs {
    /// Site root to deploy (your build output — must contain index.html)
    #[arg(default_value = ".")]
    directory: PathBuf,
    /// App slug or UUID (uses linked app if not provided)
    #[arg(long, short)]
    app: Option<String>,
    /// Create the app if the slug doesn't exist yet
    #[arg(long)]
    create: bool,
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(args: DeployArgs) -> Result<()> {
    let entries = build_manifest(&args.directory.canonicalize().map_err(|_| {
        QuomeError::Usage(format!("directory not found: {}", args.directory.display()))
    })?)?;

    let config = Config::load()?;
    let token = config.require_token()?;
    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };
    let client = QuomeClient::new(Some(&token), None)?;

    let (app_id, app_label) = resolve_app(&client, org_id, &config, &args).await?;

    let total_bytes: u64 = entries.iter().map(|e| e.size).sum();
    if !args.json {
        println!(
            "Deploying {} files ({} bytes) to {}",
            entries.len(),
            total_bytes,
            app_label
        );
    }

    let sp = ui::spinner("Starting deployment...");
    client.create_or_get_static_site(org_id, app_id).await?;
    let session = client
        .create_static_deployment(
            org_id,
            app_id,
            &CreateStaticDeploymentRequest {
                source_type: "api",
                files: entries
                    .iter()
                    .map(|e| StaticManifestFile {
                        path: e.path.clone(),
                        size: e.size,
                    })
                    .collect(),
            },
        )
        .await?;
    sp.finish_and_clear();

    upload_all(&entries, &session.upload_urls, total_bytes, args.json).await?;

    let sp = ui::spinner("Finalizing...");
    client
        .finalize_static_deployment(org_id, app_id, session.deployment_id)
        .await?;
    let row = poll_until_terminal(&client, org_id, app_id, session.deployment_id).await?;
    sp.finish_and_clear();

    if row.status == "failed" {
        return Err(QuomeError::ApiError(format!(
            "deploy failed: {}",
            row.error.unwrap_or_else(|| "unknown error".into())
        )));
    }

    let url = client
        .get_app(org_id, app_id)
        .await
        .ok()
        .and_then(|a| a.primary_url);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "deployment_id": session.deployment_id,
                "status": row.status,
                "url": url,
            }))?
        );
    } else {
        match url {
            Some(u) => ui::print_success("Deployed", &[("URL", &u)]),
            None => println!("Deployed. URL pending — check the app page in the dashboard."),
        }
    }
    Ok(())
}

/// `--app` slug|uuid → app id, falling back to the linked app; `--create`
/// makes a static app when the slug doesn't resolve.
async fn resolve_app(
    client: &QuomeClient,
    org_id: Uuid,
    config: &Config,
    args: &DeployArgs,
) -> Result<(Uuid, String)> {
    let Some(app_ref) = &args.app else {
        let id = config.require_linked_app()?;
        return Ok((id, id.to_string()));
    };
    if let Ok(id) = Uuid::parse_str(app_ref) {
        return Ok((id, app_ref.clone()));
    }
    // Paginate in full — page-one-only false-negatives past 100 apps.
    let apps = client
        .list_all_pages::<crate::api::models::App>(&format!("/api/v1/orgs/{}/apps", org_id))
        .await?;
    if let Some(app) = apps
        .iter()
        .find(|a| a.slug.as_deref() == Some(app_ref.as_str()))
    {
        return Ok((app.id, app_ref.clone()));
    }
    if args.create {
        println!("App {} not found — creating it.", app_ref);
        let app = client.create_static_app(org_id, app_ref).await?;
        return Ok((app.id, app_ref.clone()));
    }
    let slugs: Vec<String> = apps.iter().filter_map(|a| a.slug.clone()).collect();
    Err(QuomeError::NotFound(format!(
        "no app with slug or id '{}'. Existing: {}. Pass --create to create it.",
        app_ref,
        if slugs.is_empty() {
            "(none)".into()
        } else {
            slugs.join(", ")
        }
    )))
}

async fn upload_all(
    entries: &[ManifestEntry],
    upload_urls: &std::collections::HashMap<String, String>,
    total_bytes: u64,
    quiet: bool,
) -> Result<()> {
    // Bare client: signed GCS URLs — the X-API-Key default header must NOT
    // be sent, and Content-Type must match the manifest's type.
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| QuomeError::ApiError(e.to_string()))?;

    let bar = if quiet {
        ProgressBar::hidden()
    } else {
        let b = ProgressBar::new(total_bytes);
        b.set_style(
            ProgressStyle::with_template("{bar:30} {bytes}/{total_bytes} {bytes_per_sec}")
                .expect("static template"),
        );
        b
    };

    let sem = Arc::new(Semaphore::new(UPLOAD_WORKERS));
    let mut set: JoinSet<Result<u64>> = JoinSet::new();
    for entry in entries {
        let url = upload_urls
            .get(&entry.path)
            .ok_or_else(|| QuomeError::ApiError(format!("no upload URL for {}", entry.path)))?
            .clone();
        let http = http.clone();
        let sem = sem.clone();
        let path = entry.path.clone();
        let local = entry.local.clone();
        let size = entry.size;
        let content_type = content_type_for(&path);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore open");
            let body = tokio::fs::read(&local)
                .await
                .map_err(|e| QuomeError::ApiError(format!("read {}: {}", path, e)))?;
            let resp = http
                .put(&url)
                .header("Content-Type", content_type)
                .body(body)
                .send()
                .await
                .map_err(|e| QuomeError::ApiError(format!("upload {}: {}", path, e)))?;
            if !resp.status().is_success() {
                return Err(QuomeError::ApiError(format!(
                    "upload failed for {}: HTTP {}",
                    path,
                    resp.status()
                )));
            }
            Ok(size)
        });
    }
    while let Some(joined) = set.join_next().await {
        let size = joined.map_err(|e| QuomeError::ApiError(e.to_string()))??;
        bar.inc(size);
    }
    bar.finish_and_clear();
    Ok(())
}

async fn poll_until_terminal(
    client: &QuomeClient,
    org_id: Uuid,
    app_id: Uuid,
    deployment_id: Uuid,
) -> Result<crate::api::models::StaticDeployment> {
    let deadline = Instant::now() + POLL_TIMEOUT;
    while Instant::now() < deadline {
        let rows = client.list_static_deployments(org_id, app_id).await?;
        if let Some(row) = rows.into_iter().find(|r| r.id == deployment_id) {
            if row.status == "active" || row.status == "failed" {
                return Ok(row);
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(QuomeError::ApiError(format!(
        "deploy did not reach a terminal state within {}s",
        POLL_TIMEOUT.as_secs()
    )))
}
