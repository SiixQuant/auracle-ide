//! Honest presentation of the engine status chip, for the native status bar.
//!
//! The status-bar poll extracts already-parsed facts from the engine's
//! capabilities payload; this module turns those facts into the exact chip text,
//! tone, and tooltip the bar draws — without ever touching http or serde. It is
//! gpui-free so the four states (and their honesty rules) are unit-tested without
//! the graphics toolchain. Mirrors `auracle_account`'s tone+summary shape.
//!
//! Honesty rules enforced here:
//! - `Good` tone only when actually `Connected` — never "connected" from a stale
//!   or in-flight poll.
//! - `broker == None` reads "no broker yet"; we never render a broker literally
//!   named "none yet" as if one were active.
//! - the mode word is "live ok" only when the license actually allows it.

/// Glance tone for a status chip (theme-coloured at render).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipTone {
    Muted,
    Checking,
    Good,
    Bad,
}

/// The parsed engine facts the chip decides over — already extracted from JSON
/// by the gpui poll, so the reducer never touches http/serde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineFacts {
    /// No api key.
    NotConnected,
    /// Key present, first poll in flight.
    Checking,
    /// Fetch failed (no answer / network error) — the engine may be stopped.
    Unreachable,
    /// The engine answered but rejected the api key (401/403): reachable, but
    /// the key is expired or from a different install. Distinct from
    /// `Unreachable` so the chip doesn't blame the engine being down for what
    /// is really a credential problem.
    InvalidCredentials,
    Connected {
        /// None => no broker active yet.
        broker: Option<String>,
        live_allowed: bool,
        /// Engine's plain sentence ("" if none).
        capability_plain: String,
    },
}

/// Exactly what the chip renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipView {
    pub label: String,
    pub tone: ChipTone,
    pub tooltip: String,
}

/// The license-note tooltip suffix, moved verbatim from the old render.
/// Distinct wording for whether real-money trading is licensed, so the tooltip
/// never overstates what the license allows.
fn license_note(live_allowed: bool) -> &'static str {
    if live_allowed {
        "Real-money trading is allowed by your license — \
         paper stays the default."
    } else {
        "Real-money trading is not yet enabled on your \
         license; paper trading works."
    }
}

/// Map already-parsed engine facts to the honest chip text, tone, and tooltip.
/// Never fabricates a broker name and never claims `Good`/"live" it can't prove.
pub fn chip_view(facts: EngineFacts) -> ChipView {
    match facts {
        EngineFacts::NotConnected => ChipView {
            label: "engine: not connected".to_string(),
            tone: ChipTone::Muted,
            tooltip: "Your Auracle engine isn't connected yet. Click to connect.".to_string(),
        },
        EngineFacts::Checking => ChipView {
            label: "engine: checking…".to_string(),
            tone: ChipTone::Checking,
            tooltip: "Asking your engine how it's doing — usually a moment.".to_string(),
        },
        EngineFacts::Unreachable => ChipView {
            label: "engine: unreachable".to_string(),
            tone: ChipTone::Bad,
            tooltip: "Your engine didn't answer. It may be stopped — start it, \
                      or click to check the connection details."
                .to_string(),
        },
        EngineFacts::InvalidCredentials => ChipView {
            label: "engine: key rejected".to_string(),
            tone: ChipTone::Bad,
            tooltip: "Your engine answered but rejected the API key — it may be \
                      expired or from a different install. Click to reconnect \
                      with a fresh key."
                .to_string(),
        },
        EngineFacts::Connected {
            broker,
            live_allowed,
            capability_plain,
        } => {
            // Glance text answers "can I go live?" in one word; the tooltip
            // carries the engine's full plain sentence (when present) plus the
            // license note.
            let note = license_note(live_allowed);
            let tooltip = if capability_plain.is_empty() {
                note.to_string()
            } else {
                format!("{capability_plain} {note}")
            };
            // "on" (not "live") so the word never collides with live trading —
            // the mode token owns that meaning.
            let label = match broker {
                // No broker active: say so plainly, never render a broker
                // literally named "none yet" (fixes the fabricated-broker read).
                None => "engine: on · no broker yet".to_string(),
                Some(broker) => {
                    let mode = if live_allowed {
                        "live ok"
                    } else {
                        "paper only"
                    };
                    format!("engine: on · broker: {broker} · {mode}")
                }
            };
            ChipView {
                label,
                // Good only because we are actually Connected.
                tone: ChipTone::Good,
                tooltip,
            }
        }
    }
}

/// Parsed QuantConnect connection facts, extracted from
/// `GET /ui/api/quantconnect/connection` by the gpui poll (never http/serde here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QcFacts {
    /// No QC credentials configured, or the engine endpoint isn't deployed.
    NotConnected,
    /// Credentials present, probe in flight.
    Checking,
    /// Authenticated: the (non-secret) user id + how many projects are visible.
    Connected { user_id: String, projects: u32 },
}

/// Map QuantConnect facts to the honest chip text, tone, and tooltip. `Good`
/// only when actually connected — never claims a connection from a stale or
/// in-flight probe, and never echoes the API token.
pub fn qc_chip_view(facts: QcFacts) -> ChipView {
    match facts {
        QcFacts::NotConnected => ChipView {
            label: "QuantConnect: off".to_string(),
            tone: ChipTone::Muted,
            tooltip: "QuantConnect isn't connected. Add a user ID + API token in \
                      Settings to import your LEAN strategies."
                .to_string(),
        },
        QcFacts::Checking => ChipView {
            label: "QuantConnect: checking…".to_string(),
            tone: ChipTone::Checking,
            tooltip: "Checking your QuantConnect connection — usually a moment.".to_string(),
        },
        QcFacts::Connected { user_id, projects } => {
            let plural = if projects == 1 { "project" } else { "projects" };
            ChipView {
                label: "QuantConnect: connected".to_string(),
                tone: ChipTone::Good,
                tooltip: format!(
                    "Connected to QuantConnect as user {user_id} · {projects} {plural} \
                     available to import."
                ),
            }
        }
    }
}

/// One non-engine connection's resolved status, as the rollup chip sees it.
/// The engine connection is deliberately excluded — it keeps its own always-
/// visible chip — so the rollup only ever summarises brokers, data sources, and
/// QuantConnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionStatus {
    /// Display name, e.g. "QuantConnect" or "IBKR".
    pub name: String,
    pub tone: ChipTone,
    /// True only when the connection is actually connected and healthy.
    pub connected: bool,
}

/// Collapse N non-engine connections into one rollup chip. The honesty rules:
/// - `Good` only when every member is connected (one in-flight or off member
///   pulls the rollup off `Good`).
/// - otherwise the worst member tone surfaces, `Bad` > `Checking` > `Muted`.
/// - an empty set reads "no connections yet" in muted, never a fake `Good`.
///
/// The label stays compact for the status bar (`connections: <connected>/<total>`)
/// and the tooltip carries the honest per-state breakdown.
pub fn rollup_chip_view(members: &[ConnectionStatus]) -> ChipView {
    if members.is_empty() {
        return ChipView {
            label: "connections: none".to_string(),
            tone: ChipTone::Muted,
            tooltip: "No broker, data, or QuantConnect connections yet. \
                      Click to add one."
                .to_string(),
        };
    }

    let total = members.len();
    let mut connected = 0usize;
    let mut off = 0usize;
    let mut checking = 0usize;
    let mut unreachable = 0usize;
    let mut any_bad = false;
    let mut any_checking = false;
    for member in members {
        if member.connected {
            connected += 1;
            continue;
        }
        match member.tone {
            ChipTone::Bad => {
                unreachable += 1;
                any_bad = true;
            }
            ChipTone::Checking => {
                checking += 1;
                any_checking = true;
            }
            // A not-connected member with any other tone is simply "off".
            ChipTone::Muted | ChipTone::Good => off += 1,
        }
    }

    // Worst tone wins; `Good` only when nothing is off/checking/unreachable.
    let tone = if any_bad {
        ChipTone::Bad
    } else if any_checking {
        ChipTone::Checking
    } else if connected == total {
        ChipTone::Good
    } else {
        ChipTone::Muted
    };

    // Tooltip: only mention the states that are actually present, so it never
    // pads with "0 unreachable" noise.
    let mut parts = Vec::new();
    if connected > 0 {
        parts.push(format!("{connected} connected"));
    }
    if off > 0 {
        parts.push(format!("{off} off"));
    }
    if checking > 0 {
        parts.push(format!("{checking} checking"));
    }
    if unreachable > 0 {
        parts.push(format!("{unreachable} unreachable"));
    }

    ChipView {
        label: format!("connections: {connected}/{total}"),
        tone,
        tooltip: format!("{}. Click to manage connections.", parts.join(" \u{b7} ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, tone: ChipTone, connected: bool) -> ConnectionStatus {
        ConnectionStatus {
            name: name.to_string(),
            tone,
            connected,
        }
    }

    #[test]
    fn not_connected_is_muted_with_connect_hint() {
        let view = chip_view(EngineFacts::NotConnected);
        assert_eq!(view.label, "engine: not connected");
        assert_eq!(view.tone, ChipTone::Muted);
        assert!(view.tooltip.contains("Click to connect"));
    }

    #[test]
    fn checking_is_checking_tone_not_good() {
        let view = chip_view(EngineFacts::Checking);
        assert_eq!(view.tone, ChipTone::Checking);
        assert_ne!(view.tone, ChipTone::Good);
    }

    #[test]
    fn unreachable_is_bad() {
        let view = chip_view(EngineFacts::Unreachable);
        assert_eq!(view.label, "engine: unreachable");
        assert_eq!(view.tone, ChipTone::Bad);
    }

    #[test]
    fn invalid_credentials_is_distinct_from_unreachable() {
        // A rejected key is reachable-but-unauthorized; the chip must say so
        // (re-auth), never blame the engine being down.
        let view = chip_view(EngineFacts::InvalidCredentials);
        assert_eq!(view.label, "engine: key rejected");
        assert_eq!(view.tone, ChipTone::Bad);
        assert_ne!(view.label, chip_view(EngineFacts::Unreachable).label);
        assert!(view.tooltip.contains("rejected the API key"));
        assert!(!view.tooltip.contains("didn't answer"));
    }

    #[test]
    fn connected_no_broker_says_no_broker_yet_not_a_named_broker() {
        let view = chip_view(EngineFacts::Connected {
            broker: None,
            live_allowed: false,
            capability_plain: String::new(),
        });
        assert!(view.label.contains("no broker yet"));
        assert!(!view.label.contains("broker: none yet"));
        assert_eq!(view.tone, ChipTone::Good);
    }

    #[test]
    fn connected_live_allowed_reads_live_ok() {
        let view = chip_view(EngineFacts::Connected {
            broker: Some("ibkr".to_string()),
            live_allowed: true,
            capability_plain: String::new(),
        });
        assert!(view.label.contains("live ok"));
        assert!(!view.label.contains("paper only"));
    }

    #[test]
    fn connected_not_allowed_reads_paper_only() {
        let view = chip_view(EngineFacts::Connected {
            broker: Some("ibkr".to_string()),
            live_allowed: false,
            capability_plain: String::new(),
        });
        assert!(view.label.contains("paper only"));
        assert!(!view.label.contains("live ok"));
    }

    #[test]
    fn tooltip_prefixes_plain_sentence_when_present() {
        let view = chip_view(EngineFacts::Connected {
            broker: Some("ibkr".to_string()),
            live_allowed: false,
            capability_plain: "You can paper trade US equities.".to_string(),
        });
        assert!(view.tooltip.starts_with("You can paper trade US equities."));
        assert!(
            view.tooltip
                .contains("Real-money trading is not yet enabled")
        );
    }

    #[test]
    fn tooltip_is_license_note_only_when_plain_empty() {
        let view = chip_view(EngineFacts::Connected {
            broker: Some("ibkr".to_string()),
            live_allowed: false,
            capability_plain: String::new(),
        });
        assert_eq!(
            view.tooltip,
            "Real-money trading is not yet enabled on your \
             license; paper trading works."
        );
    }

    #[test]
    fn live_ok_tooltip_says_paper_stays_default() {
        // Carries the existing nuance: even when live is licensed, paper is still
        // the default — the tooltip must keep saying so.
        let view = chip_view(EngineFacts::Connected {
            broker: Some("ibkr".to_string()),
            live_allowed: true,
            capability_plain: String::new(),
        });
        assert!(view.tooltip.contains("paper stays the default"));
    }

    #[test]
    fn qc_not_connected_is_muted_with_settings_hint() {
        let view = qc_chip_view(QcFacts::NotConnected);
        assert_eq!(view.label, "QuantConnect: off");
        assert_eq!(view.tone, ChipTone::Muted);
        assert!(view.tooltip.contains("Settings"));
    }

    #[test]
    fn qc_checking_is_checking_not_good() {
        assert_eq!(qc_chip_view(QcFacts::Checking).tone, ChipTone::Checking);
        assert_ne!(qc_chip_view(QcFacts::Checking).tone, ChipTone::Good);
    }

    #[test]
    fn qc_connected_is_good_with_user_and_count() {
        let view = qc_chip_view(QcFacts::Connected {
            user_id: "123456".to_string(),
            projects: 7,
        });
        assert_eq!(view.tone, ChipTone::Good);
        assert!(view.tooltip.contains("123456"));
        assert!(view.tooltip.contains("7 projects"));
    }

    #[test]
    fn qc_single_project_reads_singular() {
        let view = qc_chip_view(QcFacts::Connected {
            user_id: "1".to_string(),
            projects: 1,
        });
        assert!(view.tooltip.contains("1 project "));
        assert!(!view.tooltip.contains("1 projects"));
    }

    #[test]
    fn rollup_empty_is_muted_not_a_fake_good() {
        let view = rollup_chip_view(&[]);
        assert_eq!(view.label, "connections: none");
        assert_eq!(view.tone, ChipTone::Muted);
        assert_ne!(view.tone, ChipTone::Good);
    }

    #[test]
    fn rollup_all_connected_is_good() {
        let view = rollup_chip_view(&[
            member("IBKR", ChipTone::Good, true),
            member("QuantConnect", ChipTone::Good, true),
        ]);
        assert_eq!(view.label, "connections: 2/2");
        assert_eq!(view.tone, ChipTone::Good);
        assert!(view.tooltip.contains("2 connected"));
    }

    #[test]
    fn rollup_one_off_drops_below_good() {
        let view = rollup_chip_view(&[
            member("IBKR", ChipTone::Good, true),
            member("QuantConnect", ChipTone::Muted, false),
        ]);
        assert_eq!(view.label, "connections: 1/2");
        assert_eq!(view.tone, ChipTone::Muted);
        assert_ne!(view.tone, ChipTone::Good);
        assert!(view.tooltip.contains("1 connected"));
        assert!(view.tooltip.contains("1 off"));
    }

    #[test]
    fn rollup_any_unreachable_is_bad_and_wins_over_checking() {
        let view = rollup_chip_view(&[
            member("IBKR", ChipTone::Good, true),
            member("Alpaca", ChipTone::Checking, false),
            member("QuantConnect", ChipTone::Bad, false),
        ]);
        assert_eq!(view.tone, ChipTone::Bad);
        assert!(view.tooltip.contains("1 unreachable"));
        assert!(view.tooltip.contains("1 checking"));
    }

    #[test]
    fn rollup_checking_wins_when_no_bad() {
        let view = rollup_chip_view(&[
            member("IBKR", ChipTone::Good, true),
            member("QuantConnect", ChipTone::Checking, false),
        ]);
        assert_eq!(view.tone, ChipTone::Checking);
    }

    #[test]
    fn rollup_tooltip_omits_absent_states() {
        let view = rollup_chip_view(&[member("IBKR", ChipTone::Good, true)]);
        assert!(view.tooltip.contains("1 connected"));
        assert!(!view.tooltip.contains("off"));
        assert!(!view.tooltip.contains("checking"));
        assert!(!view.tooltip.contains("unreachable"));
    }
}
