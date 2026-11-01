//! Real daemon framework HTTP, WSGI and ASGI adapter smoke tests.

#![cfg(feature = "test-support")]

#[path = "../python_support/mod.rs"]
mod python_support;

mod frameworks;
