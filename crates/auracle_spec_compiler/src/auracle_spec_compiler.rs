//! The one-file `# %%spec` strategy compiler.
//!
//! A strategy is a single `.py`. A delimited header block
//!
//! ```text
//! # %%spec
//! # name: OvernightGapReversion
//! # universe: liquid_megacap_200
//! # param.lookback: 20
//! # param.gap_z: 1.5
//! # signal: gap z-score > gap_z
//! # %%end
//! ```
//!
//! is the AI's authoring surface. It compiles into a real
//! `auracle.backtest.Strategy` subclass written immediately below the header.
//! Everything from a `# %%eject` marker onward is **hand-owned** Python that the
//! compiler preserves verbatim across recompiles — the spec never overwrites it.
//!
//! This is deliberately NOT the engine's Forge visual-graph compiler
//! (`auracle.framework.compiler`), which stamps `DO NOT EDIT BY HAND` and
//! round-trips a graph. The contract here is the opposite: the generated region
//! is the compiler's, and the ejected region is the user's, forever.

/// Markers that delimit the three regions of a one-file strategy.
const SPEC_OPEN: &str = "# %%spec";
const SPEC_CLOSE: &str = "# %%end";
/// Boundary between the compiler-owned generated region and the user-owned tail.
pub const EJECT_MARKER: &str = "# %%eject";

/// A parsed strategy spec. Field order is preserved so a render → parse round
/// trip is stable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Spec {
    /// The generated class name.
    pub name: String,
    /// A named universe resolved via `resolve_universe(...)`.
    pub universe: String,
    /// Tunable knobs emitted as class-level attributes (the shape
    /// `walkforward_grid` sweeps). Values are emitted verbatim.
    pub params: Vec<(String, String)>,
    /// Free-form sections (signal/exit/sizing/validate/…) carried as guidance
    /// for the generated body and for the assistant.
    pub sections: Vec<(String, String)>,
}

/// Parse the `# %%spec … # %%end` header out of a one-file strategy. Returns
/// `None` if no well-formed header is present.
pub fn parse_spec(source: &str) -> Option<Spec> {
    let mut lines = source.lines();
    // Advance to the opening marker.
    lines.by_ref().find(|line| line.trim() == SPEC_OPEN)?;

    let mut spec = Spec::default();
    let mut closed = false;
    for line in lines.by_ref() {
        let trimmed = line.trim();
        if trimmed == SPEC_CLOSE {
            closed = true;
            break;
        }
        let Some((key, value)) = parse_header_line(trimmed) else {
            continue;
        };
        match key.as_str() {
            "name" => spec.name = value,
            "universe" => spec.universe = value,
            other => {
                if let Some(param) = other.strip_prefix("param.") {
                    spec.params.push((param.to_string(), value));
                } else {
                    spec.sections.push((other.to_string(), value));
                }
            }
        }
    }
    if closed { Some(spec) } else { None }
}

/// Parse a `# key: value` header line into `(key, value)`.
fn parse_header_line(trimmed: &str) -> Option<(String, String)> {
    let body = trimmed.strip_prefix('#')?.trim();
    let (key, value) = body.split_once(':')?;
    Some((key.trim().to_string(), value.trim().to_string()))
}

/// Render the spec header + the generated `Strategy` subclass. Does NOT include
/// any ejected tail (see [`recompile`] to preserve one).
pub fn render(spec: &Spec) -> String {
    let mut out = String::new();
    out.push_str(SPEC_OPEN);
    out.push('\n');
    out.push_str(&format!("# name: {}\n", spec.name));
    out.push_str(&format!("# universe: {}\n", spec.universe));
    for (key, value) in &spec.params {
        out.push_str(&format!("# param.{key}: {value}\n"));
    }
    for (key, value) in &spec.sections {
        out.push_str(&format!("# {key}: {value}\n"));
    }
    out.push_str(SPEC_CLOSE);
    out.push('\n');

    out.push_str("from auracle.backtest import Strategy\n");
    out.push_str("from auracle.master.universe import resolve_universe\n\n\n");
    out.push_str(&format!("class {}(Strategy):\n", class_name(spec)));
    for (key, value) in &spec.params {
        out.push_str(&format!("    {key} = {value}\n"));
    }
    out.push_str(&format!(
        "    universe = resolve_universe(\"{}\")\n\n",
        spec.universe
    ));
    out.push_str("    def prices_to_signals(self, prices):\n");
    if let Some((_, signal)) = spec.sections.iter().find(|(k, _)| k == "signal") {
        out.push_str(&format!("        # signal: {signal}\n"));
    }
    out.push_str(
        "        raise NotImplementedError(\"Fill prices_to_signals, or ask the assistant.\")\n",
    );
    out
}

/// Compile `spec` into a full file, **preserving** any user-owned tail from
/// `existing` (everything from the first `# %%eject` marker onward). The
/// generated region is replaced; the ejected region is never touched.
pub fn recompile(existing: &str, spec: &Spec) -> String {
    let mut out = render(spec);
    if let Some(tail) = ejected_tail(existing) {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(EJECT_MARKER);
        out.push('\n');
        out.push_str(tail);
    }
    out
}

/// The hand-owned region of a one-file strategy: everything after the first
/// `# %%eject` marker line, verbatim. `None` if the file has no eject marker.
pub fn ejected_tail(source: &str) -> Option<&str> {
    let marker_line_start = source
        .match_indices(EJECT_MARKER)
        .map(|(idx, _)| idx)
        .find(|&idx| idx == 0 || source[..idx].ends_with('\n'))?;
    let after_marker = &source[marker_line_start + EJECT_MARKER.len()..];
    // Skip the rest of the marker line (and its newline) so the tail starts at
    // the user's first owned line.
    let tail_start = after_marker.find('\n').map(|i| i + 1).unwrap_or(after_marker.len());
    Some(&after_marker[tail_start..])
}

/// A safe Python class name: the spec name if it's a valid identifier, else a
/// stable fallback so generated code always parses.
fn class_name(spec: &Spec) -> String {
    let candidate = spec.name.trim();
    let valid = !candidate.is_empty()
        && candidate
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        candidate.to_string()
    } else {
        "GeneratedStrategy".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Spec {
        Spec {
            name: "OvernightGapReversion".to_string(),
            universe: "liquid_megacap_200".to_string(),
            params: vec![
                ("lookback".to_string(), "20".to_string()),
                ("gap_z".to_string(), "1.5".to_string()),
            ],
            sections: vec![("signal".to_string(), "gap z-score > gap_z".to_string())],
        }
    }

    #[test]
    fn parse_extracts_name_universe_and_params() {
        let spec = parse_spec(&render(&sample())).expect("header present");
        assert_eq!(spec.name, "OvernightGapReversion");
        assert_eq!(spec.universe, "liquid_megacap_200");
        assert_eq!(spec.params, sample().params);
        assert_eq!(spec.sections, sample().sections);
    }

    #[test]
    fn render_emits_params_as_class_attributes() {
        let code = render(&sample());
        assert!(code.contains("class OvernightGapReversion(Strategy):"));
        assert!(code.contains("    lookback = 20"));
        assert!(code.contains("    gap_z = 1.5"));
        assert!(code.contains("resolve_universe(\"liquid_megacap_200\")"));
    }

    #[test]
    fn round_trip_is_stable() {
        let spec = sample();
        let reparsed = parse_spec(&render(&spec)).expect("header present");
        assert_eq!(reparsed, spec);
    }

    #[test]
    fn recompile_preserves_ejected_code_verbatim() {
        let owned = "    def signals_to_target_weights(self, signals):\n        return my_custom_sizing(signals)  # hand-owned\n";
        let existing = format!("{}{}\n{}", render(&sample()), EJECT_MARKER, owned);

        // Recompile with a CHANGED spec (a new param value).
        let mut changed = sample();
        changed.params[1].1 = "2.0".to_string();
        let result = recompile(&existing, &changed);

        // The generated region reflects the new spec…
        assert!(result.contains("    gap_z = 2.0"));
        // …and the ejected tail is preserved byte-for-byte.
        assert!(result.contains(owned));
        assert_eq!(ejected_tail(&result), Some(owned));
    }

    #[test]
    fn invalid_name_falls_back_so_output_always_parses() {
        let mut spec = sample();
        spec.name = "3 bad name!".to_string();
        assert!(render(&spec).contains("class GeneratedStrategy(Strategy):"));
    }

    #[test]
    fn no_header_returns_none() {
        assert_eq!(parse_spec("class Foo(Strategy):\n    pass\n"), None);
    }
}
