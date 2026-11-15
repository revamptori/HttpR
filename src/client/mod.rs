//! Synchronous and asynchronous HTTP clients and configuration builders.

pub mod async_client;
pub mod blocking;
pub mod builder;
pub mod config;

pub use async_client::AsyncClient;
pub use blocking::{shared_runtime, Client};
pub use builder::ClientBuilder;
pub use config::{ClientConfig, SendOptions};
