//! Native Auracle commands — drive the agent from the command palette.
//!
//! Each command opens (or focuses) the Ask-Auracle agent panel, starts a fresh
//! thread seeded with a preset request, and auto-submits it. The agent then
//! calls the engine's MCP tools (research_scan, run_backtest_now, …) to do the
//! work. These migrate the most-used Houston web workflows into one-keystroke
//! IDE commands without rebuilding each as a bespoke panel — the agent + MCP
//! are the engine.
//!
//! Requires the Connect flow to have run (engine URL + API key saved) so the
//! engine's MCP server is reachable; otherwise the prompt lands in a thread
//! with no tools and the agent says so.

use std::sync::Arc;

use agent_ui::AgentPanel;
use futures::AsyncReadExt as _;
use gpui::{App, Context, Window, actions};
use workspace::Workspace;

actions!(
    auracle,
    [
        /// Scan recent arXiv research and synthesize futures-trading-system ideas.
        ResearchIdeas,
        /// Pick a strategy and run a vectorized backtest (Sharpe, drawdown, return).
        RunBacktest,
        /// Run a rolling out-of-sample walk-forward and get a regime diagnosis.
        WalkForward,
        /// One-shot pre-market readiness check (broker, NLV, positions, recent jobs).
        PreMarketCheck,
        /// Open the tearsheet for a recent backtest or live job.
        OpenTearsheet,
        /// Browse the bundled example strategies.
        BrowseTemplates,
        /// Clone a bundled example strategy into your strategies/ folder.
        CloneTemplate,
        /// Pre-flight a strategy before deploying (imports, data, broker, license).
        RunPreflight,
        /// Download and store historical daily bars for a symbol.
        IngestData,
        /// Auto-draft a deployment manifest for a strategy.
        DraftManifest,
        /// Validate a deployment manifest and list any issues.
        ValidateManifest,
        /// Backtest a deployment manifest without any live or paper broker call.
        BacktestManifest,
        /// Propose strategy variants, prove each out-of-sample, and recommend at most one.
        ProposeAndProve,
        /// Two-question intake, then build a real strategy and show its backtest.
        StartInterview,
    ]
);

const RESEARCH_IDEAS: &str = "Scan recent arXiv papers on futures trading, term \
structure, carry, and trend-following / CTA systems using the research_scan tool. \
Return the most relevant papers with their abstracts, then synthesize at least \
three concrete futures-trading-system ideas I could backtest in Auracle, and \
write them up as a markdown research note.";

const RUN_BACKTEST: &str = "List the available strategies, ask me which one to \
test, then run a full vectorized backtest. Show the Sharpe ratio, maximum \
drawdown, total return, and the final target-weight vector.";

const WALK_FORWARD: &str = "Run an out-of-sample rolling walk-forward backtest \
with a train/test split using the run_walkforward tool. Show per-fold Sharpe and \
return, then give a one-sentence regime diagnosis summarising the aggregate \
out-of-sample win-rate and mean Sharpe.";

const PRE_MARKET_CHECK: &str = "Run a one-shot readiness check with the \
premarket_check tool: broker connectivity, net liquidation value, current \
positions, open orders, and the last 24 hours of job outcomes. Put any failures \
at the top.";

const OPEN_TEARSHEET: &str = "List the 10 most recent jobs. Ask me which one to \
open, then show its full summary: status, timing, any error message, and a \
plain-English account of what happened and why.";

const BROWSE_TEMPLATES: &str = "List all bundled example strategies with their \
titles and a one-sentence summary of each. Ask me which one I'd like to see in \
detail.";

const CLONE_TEMPLATE: &str = "List the bundled example strategies. Ask me which \
one to clone, then copy it into strategies/ as a new class and show me the file \
path and strategy path so I can run a backtest right away.";

const RUN_PREFLIGHT: &str = "List the available strategies and ask which one to \
check. Run pre-flight validation (code imports, universe data availability, \
broker auth, license tier) and show a pass / warn / fail verdict for each check.";

const INGEST_DATA: &str = "Ask me for a symbol, exchange, and date range, then \
download and store the daily OHLCV bars into the Auracle database. Confirm how \
many bars were inserted.";

const DRAFT_MANIFEST: &str = "List the available strategies and brokers, ask me \
which to use, then draft a deployment manifest with sensible defaults for the \
cron schedule, risk limits, and position sizing. Flag any validation issues.";

const VALIDATE_MANIFEST: &str = "Ask me which manifest file to validate, then \
check its structure, broker registration, universe coverage, and risk settings. \
List every issue you find with a concrete fix.";

const BACKTEST_MANIFEST: &str = "Ask me for a manifest file path, then run its \
strategy as a vectorized backtest with no live or paper broker call. Show the \
target weights and performance stats.";

const PROPOSE_AND_PROVE: &str = "Take the strategy in the active editor (or ask \
me which one). Propose two or three concrete variations — a different parameter \
value, an added exit rule, or a simpler signal. For EACH variation, run a \
backtest AND a rolling out-of-sample walk-forward using the backtest and \
run_walkforward tools, and present a side-by-side comparison versus the current \
strategy: in-sample Sharpe, out-of-sample Sharpe, max drawdown, and turnover. \
Recommend at most one change, and ONLY if it improves out-of-sample robustness — \
never recommend something that just looks better in-sample. Do not edit my file \
or deploy anything: show the comparison and the exact diff you would apply, and \
let me approve it.";

const START_INTERVIEW: &str = "Run a short strategy intake, then build me \
something real. Ask me at most two quick questions — what markets I follow and \
how often I want to trade — then propose one concrete, defensible strategy idea, \
write it as a one-file strategy with a `# %%spec` header that compiles to a \
Strategy subclass, and run a fast backtest so I can see an equity curve. Stay \
honest: if the edge looks weak out-of-sample, say so plainly. I can refine it by \
chat from there.";

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|ws, _: &ResearchIdeas, window, cx| {
            run_prompt(ws, "Research ideas", RESEARCH_IDEAS, window, cx);
        });
        workspace.register_action(|ws, _: &RunBacktest, window, cx| {
            run_prompt(ws, "Run backtest", RUN_BACKTEST, window, cx);
        });
        workspace.register_action(|ws, _: &WalkForward, window, cx| {
            run_prompt(ws, "Walk-forward test", WALK_FORWARD, window, cx);
        });
        workspace.register_action(|ws, _: &PreMarketCheck, window, cx| {
            run_prompt(ws, "Pre-market check", PRE_MARKET_CHECK, window, cx);
        });
        workspace.register_action(|ws, _: &OpenTearsheet, window, cx| {
            run_prompt(ws, "Open tearsheet", OPEN_TEARSHEET, window, cx);
        });
        workspace.register_action(|ws, _: &BrowseTemplates, window, cx| {
            run_prompt(ws, "Browse templates", BROWSE_TEMPLATES, window, cx);
        });
        workspace.register_action(|ws, _: &CloneTemplate, window, cx| {
            run_prompt(ws, "Clone template", CLONE_TEMPLATE, window, cx);
        });
        workspace.register_action(|ws, _: &RunPreflight, window, cx| {
            deploy_with_broker_gate(ws, "Run preflight", RUN_PREFLIGHT, window, cx);
        });
        workspace.register_action(|ws, _: &IngestData, window, cx| {
            run_prompt(ws, "Ingest data", INGEST_DATA, window, cx);
        });
        workspace.register_action(|ws, _: &DraftManifest, window, cx| {
            deploy_with_broker_gate(ws, "Draft manifest", DRAFT_MANIFEST, window, cx);
        });
        workspace.register_action(|ws, _: &ValidateManifest, window, cx| {
            run_prompt(ws, "Validate manifest", VALIDATE_MANIFEST, window, cx);
        });
        workspace.register_action(|ws, _: &BacktestManifest, window, cx| {
            run_prompt(ws, "Backtest manifest", BACKTEST_MANIFEST, window, cx);
        });
        workspace.register_action(|ws, _: &ProposeAndProve, window, cx| {
            run_prompt(ws, "Propose and prove", PROPOSE_AND_PROVE, window, cx);
        });
        workspace.register_action(|ws, _: &StartInterview, window, cx| {
            run_prompt(ws, "Start interview", START_INTERVIEW, window, cx);
        });
    })
    .detach();
}

/// Focus the agent panel and start an auto-submitted thread with `prompt`.
/// No-ops if the agent panel isn't present (the engine isn't connected yet).
fn run_prompt(
    workspace: &mut Workspace,
    title: &str,
    prompt: &str,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(panel) = workspace.focus_panel::<AgentPanel>(window, cx) else {
        return;
    };
    panel.update(cx, |panel, cx| {
        panel.start_auracle_prompt(title.into(), prompt.to_string(), window, cx);
    });
}

/// Deploy-path commands run through here so the broker connection wizard
/// appears in the deploy flow when no execution broker is configured yet —
/// the same wizard as Settings → Connections, opened in place. If a broker
/// is configured, the agent prompt runs as usual. The check is best effort:
/// any error (engine unreachable, etc.) falls through to the prompt, where
/// the agent's own preflight reports the real state.
fn deploy_with_broker_gate(
    _workspace: &mut Workspace,
    title: &'static str,
    prompt: &'static str,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let http = cx.http_client();
    cx.spawn_in(window, async move |workspace, cx| {
        let configured = broker_configured(http).await;
        workspace
            .update_in(cx, |workspace, window, cx| {
                if configured {
                    run_prompt(workspace, title, prompt, window, cx);
                } else {
                    // No execution broker yet — open the connect wizard so
                    // the user can connect right here, then deploy.
                    window.dispatch_action(
                        Box::new(auracle_connections::OpenBrokerWizard),
                        cx,
                    );
                }
            })
            .ok();
    })
    .detach();
}

/// True only if the engine reports a configured active execution broker.
/// Best effort: a missing key, unreachable engine, or unexpected response
/// returns false so the wizard is offered rather than silently skipped.
async fn broker_configured(http: Arc<dyn http_client::HttpClient>) -> bool {
    let config = auracle_connect::load_config();
    let key = config.api_key.clone().unwrap_or_default();
    if key.trim().is_empty() {
        return false;
    }
    let url = config
        .engine_url
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: anyhow::Result<bool> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/capabilities"))
            .header("X-API-Key", key.clone())
            .header("Cookie", format!("auracle_session={key}"))
            .body(http_client::AsyncBody::default())?;
        let mut response = http.send(request).await?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        Ok(value
            .get("active_broker")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false))
    }
    .await;
    attempt.unwrap_or(false)
}
