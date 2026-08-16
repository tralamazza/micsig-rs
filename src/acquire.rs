//! Acquisition run control.
//!
//! The `:MENU:RUN`, `:MENU:STOP` and `:MENU:SINGLE` commands are filed under
//! the menu subsystem rather than anywhere obvious, and the state they change
//! is read back from `:TRIGger:STATus?`. `waveform --mode raw` wants a stopped
//! instrument, so this exists mainly so that prerequisite does not send you to
//! raw SCPI.

use crate::error::Result;
use crate::transport::Scpi;

/// What to do to the acquisition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Acquire continuously.
    Run,
    /// Halt, freezing the current record.
    Stop,
    /// Arm for one acquisition, then stop.
    Single,
}

impl Action {
    fn command(self) -> &'static str {
        match self {
            Action::Run => ":MENU:RUN",
            Action::Stop => ":MENU:STOP",
            Action::Single => ":MENU:SINGLE",
        }
    }
}

/// Apply an action and report the state the instrument settles into.
///
/// The reply is passed through as the instrument words it (`RUN`, `STOP`,
/// ...) rather than being mapped onto [`Action`], since the two are not the
/// same vocabulary — `single` is a command, not a status.
pub fn set(inst: &mut impl Scpi, action: Action) -> Result<String> {
    inst.send(action.command())?;
    status(inst)
}

/// Read the current acquisition state.
pub fn status(inst: &mut impl Scpi) -> Result<String> {
    Ok(inst.query(":TRIGger:STATus?")?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_actions_to_the_menu_subsystem() {
        assert_eq!(Action::Run.command(), ":MENU:RUN");
        assert_eq!(Action::Stop.command(), ":MENU:STOP");
        assert_eq!(Action::Single.command(), ":MENU:SINGLE");
    }
}
