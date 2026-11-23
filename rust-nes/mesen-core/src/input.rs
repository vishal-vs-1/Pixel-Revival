//! Controllers and input devices.
//!
//! OWNER: input agent. Start with the standard controller; add Zapper, Four
//! Score, and other Mesen devices as time permits.

use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::buttons;

/// Standard NES controller (4021 shift register).
#[derive(Clone, Debug, Default)]
pub struct Controller {
    buttons: u8,
    shift: u8,
    strobe: bool,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            buttons: 0,
            shift: 0,
            strobe: false,
        }
    }

    pub fn set_buttons(&mut self, buttons: u8) {
        self.buttons = buttons;
    }

    pub fn buttons(&self) -> u8 {
        self.buttons
    }

    pub fn write_strobe(&mut self, value: u8) {
        self.strobe = value & 1 != 0;
        if self.strobe {
            self.shift = self.buttons;
        }
    }

    pub fn read(&mut self) -> u8 {
        if self.strobe {
            self.buttons & 1
        } else {
            let value = self.shift & 1;
            self.shift = (self.shift >> 1) | 0x80;
            value
        }
    }

    pub fn save_state(&self, w: &mut StateWriter) {
        w.u8(self.buttons);
        w.u8(self.shift);
        w.bool(self.strobe);
    }

    pub fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.buttons = r.u8()?;
        self.shift = r.u8()?;
        self.strobe = r.bool()?;
        Ok(())
    }
}

/// Convenience: make a button mask from individual flags.
pub fn mask(button_list: &[u8]) -> u8 {
    button_list.iter().fold(buttons::NONE, |acc, b| acc | b)
}
