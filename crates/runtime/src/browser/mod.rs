//! Bounded CDP transports and verified browser installation inputs.

mod cdp;
mod client;
mod downloads;
mod frontend;
mod installation;
mod manager;
mod process;
mod scope;
mod session;
pub use cdp::{BrowserCdp, BrowserCdpEvents};
pub use frontend::BrowserFrontend;
pub use installation::BrowserInstallation;
pub use manager::{BrowserGeneration, BrowserManager};
pub use process::BrowserProcess;
pub use scope::BrowserScope;
pub use session::ManagedBrowserSession;

#[cfg(test)]
mod process_tests;
