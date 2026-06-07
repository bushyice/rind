#[cfg(not(feature = "no-run0"))]
pub mod run0;
#[cfg(not(feature = "no-sysinvoke"))]
pub mod sysinvoke;
#[cfg(not(feature = "no-syslogs"))]
pub mod syslogs;

pub mod sysperms;
pub mod syssess;
pub mod sysunit;

include!(concat!(env!("OUT_DIR"), "/generated_applets.rs"));
