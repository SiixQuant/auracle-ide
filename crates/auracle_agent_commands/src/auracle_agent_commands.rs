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

use agent_ui::AgentPanel;
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
            run_prompt(ws, "Run preflight", RUN_PREFLIGHT, window, cx);
        });
        workspace.register_action(|ws, _: &IngestData, window, cx| {
            run_prompt(ws, "Ingest data", INGEST_DATA, window, cx);
        });
        workspace.register_action(|ws, _: &DraftManifest, window, cx| {
            run_prompt(ws, "Draft manifest", DRAFT_MANIFEST, window, cx);
        });
        workspace.register_action(|ws, _: &ValidateManifest, window, cx| {
            run_prompt(ws, "Validate manifest", VALIDATE_MANIFEST, window, cx);
        });
        workspace.register_action(|ws, _: &BacktestManifest, window, cx| {
            run_prompt(ws, "Backtest manifest", BACKTEST_MANIFEST, window, cx);
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
