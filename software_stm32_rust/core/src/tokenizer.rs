//! Command line tokenizer for the text protocols. Hardware-free.

use crate::arg_parser::{parse_i32, parse_u32};

fn is_separator(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

fn skip_separators(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|&&c| is_separator(c)).count();
    s.get(n..).unwrap_or_default()
}

/// The token at the start of `s` and the rest behind it.
fn split_token(s: &[u8]) -> (&[u8], &[u8]) {
    let n = s.iter().position(|&c| is_separator(c)).unwrap_or(s.len());
    s.split_at(n)
}

const ARG_SLOTS: usize = 8;

/// Splits a command line into a command and its arguments.
/// Separators are runs of spaces and tabs, so a trailing separator is optional
/// and empty tokens never occur.
///
/// The C++ tokenizer writes a NUL behind every token inside the line; the Rust tokens are
/// sub-slices of the line, which stays unchanged.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tokenizer<'a> {
    command: &'a [u8],
    args: [&'a [u8]; ARG_SLOTS],
    argc: u8,
    too_many: bool,
}

impl<'a> Tokenizer<'a> {
    pub const MAX_ARGS: u8 = ARG_SLOTS as u8;

    /// Tokenizes `line`. Returns false for an empty line or when there are more than
    /// min(max_args, MAX_ARGS) arguments; in the latter case command() and the first
    /// arguments stay available and too_many_args() is true. (C++ default for max_args:
    /// MAX_ARGS.)
    pub fn parse(&mut self, line: &'a [u8], max_args: u8) -> bool {
        self.command = &[];
        self.argc = 0;
        self.too_many = false;
        let max_args = max_args.min(Self::MAX_ARGS);

        let rest = skip_separators(line);
        if rest.is_empty() {
            return false;
        }
        let (command, mut rest) = split_token(rest);
        self.command = command;

        loop {
            rest = skip_separators(rest);
            if rest.is_empty() {
                return true;
            }
            if self.argc == max_args {
                self.too_many = true;
                return false;
            }
            let (token, tail) = split_token(rest);
            if let Some(slot) = self.args.get_mut(usize::from(self.argc)) {
                *slot = token;
            }
            self.argc += 1;
            rest = tail;
        }
    }

    pub fn command(&self) -> &'a [u8] {
        self.command
    }

    /// Exact, case-sensitive match of the command token; false without one.
    pub fn is(&self, name: &[u8]) -> bool {
        !self.command.is_empty() && self.command == name
    }

    pub fn argc(&self) -> u8 {
        self.argc
    }

    pub fn too_many_args(&self) -> bool {
        self.too_many
    }

    /// Argument i, or "" if it does not exist.
    pub fn arg(&self, i: u8) -> &'a [u8] {
        if i < self.argc {
            self.args.get(usize::from(i)).copied().unwrap_or_default()
        } else {
            &[]
        }
    }

    /// Checked conversions of argument i (see arg_parser); `None` if the
    /// argument is missing, malformed or outside [lo, hi].
    pub fn arg_u32(&self, i: u8, lo: u32, hi: u32) -> Option<u32> {
        parse_u32(self.arg(i), lo, hi)
    }

    pub fn arg_i32(&self, i: u8, lo: i32, hi: i32) -> Option<i32> {
        parse_i32(self.arg(i), lo, hi)
    }

    pub fn arg_u16(&self, i: u8, lo: u16, hi: u16) -> Option<u16> {
        // the value is at most hi, so it fits
        self.arg_u32(i, u32::from(lo), u32::from(hi))
            .map(|v| v as u16)
    }

    pub fn arg_u8(&self, i: u8, lo: u8, hi: u8) -> Option<u8> {
        self.arg_u32(i, u32::from(lo), u32::from(hi))
            .map(|v| v as u8)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_fuzz;
