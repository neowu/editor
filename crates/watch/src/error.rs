//! Watch error types.

use std::error;
use std::fmt;

#[derive(Debug, Eq, PartialEq)]
pub struct NoReceiverError;

impl fmt::Display for NoReceiverError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "all receivers were dropped")
    }
}

impl error::Error for NoReceiverError {}

#[derive(Debug, Eq, PartialEq)]
pub struct NoSenderError;

impl fmt::Display for NoSenderError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "sender was dropped")
    }
}

impl error::Error for NoSenderError {}
