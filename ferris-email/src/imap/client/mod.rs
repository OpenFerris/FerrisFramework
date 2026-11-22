//! Connection, TLS and the commands the inbound poller needs.
//!
//! Split in two: [`transport`] owns the socket, the TLS upgrade and the
//! literal-aware response reader; [`session`] owns the IMAP commands and the
//! client API. The reader is the only piece that has to understand the wire
//! format, so a change to the command set never touches TLS, and vice versa.

mod session;
mod transport;

pub use session::{FetchedMessage, ImapClient, ImapSession};
