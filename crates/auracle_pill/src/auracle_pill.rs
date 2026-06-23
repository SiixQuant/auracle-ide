//! Decision layer for the Studio agent timeline's fixed pastel pills (the
//! editorial signature), scoped to agent action types only. gpui-free so it is
//! unit-tested without rendering; the GPUI layer converts these to painted pills.
//!
//! The Runway rail reuses the pill *shape* but is coloured by progress STATE
//! from the theme, never these pastels — phases are progress, not action types.
//! That tone lives in `ui::PillTone` (mapped from `auracle_runway::StageTone`),
//! so it is deliberately absent here.

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
}
