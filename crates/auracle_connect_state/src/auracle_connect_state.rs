//! Honest decision logic for the Connect surface — the engine connect-check
//! verdict, gpui-free so it can be unit-tested without rendering.
//!
//! The Connect modal (`auracle_connect`) is the single auth-handoff front door
//! for the IDE. Honesty is paramount: it must never read "connected" unless the
//! engine truly answered and accepted the key, an in-flight test is never green,
//! an unreachable AI agent never reads as a fully-ready setup, and nothing is
//! fabricated when a field the engine should report is missing.
//!
//! The I/O layer (`auracle_connect::test_connection`) performs the HTTP send and
//! JSON decode, then builds exactly one `ConnectProbe` from already-decoded
//! fields — this module never touches HTTP or JSON, mirroring how
//! `auracle_account::license_summary` takes `(&str, Option<i64>)` rather than a
//! `serde_json::Value`. The reducer turns that probe into the exact text, tone,
//! and retry affordance the modal shows, so text and colour can never disagree.

/// How a verdict reads at a glance, for the theme to colour at render time
/// (mirrors `auracle_account::LicenseTone` — render maps tone→Color, never a
/// colour literal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictTone {
    /// No judgement — e.g. a test in flight, which must never read as green.
    Neutral,
    /// Engine answered, key accepted, agent reachable: a clean success.
    Positive,
    /// Worth attention but not broken — e.g. engine+key fine but the AI agent
    /// leg is unreachable.
    Caution,
    /// A problem the user should act on — unreachable engine, rejected key, or
    /// an engine error.
    Negative,
}

/// The classified result of the connect-check probe, with engine I/O already
/// done. The I/O layer builds exactly one of these; the reducer never touches
/// HTTP or JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectProbe {
    /// Transport failed — the engine couldn't be reached at all. `detail` is a
    /// short plain phrase crafted by the I/O layer, NOT a raw `anyhow` chain.
    Unreachable { detail: String },
    /// The engine answered but rejected the key (HTTP 401/302).
    KeyRejected,
    /// The engine answered with a non-success, non-auth status.
    EngineError { status: u16 },
    /// The engine answered 2xx; fields read defensively from the JSON body.
    Ok {
        /// `None` ⇒ rendered as "unknown", never a fabricated version.
        engine_version: Option<String>,
        /// `None` ⇒ rendered as "none yet", never an invented broker name.
        active_broker: Option<String>,
        /// Whether the engine could reach the AI agent (MCP) leg.
        agent_reachable: bool,
        /// `None`/empty ⇒ a generic phrase, never a fabricated detail.
        agent_detail: Option<String>,
    },
}

/// The exact text + tone + retry affordance the modal renders for a finished
/// verdict. One struct so text and colour can never disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub message: String,
    pub tone: VerdictTone,
    pub retryable: bool,
}

/// What the verdict slot renders right now. The modal's render is a thin match
/// over this. We deliberately do not reuse `auracle_view_state::ViewState<T>`:
/// the connect probe is a one-shot action (Idle/Testing/Done), not a fetch-of-T
/// with empty/ready semantics, so a `ViewState` would need a fake `Empty` branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectView {
    /// No test run yet — render nothing (the verdict slot is absent).
    Idle,
    /// A test is in flight — render a NEUTRAL in-progress row, never green.
    Testing,
    /// A finished verdict.
    Done(Verdict),
}

/// The default engine address, single-sourced so the I/O layer and the editor
/// placeholder can never drift. The I/O layer uses this when the URL field is
/// blank; the editor shows it as placeholder text rather than a committed value,
/// so an untouched field is honestly empty rather than a fabricated entry.
pub const DEFAULT_ENGINE_URL: &str = "http://127.0.0.1:1969";

/// Classify a non-success HTTP status into the matching non-Ok probe variant.
/// Pulled out so the "401 or 302 ⇒ key rejected" rule is unit-tested. Returns
/// `None` for a success status — the caller then builds `ConnectProbe::Ok` from
/// the parsed body.
///
/// 302 counts as a rejected key because the engine's legacy `/ui` auth answers
/// an unaccepted key with a redirect to its login page rather than a 401.
pub fn classify_status(status: u16) -> Option<ConnectProbe> {
    if (200..300).contains(&status) {
        return None;
    }
    if status == 401 || status == 302 {
        return Some(ConnectProbe::KeyRejected);
    }
    Some(ConnectProbe::EngineError { status })
}

/// Map a classified probe to the honest verdict — text, tone, and whether Retry
/// applies. This is the whole honesty contract of the surface, now testable:
///
///   * `Unreachable` / `EngineError`  ⇒ Negative, retryable
///   * `KeyRejected`                  ⇒ Negative, retryable (re-test after the
///                                      key is fixed)
///   * `Ok` + agent reachable         ⇒ Positive, NOT retryable
///   * `Ok` + agent unreachable       ⇒ Caution (NOT Positive) + a second
///                                      sentence of truth about the agent leg,
///                                      NOT retryable (engine+key are fine; a
///                                      re-test won't fix the MCP leg, so the
///                                      message tells the user where to look)
pub fn verdict_for(probe: &ConnectProbe) -> Verdict {
    match probe {
        ConnectProbe::Unreachable { detail } => Verdict {
            message: format!(
                "Couldn't reach the engine ({detail}). Check the address and \
                 that your engine is running."
            ),
            tone: VerdictTone::Negative,
            retryable: true,
        },
        ConnectProbe::KeyRejected => Verdict {
            message: "The engine answered, but your key wasn't accepted. Check \
                      the key and test again."
                .into(),
            tone: VerdictTone::Negative,
            retryable: true,
        },
        ConnectProbe::EngineError { status } => Verdict {
            message: format!(
                "The engine answered with an error (status {status}). Try again \
                 in a moment."
            ),
            tone: VerdictTone::Negative,
            retryable: true,
        },
        ConnectProbe::Ok {
            engine_version,
            active_broker,
            agent_reachable,
            agent_detail,
        } => {
            // Honesty: never fabricate. A missing version reads "unknown" and a
            // missing broker reads "none yet" rather than an invented value.
            let version = engine_version.as_deref().unwrap_or("unknown");
            let active = active_broker.as_deref().unwrap_or("none yet");
            let mut message = format!(
                "Connected — engine v{version} is up and your key works \
                 (active broker: {active})."
            );
            if *agent_reachable {
                message.push_str(" Your AI agent is reachable.");
                Verdict {
                    message,
                    tone: VerdictTone::Positive,
                    retryable: false,
                }
            } else {
                // Surface the agent leg as a separate fact so an unreachable
                // agent never reads as a fully-ready setup. A missing detail
                // falls back to a generic phrase rather than a fabricated one.
                let detail = agent_detail
                    .as_deref()
                    .filter(|detail| !detail.is_empty())
                    .unwrap_or("the engine couldn't reach the MCP agent server");
                message.push_str(&format!(
                    " Note: the AI agent isn't reachable yet ({detail})."
                ));
                Verdict {
                    message,
                    tone: VerdictTone::Caution,
                    retryable: false,
                }
            }
        }
    }
}

/// The render decision: `Idle` (no test) / `Testing` (in flight) / `Done`
/// verdict. `testing` is true while the probe task is running; `probe` is `Some`
/// once it returns. Keeps the "in-flight is neutral" rule out of the render
/// path: while a test runs we always report `Testing`, even if a stale probe
/// from a previous run is still present.
pub fn connect_view(testing: bool, probe: Option<&ConnectProbe>) -> ConnectView {
    if testing {
        return ConnectView::Testing;
    }
    match probe {
        None => ConnectView::Idle,
        Some(probe) => ConnectView::Done(verdict_for(probe)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_probe(agent_reachable: bool) -> ConnectProbe {
        ConnectProbe::Ok {
            engine_version: Some("1.2.3".into()),
            active_broker: Some("ibkr".into()),
            agent_reachable,
            agent_detail: None,
        }
    }

    #[test]
    fn default_engine_url_is_loopback() {
        // Single source of the default so the I/O layer and the editor
        // placeholder can never drift apart.
        assert_eq!(DEFAULT_ENGINE_URL, "http://127.0.0.1:1969");
    }

    #[test]
    fn status_401_is_key_rejected() {
        assert_eq!(classify_status(401), Some(ConnectProbe::KeyRejected));
    }

    #[test]
    fn status_302_is_key_rejected() {
        // A legacy `/ui` auth redirect to the login page counts as a rejected
        // key, not a generic engine error.
        assert_eq!(classify_status(302), Some(ConnectProbe::KeyRejected));
    }

    #[test]
    fn status_500_is_engine_error_carrying_status() {
        assert_eq!(
            classify_status(500),
            Some(ConnectProbe::EngineError { status: 500 })
        );
    }

    #[test]
    fn success_status_is_not_classified() {
        // A 2xx isn't an error variant — the caller builds Ok from the body.
        assert_eq!(classify_status(200), None);
        assert_eq!(classify_status(204), None);
    }

    #[test]
    fn reachable_agent_reads_positive_and_not_retryable() {
        let verdict = verdict_for(&ok_probe(true));
        assert_eq!(verdict.tone, VerdictTone::Positive);
        assert!(!verdict.retryable);
        assert!(verdict.message.contains("Your AI agent is reachable."));
    }

    #[test]
    fn unreachable_agent_reads_caution_not_positive() {
        // The agent leg is a separate fact: engine+key fine, but the agent is
        // not, so this must NOT read as a clean (Positive) success.
        let verdict = verdict_for(&ok_probe(false));
        assert_eq!(verdict.tone, VerdictTone::Caution);
        assert_ne!(verdict.tone, VerdictTone::Positive);
        assert!(verdict.message.contains("the AI agent isn't reachable yet"));
        // Distinct from the all-green path — it never claims the agent is up.
        assert!(!verdict.message.contains("Your AI agent is reachable."));
        // Engine+key are fine; a re-test won't fix the MCP leg.
        assert!(!verdict.retryable);
    }

    #[test]
    fn unreachable_agent_with_detail_surfaces_the_engine_phrase() {
        let probe = ConnectProbe::Ok {
            engine_version: Some("1.0.0".into()),
            active_broker: Some("ibkr".into()),
            agent_reachable: false,
            agent_detail: Some("connection refused on port 1968".into()),
        };
        let verdict = verdict_for(&probe);
        assert!(verdict.message.contains("connection refused on port 1968"));
    }

    #[test]
    fn missing_engine_version_reads_unknown_not_fabricated() {
        let probe = ConnectProbe::Ok {
            engine_version: None,
            active_broker: Some("ibkr".into()),
            agent_reachable: true,
            agent_detail: None,
        };
        let verdict = verdict_for(&probe);
        assert!(verdict.message.contains("engine vunknown"));
    }

    #[test]
    fn missing_active_broker_reads_none_yet() {
        let probe = ConnectProbe::Ok {
            engine_version: Some("1.0.0".into()),
            active_broker: None,
            agent_reachable: true,
            agent_detail: None,
        };
        let verdict = verdict_for(&probe);
        assert!(verdict.message.contains("active broker: none yet"));
    }

    #[test]
    fn empty_agent_detail_falls_back_to_generic_phrase() {
        // An empty string is as absent as None — fall back, never show "()".
        let probe = ConnectProbe::Ok {
            engine_version: Some("1.0.0".into()),
            active_broker: Some("ibkr".into()),
            agent_reachable: false,
            agent_detail: Some(String::new()),
        };
        let verdict = verdict_for(&probe);
        assert!(
            verdict
                .message
                .contains("the engine couldn't reach the MCP agent server")
        );
    }

    #[test]
    fn unreachable_engine_is_negative_and_retryable() {
        let verdict = verdict_for(&ConnectProbe::Unreachable {
            detail: "connection refused".into(),
        });
        assert_eq!(verdict.tone, VerdictTone::Negative);
        assert!(verdict.retryable);
        // The crafted detail is surfaced, not a raw anyhow chain.
        assert!(verdict.message.contains("connection refused"));
    }

    #[test]
    fn key_rejected_is_negative_and_retryable() {
        let verdict = verdict_for(&ConnectProbe::KeyRejected);
        assert_eq!(verdict.tone, VerdictTone::Negative);
        assert!(verdict.retryable);
    }

    #[test]
    fn engine_error_is_negative_retryable_and_names_the_status() {
        let verdict = verdict_for(&ConnectProbe::EngineError { status: 503 });
        assert_eq!(verdict.tone, VerdictTone::Negative);
        assert!(verdict.retryable);
        assert!(verdict.message.contains("503"));
    }

    #[test]
    fn in_flight_is_testing_and_neutral() {
        // The in-flight render decision is neutral, never green — and a test in
        // flight reports Testing even if a stale probe is still around.
        assert_eq!(connect_view(true, None), ConnectView::Testing);
        assert_eq!(
            connect_view(true, Some(&ok_probe(true))),
            ConnectView::Testing
        );
    }

    #[test]
    fn no_test_yet_is_idle() {
        assert_eq!(connect_view(false, None), ConnectView::Idle);
    }

    #[test]
    fn finished_probe_is_done_with_its_verdict() {
        let probe = ok_probe(true);
        assert_eq!(
            connect_view(false, Some(&probe)),
            ConnectView::Done(verdict_for(&probe))
        );
    }

    #[test]
    fn verdict_text_and_tone_never_disagree() {
        // The V5 guard: text and colour are decided together, so for each probe
        // the (message, tone) pair is fixed as a unit. Green (Positive) is
        // reachable ONLY from an Ok with a reachable agent.
        let cases = [
            (
                ConnectProbe::Unreachable {
                    detail: "engine down".into(),
                },
                VerdictTone::Negative,
            ),
            (ConnectProbe::KeyRejected, VerdictTone::Negative),
            (
                ConnectProbe::EngineError { status: 500 },
                VerdictTone::Negative,
            ),
            (ok_probe(true), VerdictTone::Positive),
            (ok_probe(false), VerdictTone::Caution),
        ];
        for (probe, expected_tone) in cases {
            let verdict = verdict_for(&probe);
            assert_eq!(verdict.tone, expected_tone, "tone mismatch for {probe:?}");
            // The only path to a green verdict is a reachable agent.
            if verdict.tone == VerdictTone::Positive {
                assert!(verdict.message.contains("Your AI agent is reachable."));
            }
            assert!(!verdict.message.is_empty());
        }
    }
}
