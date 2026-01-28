use clap::{Parser, Subcommand};
use colored::Colorize;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::time::Duration;
use uuid::Uuid;

use crate::api::models::{
    AgentState, ColorPreferences, SendPromptRequest, StackConfig, StartAgentRequest, TechStack,
};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui;

#[derive(Subcommand)]
pub enum AgentCommands {
    /// Start a new AI app building workflow
    Start(StartArgs),
    /// Send a follow-up prompt to an active workflow
    Prompt(PromptArgs),
    /// Get the current state of a workflow
    State(StateArgs),
    /// Stop an active workflow
    Stop(StopArgs),
    /// Pull the latest changes from a workflow
    Pull(PullArgs),
}

#[derive(Parser)]
pub struct StartArgs {
    /// The prompt describing the application to build
    prompt: String,

    /// Project name (auto-generated if not provided)
    #[arg(long)]
    name: Option<String>,

    /// Create a GitHub repository for the app
    #[arg(long)]
    github: bool,

    /// Run build stages in parallel for faster completion
    #[arg(long)]
    parallel: bool,

    /// WCAG accessibility compliance target (A, AA, AAA)
    #[arg(long, default_value = "AA")]
    accessibility: String,

    /// Backend stack (e.g., fastapi, express, django)
    #[arg(long)]
    backend: Option<String>,

    /// Backend language (e.g., python, javascript, typescript)
    #[arg(long)]
    backend_lang: Option<String>,

    /// Frontend stack (e.g., react, vue, nextjs)
    #[arg(long)]
    frontend: Option<String>,

    /// Frontend language (e.g., javascript, typescript)
    #[arg(long)]
    frontend_lang: Option<String>,

    /// Database type (e.g., postgresql, sqlite, mongodb)
    #[arg(long)]
    database: Option<String>,

    /// Primary color hex code (e.g., #3B82F6)
    #[arg(long)]
    primary_color: Option<String>,

    /// Secondary color hex code
    #[arg(long)]
    secondary_color: Option<String>,

    /// Don't watch progress (just start and return thread ID)
    #[arg(long)]
    no_watch: bool,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct PromptArgs {
    /// The workflow thread ID
    thread_id: Uuid,

    /// The follow-up prompt or instruction
    prompt: String,

    /// Watch progress after sending prompt
    #[arg(long, short)]
    watch: bool,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct StateArgs {
    /// The workflow thread ID
    thread_id: Uuid,

    /// Watch progress continuously
    #[arg(long, short)]
    watch: bool,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct StopArgs {
    /// The workflow thread ID
    thread_id: Uuid,

    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct PullArgs {
    /// The workflow thread ID
    thread_id: Uuid,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(command: AgentCommands) -> Result<()> {
    match command {
        AgentCommands::Start(args) => start(args).await,
        AgentCommands::Prompt(args) => prompt(args).await,
        AgentCommands::State(args) => state(args).await,
        AgentCommands::Stop(args) => stop(args).await,
        AgentCommands::Pull(args) => pull(args).await,
    }
}

async fn start(args: StartArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let client = QuomeClient::new(Some(&token), None)?;

    // Build tech stack if any options provided
    let tech_stack = if args.backend.is_some()
        || args.backend_lang.is_some()
        || args.frontend.is_some()
        || args.frontend_lang.is_some()
        || args.database.is_some()
    {
        Some(TechStack {
            backend: if args.backend.is_some() || args.backend_lang.is_some() {
                Some(StackConfig {
                    stack: args.backend,
                    language: args.backend_lang,
                })
            } else {
                None
            },
            frontend: if args.frontend.is_some() || args.frontend_lang.is_some() {
                Some(StackConfig {
                    stack: args.frontend,
                    language: args.frontend_lang,
                })
            } else {
                None
            },
            database: args.database,
        })
    } else {
        None
    };

    // Build color preferences if any provided
    let color_preferences = if args.primary_color.is_some() || args.secondary_color.is_some() {
        Some(ColorPreferences {
            color_type: "custom".to_string(),
            primary_color: args.primary_color,
            secondary_color: args.secondary_color,
        })
    } else {
        None
    };

    let request = StartAgentRequest {
        prompt: args.prompt.clone(),
        project_name: args.name.clone(),
        include_github: Some(args.github),
        parallel_mode: Some(args.parallel),
        accessibility_target: Some(args.accessibility),
        tech_stack,
        color_preferences,
    };

    let sp = ui::spinner("Starting AI workflow...");
    let response = client
        .post::<crate::api::models::StartAgentResponse, _>(
            "/api/v1/agents/quome-coder/start",
            &request,
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    // If no-watch, just print the result and exit
    if args.no_watch {
        ui::print_success(
            "Started AI workflow",
            &[
                ("Thread ID", &response.thread_id.to_string()),
                ("Status", &response.status),
                ("Message", &response.message),
            ],
        );
        println!();
        println!(
            "{}",
            "Use 'quome agent state <thread-id>' to check progress.".dimmed()
        );
        return Ok(());
    }

    // Watch mode - show beautiful progress
    let app_name = args.name.unwrap_or_else(|| "your app".to_string());
    watch_progress(&client, response.thread_id, &args.prompt, &app_name).await
}

async fn prompt(args: PromptArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let client = QuomeClient::new(Some(&token), None)?;

    let request = SendPromptRequest {
        prompt: args.prompt.clone(),
    };

    let sp = ui::spinner("Sending prompt...");
    let response = client
        .post::<crate::api::models::SendPromptResponse, _>(
            &format!("/api/v1/agents/quome-coder/{}/prompt", args.thread_id),
            &request,
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    if !response.success {
        eprintln!("{} {}", "error:".red().bold(), response.message);
        return Ok(());
    }

    ui::print_success("Prompt sent", &[("Message", &response.message)]);

    // Watch if requested
    if args.watch {
        println!();
        watch_progress(&client, args.thread_id, &args.prompt, "your app").await?;
    }

    Ok(())
}

async fn state(args: StateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let client = QuomeClient::new(Some(&token), None)?;

    if args.watch {
        // Get initial state for app name
        let state = client
            .get::<AgentState>(&format!(
                "/api/v1/agents/quome-coder/{}/state",
                args.thread_id
            ))
            .await?;
        let app_name = state
            .app_context
            .as_ref()
            .and_then(|c| c.name.clone())
            .unwrap_or_else(|| "your app".to_string());
        return watch_progress(&client, args.thread_id, "", &app_name).await;
    }

    let sp = ui::spinner("Fetching workflow state...");
    let state = client
        .get::<AgentState>(&format!(
            "/api/v1/agents/quome-coder/{}/state",
            args.thread_id
        ))
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&state)?);
    } else {
        print_agent_state(&state);
    }

    Ok(())
}

async fn stop(args: StopArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    if !args.force {
        let confirm = inquire::Confirm::new(&format!(
            "Are you sure you want to stop workflow {}?",
            args.thread_id
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| crate::errors::QuomeError::Io(std::io::Error::other(e.to_string())))?;

        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Stopping workflow...");
    let response = client
        .post::<crate::api::models::StopWorkflowResponse, _>(
            &format!("/api/v1/agents/quome-coder/{}/stop", args.thread_id),
            &serde_json::json!({}),
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else if response.success {
        ui::print_success("Workflow stopped", &[("Message", &response.message)]);
    } else {
        eprintln!("{} {}", "error:".red().bold(), response.message);
    }

    Ok(())
}

async fn pull(args: PullArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Pulling latest changes...");
    let response = client
        .get::<crate::api::models::PullLatestResponse>(&format!(
            "/api/v1/agents/quome-coder/{}/pull",
            args.thread_id
        ))
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else if response.success {
        ui::print_success("Pulled latest changes", &[("Message", &response.message)]);
        if let Some(state) = &response.state {
            println!();
            print_agent_state(state);
        }
    } else {
        eprintln!("{} {}", "error:".red().bold(), response.message);
    }

    Ok(())
}

// ============ Watch Mode Progress Display ============

async fn watch_progress(
    client: &QuomeClient,
    thread_id: Uuid,
    initial_prompt: &str,
    app_name: &str,
) -> Result<()> {
    let mp = MultiProgress::new();

    // Header
    println!();
    println!("{}", format!("  Building: {}", app_name).cyan().bold());
    if !initial_prompt.is_empty() {
        let truncated = if initial_prompt.len() > 60 {
            format!("{}...", &initial_prompt[..60])
        } else {
            initial_prompt.to_string()
        };
        println!("  {}", truncated.dimmed());
    }
    println!();

    // Main progress bar
    let progress_bar = mp.add(ProgressBar::new(100));
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("  {bar:40.cyan/dim} {pos:>3}%  {msg}")
            .unwrap()
            .progress_chars("━━─"),
    );

    // Status line
    let status_bar = mp.add(ProgressBar::new_spinner());
    status_bar.set_style(
        ProgressStyle::default_spinner()
            .template("  {spinner:.cyan} {msg}")
            .unwrap(),
    );
    status_bar.enable_steady_tick(Duration::from_millis(100));

    // Phase line
    let phase_bar = mp.add(ProgressBar::new_spinner());
    phase_bar.set_style(
        ProgressStyle::default_spinner()
            .template("  {msg}")
            .unwrap(),
    );

    // Info line (URLs, etc)
    let info_bar = mp.add(ProgressBar::new_spinner());
    info_bar.set_style(
        ProgressStyle::default_spinner()
            .template("  {msg}")
            .unwrap(),
    );

    let mut last_message_count = 0;
    let mut deployment_url: Option<String> = None;

    let final_state: AgentState = loop {
        // Fetch current state
        let state = match client
            .get::<AgentState>(&format!("/api/v1/agents/quome-coder/{}/state", thread_id))
            .await
        {
            Ok(s) => s,
            Err(e) => {
                progress_bar.finish_and_clear();
                status_bar.finish_and_clear();
                phase_bar.finish_and_clear();
                info_bar.finish_and_clear();
                return Err(e);
            }
        };

        // Update progress bar
        if let Some(progress) = &state.progress {
            let pct = progress.percentage.unwrap_or(0.0) as u64;
            progress_bar.set_position(pct);

            if let (Some(current), Some(total)) = (progress.current_stage, progress.total_stages) {
                progress_bar.set_message(format!("Stage {}/{}", current, total));
            }
        }

        // Update status
        if let Some(status) = &state.status {
            let truncated = if status.len() > 50 {
                format!("{}...", &status[..50])
            } else {
                status.clone()
            };
            status_bar.set_message(truncated);
        }

        // Update phase
        if let Some(phase) = &state.phase {
            let phase_icon = match phase.as_str() {
                "planning" => "📋",
                "building" => "🔨",
                "testing" => "🧪",
                "deploying" => "🚀",
                "deployed" | "complete" => "✅",
                _ => "⚡",
            };
            phase_bar.set_message(
                format!("{} Phase: {}", phase_icon, phase.to_uppercase())
                    .dimmed()
                    .to_string(),
            );
        }

        // Update info line with URLs
        let mut info_parts: Vec<String> = Vec::new();

        if let Some(container) = &state.container_info {
            if let Some(url) = &container.frontend_url {
                info_parts.push(format!("Preview: {}", url.cyan()));
            }
        }

        if let Some(deploy) = &state.deployment {
            if let Some(url) = &deploy.url {
                deployment_url = Some(url.clone());
                if deploy.status.as_deref() == Some("deployed") {
                    info_parts.push(format!("Live: {}", url.green().bold()));
                }
            }
        }

        if !info_parts.is_empty() {
            info_bar.set_message(info_parts.join("  │  "));
        }

        // Show new messages from AI
        if state.messages.len() > last_message_count {
            for msg in state.messages.iter().skip(last_message_count) {
                if msg.message_type == "assistant" {
                    if let Some(content) = &msg.content {
                        // Print AI message below the progress bars
                        let truncated = if content.len() > 70 {
                            format!("{}...", &content[..70])
                        } else {
                            content.clone()
                        };
                        mp.println(format!("  {} {}", "AI:".green().bold(), truncated.dimmed()))?;
                    }
                }
            }
            last_message_count = state.messages.len();
        }

        // Check if complete
        let phase = state.phase.as_deref().unwrap_or("");
        if !state.is_working && (phase == "deployed" || phase == "complete" || phase == "failed") {
            break state;
        }

        // Also check deployment status
        if let Some(deploy) = &state.deployment {
            if deploy.status.as_deref() == Some("deployed") {
                progress_bar.set_position(100);
                break state;
            }
        }

        // Poll interval
        tokio::time::sleep(Duration::from_secs(2)).await;
    };

    // Clean up progress bars
    progress_bar.finish_and_clear();
    status_bar.finish_and_clear();
    phase_bar.finish_and_clear();
    info_bar.finish_and_clear();

    // Print final result
    println!();

    let phase = final_state.phase.as_deref().unwrap_or("");

    if phase == "failed" {
        println!("  {} {}", "✗".red().bold(), "Build failed".red().bold());
        if let Some(status) = &final_state.status {
            println!("  {}", status.dimmed());
        }
    } else {
        // Success!
        print_deployment_success(&final_state, deployment_url.as_deref());
    }

    Ok(())
}

fn print_deployment_success(state: &AgentState, deployment_url: Option<&str>) {
    println!(
        "  {}",
        "╔══════════════════════════════════════════════════════════╗".green()
    );
    println!(
        "  {}",
        "║                                                          ║".green()
    );
    println!(
        "  {}  {}  {}",
        "║".green(),
        "🎉 Your app is live!                                    ".bold(),
        "║".green()
    );
    println!(
        "  {}",
        "║                                                          ║".green()
    );

    if let Some(url) = deployment_url {
        let url_display = format!("   {}", url);
        let padding = 56_i32.saturating_sub(url_display.len() as i32);
        println!(
            "  {}  {}{}{}",
            "║".green(),
            url_display.cyan().bold(),
            " ".repeat(padding.max(0) as usize),
            "║".green()
        );
    }

    println!(
        "  {}",
        "║                                                          ║".green()
    );
    println!(
        "  {}",
        "╚══════════════════════════════════════════════════════════╝".green()
    );

    // Additional details
    println!();

    let mut details: Vec<(&str, String)> = Vec::new();

    if let Some(ctx) = &state.app_context {
        if let Some(name) = &ctx.name {
            details.push(("App Name", name.clone()));
        }
    }

    if let Some(app_id) = &state.app_uuid {
        details.push(("App ID", app_id.to_string()));
    }

    if let Some(true) = state.github_repo_created {
        if let Some(url) = &state.github_repo_url {
            details.push(("GitHub", url.clone()));
        }
    }

    if !state.files.is_empty() {
        details.push(("Files", format!("{} files generated", state.files.len())));
    }

    if let Some(ran) = state.tests_ran {
        if ran > 0 {
            let passed = state.tests_passed.unwrap_or(0);
            let failed = state.tests_failed.unwrap_or(0);
            details.push(("Tests", format!("{} passed, {} failed", passed, failed)));
        }
    }

    if !details.is_empty() {
        for (key, value) in &details {
            println!("  {} {}", format!("{}:", key).dimmed(), value);
        }
    }

    println!();
    println!(
        "  {}",
        "Run 'quome agent prompt <thread-id> \"your changes\"' to iterate.".dimmed()
    );
}

fn print_agent_state(state: &AgentState) {
    println!("{}", "Workflow State".bold());
    println!("  {} {}", "Thread ID:".cyan(), state.thread_id);
    println!(
        "  {} {}",
        "Working:".cyan(),
        if state.is_working { "Yes" } else { "No" }
    );

    if let Some(status) = &state.status {
        println!("  {} {}", "Status:".cyan(), status);
    }

    if let Some(phase) = &state.phase {
        println!("  {} {}", "Phase:".cyan(), phase);
    }

    // Progress
    if let Some(progress) = &state.progress {
        if let Some(pct) = progress.percentage {
            let current = progress.current_stage.unwrap_or(0);
            let total = progress.total_stages.unwrap_or(0);
            println!(
                "  {} {:.0}% (stage {}/{})",
                "Progress:".cyan(),
                pct,
                current,
                total
            );
        }
    }

    // App context
    if let Some(ctx) = &state.app_context {
        if let Some(name) = &ctx.name {
            println!();
            println!("{}", "Application".bold());
            println!("  {} {}", "Name:".cyan(), name);
        }
        if let Some(goal) = &ctx.goal {
            println!("  {} {}", "Goal:".cyan(), goal);
        }
    }

    // Deployment info
    if let Some(deployment) = &state.deployment {
        if let Some(url) = &deployment.url {
            println!();
            println!("{}", "Deployment".bold());
            println!("  {} {}", "URL:".cyan(), url);
        }
        if let Some(status) = &deployment.status {
            println!("  {} {}", "Status:".cyan(), status);
        }
    }

    // Container info
    if let Some(container) = &state.container_info {
        if container.frontend_url.is_some() || container.backend_url.is_some() {
            println!();
            println!("{}", "Preview".bold());
            if let Some(url) = &container.frontend_url {
                println!("  {} {}", "Frontend:".cyan(), url);
            }
            if let Some(url) = &container.backend_url {
                println!("  {} {}", "Backend:".cyan(), url);
            }
            if let Some(healthy) = container.is_healthy {
                println!(
                    "  {} {}",
                    "Healthy:".cyan(),
                    if healthy { "Yes" } else { "No" }
                );
            }
        }
    }

    // GitHub
    if let Some(true) = state.github_repo_created {
        if let Some(url) = &state.github_repo_url {
            println!();
            println!("{}", "GitHub".bold());
            println!("  {} {}", "Repository:".cyan(), url);
        }
    }

    // Tests
    if let Some(ran) = state.tests_ran {
        if ran > 0 {
            println!();
            println!("{}", "Tests".bold());
            println!(
                "  {} passed, {} failed ({} total)",
                state.tests_passed.unwrap_or(0).to_string().green(),
                state.tests_failed.unwrap_or(0).to_string().red(),
                ran
            );
        }
    }

    // Files count
    if !state.files.is_empty() {
        println!();
        println!("{} {} files generated", "Files:".cyan(), state.files.len());
    }

    // Recent messages
    if !state.messages.is_empty() {
        println!();
        println!("{}", "Recent Messages".bold());
        for msg in state.messages.iter().rev().take(3).rev() {
            let type_label = match msg.message_type.as_str() {
                "user" => "You".blue(),
                "assistant" => "AI".green(),
                "system" => "System".yellow(),
                "tool" => "Tool".magenta(),
                _ => msg.message_type.as_str().normal(),
            };
            if let Some(content) = &msg.content {
                let truncated = if content.len() > 100 {
                    format!("{}...", &content[..100])
                } else {
                    content.clone()
                };
                println!("  {} {}", type_label, truncated.dimmed());
            }
        }
    }
}
