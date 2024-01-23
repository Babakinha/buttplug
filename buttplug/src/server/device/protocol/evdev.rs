// Buttplug Rust Source Code File - See https://buttplug.io for more info.
//
// Copyright 2016-2022 Nonpolynomial Labs LLC. All rights reserved.
//
// Licensed under the BSD 3-Clause license. See LICENSE file in the project root
// for full license information.

use byteorder::LittleEndian;

use crate::{
  core::{
    errors::ButtplugDeviceError,
    message::{ActuatorType, Endpoint},
  },
  server::device::{
    hardware::{HardwareCommand, HardwareWriteCmd},
    protocol::{generic_protocol_setup, ProtocolHandler},
  },
};
use byteorder::WriteBytesExt;

generic_protocol_setup!(Evdev, "evdev");

#[derive(Default)]
pub struct Evdev {}

impl ProtocolHandler for Evdev {
  fn needs_full_command_set(&self) -> bool {
    true
  }

  fn handle_scalar_cmd(
    &self,
    cmds: &[Option<(ActuatorType, u32)>],
  ) -> Result<Vec<HardwareCommand>, ButtplugDeviceError> {
    let mut cmd = vec![];
    if cmd
      .write_i16::<LittleEndian>(
        cmds[0]
          .expect(":3")
          .1 as i16,
      )
      .is_err()
    {
      return Err(ButtplugDeviceError::ProtocolSpecificError(
        "Evdev".to_owned(),
        "Cannot convert Evdev value for processing".to_owned(),
      ));
    }
    Ok(vec![HardwareWriteCmd::new(Endpoint::Tx, cmd, false).into()])
  }
}
