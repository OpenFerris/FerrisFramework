//! Schedule persistence
//!
//! The `schedules` table is the durable record of intent (which cron, enabled
//! or not, and when it last ran). Split into the row conversion
//! ([`ScheduleRecord`]) and the store ([`ScheduleStore`]) so the validation
//! rules and the statements can be read separately.

mod record;
mod store;

pub use record::ScheduleRecord;
pub use store::ScheduleStore;

#[cfg(test)]
mod tests;
