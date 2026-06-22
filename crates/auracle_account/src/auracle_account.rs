//! Honest presentation of an account's license, for the native Account page.
//!
//! The engine's `/ui/api/me` reports a license as a `status` plus, for an active
//! one, the days remaining. This module turns that into the exact text the
//! Account page shows — and, critically, never invents anything: an unknown
//! status reads "Unknown", and an active license whose day count the engine
//! didn't supply shows no fabricated number. The decision is gpui-free so it is
//! unit-tested without rendering. See `RUBRIC.md` in the `auracle_view_state`
//! crate (item 5, honesty).

/// How a license row reads at a glance, for the theme to colour at render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseTone {
    /// No judgement (e.g. community, or an unknown status we won't dramatise).
    Neutral,
    /// All good (perpetual, active).
    Positive,
    /// Worth attention but not broken.
    Caution,
    /// A problem the user should act on (expired).
    Negative,
}

/// The exact text the Account page shows for a license.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseSummary {
    pub label: String,
    pub detail: Option<String>,
    pub tone: LicenseTone,
}

/// "1 day left" / "N days left", pluralised honestly.
fn days_left_phrase(days: i64) -> String {
    if days == 1 {
        "1 day left".into()
    } else {
        format!("{days} days left")
    }
}

/// Map an engine license `status` (and, for an active one, `days_remaining`) to
/// the honest row text. Never fabricates: an unrecognised status reads
/// "Unknown", and a missing day count shows no invented number.
pub fn license_summary(status: &str, days_remaining: Option<i64>) -> LicenseSummary {
    match status {
        "perpetual" => LicenseSummary {
            label: "Perpetual".into(),
            detail: Some("Never expires".into()),
            tone: LicenseTone::Positive,
        },
        "active" => {
            let (detail, tone) = match days_remaining {
                Some(0) => (Some("Expires today".into()), LicenseTone::Caution),
                Some(n) => (Some(days_left_phrase(n)), LicenseTone::Positive),
                None => (None, LicenseTone::Positive),
            };
            LicenseSummary {
                label: "Active".into(),
                detail,
                tone,
            }
        }
        "expired" => LicenseSummary {
            label: "Expired".into(),
            detail: Some("Renew to restore access".into()),
            tone: LicenseTone::Negative,
        },
        "community" => LicenseSummary {
            label: "Community".into(),
            detail: None,
            tone: LicenseTone::Neutral,
        },
        // Honesty: never dress an unrecognised status as something it isn't.
        _ => LicenseSummary {
            label: "Unknown".into(),
            detail: None,
            tone: LicenseTone::Neutral,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn community_reads_as_community() {
        let summary = license_summary("community", None);
        assert_eq!(
            summary,
            LicenseSummary {
                label: "Community".into(),
                detail: None,
                tone: LicenseTone::Neutral,
            }
        );
    }

    #[test]
    fn perpetual_never_expires() {
        let summary = license_summary("perpetual", None);
        assert_eq!(
            summary,
            LicenseSummary {
                label: "Perpetual".into(),
                detail: Some("Never expires".into()),
                tone: LicenseTone::Positive,
            }
        );
    }

    #[test]
    fn active_shows_days_remaining() {
        let summary = license_summary("active", Some(42));
        assert_eq!(
            summary,
            LicenseSummary {
                label: "Active".into(),
                detail: Some("42 days left".into()),
                tone: LicenseTone::Positive,
            }
        );
    }

    #[test]
    fn active_with_one_day_is_singular() {
        let summary = license_summary("active", Some(1));
        assert_eq!(summary.detail, Some("1 day left".into()));
    }

    #[test]
    fn active_expiring_today_reads_plainly_and_cautions() {
        // Zero days remaining shouldn't read "0 days left" — say it plainly,
        // and flag it (Caution) since it's urgent but not yet expired.
        let summary = license_summary("active", Some(0));
        assert_eq!(
            summary,
            LicenseSummary {
                label: "Active".into(),
                detail: Some("Expires today".into()),
                tone: LicenseTone::Caution,
            }
        );
    }

    #[test]
    fn expired_reads_as_expired_and_negative() {
        let summary = license_summary("expired", None);
        assert_eq!(
            summary,
            LicenseSummary {
                label: "Expired".into(),
                detail: Some("Renew to restore access".into()),
                tone: LicenseTone::Negative,
            }
        );
    }

    #[test]
    fn an_unrecognised_status_reads_as_unknown_not_fabricated() {
        let summary = license_summary("some_new_engine_status", None);
        assert_eq!(
            summary,
            LicenseSummary {
                label: "Unknown".into(),
                detail: None,
                tone: LicenseTone::Neutral,
            }
        );
    }

    #[test]
    fn active_without_a_day_count_invents_no_number() {
        // Honesty guard: if the engine reports active but no day count, we show
        // no fabricated number rather than "0 days left" or a guess.
        let summary = license_summary("active", None);
        assert_eq!(summary.label, "Active");
        assert_eq!(summary.detail, None);
    }
}
