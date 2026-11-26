//! What the interpreter refuses to run, and why. The Python raises `Untranslated`, `RuntimeError`
//! or a value error; the CLI prints the kind, so the parity test can compare them.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// An instruction or operand the interpreter does not execute yet; the message names it.
    Untranslated(String),
    /// An operand or register the model refuses (a wave size, a write out of range).
    Operand(String),
    /// The program ran `MAX_STEPS` instructions without ending.
    Loop(u64),
}

impl Error {
    /// The kind as the CLI prints it on its `error` line.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Untranslated(_) => "untranslated",
            Error::Operand(_) => "operand",
            Error::Loop(_) => "loop",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Untranslated(message) | Error::Operand(message) => formatter.write_str(message),
            Error::Loop(steps) => write!(
                formatter,
                "programme interrompu après {steps} instructions (boucle infinie ?)"
            ),
        }
    }
}

impl std::error::Error for Error {}
