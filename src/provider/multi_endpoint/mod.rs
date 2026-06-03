//! Multi-endpoint support for Copilot provider.
//! Each submodule implements payload building, response parsing, and SSE streaming
//! for a specific API endpoint (chat/completions, /responses, /v1/messages).

pub mod anthropic_messages;
pub mod dispatcher;
pub mod responses_api;

pub use anthropic_messages::{
    build_anthropic_payload, parse_anthropic_response, stream_anthropic_messages,
};
pub use dispatcher::{
    build_request_payload, build_request_payload_value, is_retriable_endpoint_error,
    parse_response_by_endpoint,
};
pub use responses_api::{build_responses_payload, parse_responses_response, stream_responses_api};
