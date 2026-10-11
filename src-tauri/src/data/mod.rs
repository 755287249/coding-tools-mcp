mod connection;
pub use connection::GLOBAL_MCP_ID;
mod migrate;
mod model;
mod protection;
mod store;

pub use model::AppData;
pub use protection::run_protect_cli as protection_cli;
pub use store::DataStore;

pub(crate) use connection::control_folder;
