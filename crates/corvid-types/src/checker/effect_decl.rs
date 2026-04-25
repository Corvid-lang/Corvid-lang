use super::*;
use corvid_ast::{DimensionValue, EffectDecl};

impl<'a> Checker<'a> {
    pub(super) fn check_effect_decl_confidence(&mut self, effect: &EffectDecl) {
        for dim in &effect.dimensions {
            match (&dim.name.name[..], &dim.value) {
                ("confidence", DimensionValue::Number(value)) if !(0.0..=1.0).contains(value) => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::InvalidConfidence { value: *value },
                        dim.span,
                    ));
                }
                (
                    "trust",
                    DimensionValue::ConfidenceGated {
                        threshold,
                        ..
                    },
                ) if !(0.0..=1.0).contains(threshold) => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::InvalidConfidence { value: *threshold },
                        dim.span,
                    ));
                }
                _ => {}
            }
        }
    }
}
