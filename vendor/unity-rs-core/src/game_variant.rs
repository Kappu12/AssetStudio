//! Explicit caller-selected layouts for the AssetStudio fork's game variants.
//!
//! Variants are never detected heuristically from object bytes. A variant only
//! changes the documented fields for that fork; it does not relax limits,
//! enable unknown compression, or claim every version of a game is verified.

/// Serialized-object layout selected by the caller after container decoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GameVariant {
    /// Ordinary Unity and wrappers that retain ordinary serialized layouts.
    #[default]
    Standard,
    /// Naraka's Material field between string tags and disabled passes.
    Naraka,
    /// Tears of Themis uses player objects despite its `NoTarget` metadata.
    TearsOfThemis,
    /// Historical Honkai Impact 3 build preceding the other BH3 variants.
    HonkaiImpact3PrePre,
    /// Ex Astris texture settings and Mesh collider metadata.
    ExAstris,
    /// Love and Deepspace's material, mesh, shader and controller additions.
    /// This selects layout fields only; opaque animation codecs stay separate.
    LoveAndDeepspace,
}

impl GameVariant {
    /// Matches the baseline `IsMhyGroup` player-layout override only for the
    /// explicitly implemented members of that group. Other platforms survive.
    pub(crate) const fn effective_target_platform(self, declared: i32) -> i32 {
        if declared == -2 && matches!(self, Self::TearsOfThemis | Self::HonkaiImpact3PrePre) {
            19 // BuildTarget.StandaloneWindows64
        } else {
            declared
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GameVariant;

    #[test]
    fn player_override_is_explicit_and_does_not_change_other_platforms() {
        for variant in [GameVariant::TearsOfThemis, GameVariant::HonkaiImpact3PrePre] {
            assert_eq!(variant.effective_target_platform(-2), 19);
            assert_eq!(variant.effective_target_platform(13), 13);
            assert_eq!(variant.effective_target_platform(19), 19);
        }
        for variant in [
            GameVariant::Standard,
            GameVariant::Naraka,
            GameVariant::ExAstris,
            GameVariant::LoveAndDeepspace,
        ] {
            assert_eq!(variant.effective_target_platform(-2), -2);
        }
    }
}
