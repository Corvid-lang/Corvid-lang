//! Display formatting for type diagnostics.

use std::fmt;

use super::{TypeError, TypeWarning};

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}..{}] {}",
            self.span.start,
            self.span.end,
            self.message()
        )?;
        if let Some(hint) = self.hint() {
            write!(f, "\n  help: {hint}")?;
        }
        Ok(())
    }
}

impl fmt::Display for TypeWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}..{}] {}",
            self.span.start,
            self.span.end,
            self.message()
        )?;
        if let Some(hint) = self.hint() {
            write!(f, "\n  help: {hint}")?;
        }
        Ok(())
    }
}
