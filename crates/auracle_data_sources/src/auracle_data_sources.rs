//! Honest models for the native "Data sources" section on the Connections
//! settings page.
//!
//! The engine reports which market-data vendor keys it holds as a map of
//! vendor-key → configured. The IDE shows that read-only: it never edits the
//! keys (they live in the launcher/engine) and never claims a vendor is
//! configured unless the engine said so. The only real transform is humanizing
//! the engine's snake_case vendor key into a label (`nasdaq_data_link` →
//! "Nasdaq Data Link") — done mechanically, inventing no fancy capitalization.
//! Kept gpui-free so it is unit-tested without rendering.

/// One read-only data-source row: a humanized vendor name and whether the engine
/// holds a usable key for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSourceRow {
    pub name: String,
    pub configured: bool,
}

/// Humanize an engine vendor key for display: split on `_`, drop empty segments
/// (so leading/trailing/double underscores don't produce blank words), and
/// capitalize the first letter of each remaining word. Mechanical only — it
/// uppercases the first character and leaves the rest as the engine sent it, so
/// it never invents an acronym casing the engine didn't provide.
pub fn humanize_vendor(key: &str) -> String {
    key.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the read-only data-source rows from the engine's `(vendor_key,
/// configured)` entries, humanizing each name and preserving the given order
/// (the engine map is already sorted). `configured` is passed through verbatim —
/// never inferred.
pub fn data_source_rows<'a>(
    entries: impl IntoIterator<Item = (&'a str, bool)>,
) -> Vec<DataSourceRow> {
    entries
        .into_iter()
        .map(|(vendor_key, configured)| DataSourceRow {
            name: humanize_vendor(vendor_key),
            configured,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_multi_word_key() {
        assert_eq!(humanize_vendor("nasdaq_data_link"), "Nasdaq Data Link");
    }

    #[test]
    fn humanize_single_word_key() {
        assert_eq!(humanize_vendor("polygon"), "Polygon");
    }

    #[test]
    fn humanize_leaves_inner_letters_untouched() {
        // We don't invent acronym casing: only the first letter is uppercased.
        assert_eq!(humanize_vendor("eodhd"), "Eodhd");
    }

    #[test]
    fn humanize_drops_empty_segments() {
        assert_eq!(humanize_vendor("_brain__feed_"), "Brain Feed");
    }

    #[test]
    fn humanize_empty_is_empty() {
        assert_eq!(humanize_vendor(""), "");
    }

    #[test]
    fn rows_humanize_and_preserve_order_and_configured() {
        let rows = data_source_rows([
            ("coingecko", true),
            ("nasdaq_data_link", false),
            ("polygon", true),
        ]);
        assert_eq!(
            rows,
            vec![
                DataSourceRow {
                    name: "Coingecko".to_string(),
                    configured: true
                },
                DataSourceRow {
                    name: "Nasdaq Data Link".to_string(),
                    configured: false
                },
                DataSourceRow {
                    name: "Polygon".to_string(),
                    configured: true
                },
            ]
        );
    }

    #[test]
    fn no_entries_yield_no_rows() {
        let rows = data_source_rows(std::iter::empty::<(&str, bool)>());
        assert!(rows.is_empty());
    }
}
