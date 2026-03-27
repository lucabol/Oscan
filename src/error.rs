#![allow(dead_code)]

use crate::token::Span;
use std::fmt;

/// Compiler error with source location and message.
#[derive(Debug, Clone)]
pub struct CompileError {
    pub span: Span,
    pub message: String,
    pub file: Option<String>,
}

impl CompileError {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
            file: None,
        }
    }

    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "error in {}:{}: {}", file, self.span, self.message)
        } else {
            write!(f, "error at {}: {}", self.span, self.message)
        }
    }
}

impl std::error::Error for CompileError {}

pub type CompileResult<T> = Result<T, CompileError>;
