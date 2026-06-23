//! Decision layer for the editorial timeline pills, shared by the Studio agent
//! timeline and the Runway rail. gpui-free so it is unit-tested without
//! rendering; the GPUI layer maps these to painted pills.
//!
//! Two distinct vocabularies, on purpose:
//! - the Studio AGENT timeline uses the fixed pastel pills (the signature),
//!   scoped to agent action types only;
//! - the RUNWAY pipeline reuses the pill *shape* but is coloured by progress
//!   STATE from the theme (accent/positive/muted), never the pastels — phases
//!   are progress, not action types, so a rainbow there would be decoration.

/// A plain 8-bit RGB triple. The agent pastels are fixed brand values (not theme
/// tokens), so they live here as data the GPUI layer converts to its colour type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// An action the Studio agent is performing, in timeline order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAction {
    Thinking,
    Reading,
    Editing,
    Running,
    Done,
}

/// A painted agent pill: an uppercase caption, a pastel fill, and the ink colour
/// that stays legible on that fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentPill {
    pub label: &'static str,
    pub fill: Rgb,
    pub ink: Rgb,
}

// The five pastels, dark-adapted. Fixed brand values; never theme tokens.
const PEACH: Rgb = Rgb::new(0xDF, 0xA8, 0x8F);
const BLUE: Rgb = Rgb::new(0x9F, 0xBB, 0xE0);
const LAVENDER: Rgb = Rgb::new(0xC0, 0xA8, 0xDD);
const MINT: Rgb = Rgb::new(0x9F, 0xC9, 0xA2);
const GOLD: Rgb = Rgb::new(0xC0, 0x85, 0x32);

// Dark ink reads on the four light pastels; the gold pill is dark enough that it
// needs light ink instead, so it gets the off-white.
const DARK_INK: Rgb = Rgb::new(0x14, 0x16, 0x1B);
const LIGHT_INK: Rgb = Rgb::new(0xEC, 0xEC, 0xE6);

/// Map an agent action to its pill. Scoped to the Studio agent timeline only.
pub fn agent_pill(action: AgentAction) -> AgentPill {
    match action {
        AgentAction::Thinking => AgentPill {
            label: "THINKING",
            fill: PEACH,
            ink: DARK_INK,
        },
        AgentAction::Reading => AgentPill {
            label: "READING",
            fill: BLUE,
            ink: DARK_INK,
        },
        AgentAction::Editing => AgentPill {
            label: "EDITING",
            fill: LAVENDER,
            ink: DARK_INK,
        },
        AgentAction::Running => AgentPill {
            label: "RUNNING",
            fill: MINT,
            ink: DARK_INK,
        },
        AgentAction::Done => AgentPill {
            label: "DONE",
            fill: GOLD,
            ink: LIGHT_INK,
        },
    }
}

/// Progress state of a Runway stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunwayState {
    Current,
    Done,
    Upcoming,
}

/// Theme tone the GPUI layer colours a Runway pill with. Deliberately NOT a
/// pastel — Runway is progress, so it borrows the theme's brand/semantic tones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunwayTone {
    /// The active stage — the one scarce accent.
    Accent,
    /// A reached stage.
    Positive,
    /// A stage not yet reached.
    Muted,
}

/// Map a Runway stage's progress state to its pill tone.
pub fn runway_tone(state: RunwayState) -> RunwayTone {
    match state {
        RunwayState::Current => RunwayTone::Accent,
        RunwayState::Done => RunwayTone::Positive,
        RunwayState::Upcoming => RunwayTone::Muted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_actions_map_to_their_pastels() {
        assert_eq!(agent_pill(AgentAction::Thinking).fill, PEACH);
        assert_eq!(agent_pill(AgentAction::Reading).fill, BLUE);
        assert_eq!(agent_pill(AgentAction::Editing).fill, LAVENDER);
        assert_eq!(agent_pill(AgentAction::Running).fill, MINT);
        assert_eq!(agent_pill(AgentAction::Done).fill, GOLD);
    }

    #[test]
    fn agent_labels_are_uppercase_captions() {
        for action in [
            AgentAction::Thinking,
            AgentAction::Reading,
            AgentAction::Editing,
            AgentAction::Running,
            AgentAction::Done,
        ] {
            let pill = agent_pill(action);
            assert!(!pill.label.is_empty());
            assert_eq!(pill.label, pill.label.to_uppercase());
        }
    }

    #[test]
    fn only_the_gold_pill_uses_light_ink() {
        // The four light pastels carry dark ink; gold is dark enough to flip.
        assert_eq!(agent_pill(AgentAction::Thinking).ink, DARK_INK);
        assert_eq!(agent_pill(AgentAction::Reading).ink, DARK_INK);
        assert_eq!(agent_pill(AgentAction::Editing).ink, DARK_INK);
        assert_eq!(agent_pill(AgentAction::Running).ink, DARK_INK);
        assert_eq!(agent_pill(AgentAction::Done).ink, LIGHT_INK);
    }

    #[test]
    fn the_five_pastels_are_distinct() {
        let fills = [PEACH, BLUE, LAVENDER, MINT, GOLD];
        for (i, a) in fills.iter().enumerate() {
            for b in &fills[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn runway_state_maps_to_progress_tone() {
        assert_eq!(runway_tone(RunwayState::Current), RunwayTone::Accent);
        assert_eq!(runway_tone(RunwayState::Done), RunwayTone::Positive);
        assert_eq!(runway_tone(RunwayState::Upcoming), RunwayTone::Muted);
    }

    #[test]
    fn runway_never_borrows_an_agent_pastel() {
        // Different vocabularies: the Runway tone enum has no pastel at all, so a
        // stage can never accidentally render as a Cursor agent colour. This test
        // documents that boundary — RunwayTone and AgentPill share no colour API.
        let tones = [RunwayTone::Accent, RunwayTone::Positive, RunwayTone::Muted];
        assert_eq!(tones.len(), 3);
    }
}
