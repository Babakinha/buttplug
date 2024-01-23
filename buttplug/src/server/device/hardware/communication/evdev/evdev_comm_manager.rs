use async_trait::async_trait;
use std::fs;
use tokio::sync::mpsc::Sender;

use crate::{
  core::errors::ButtplugDeviceError,
  server::device::hardware::communication::{
    HardwareCommunicationManager, HardwareCommunicationManagerBuilder,
    HardwareCommunicationManagerEvent, TimedRetryCommunicationManager,
    TimedRetryCommunicationManagerImpl, evdev::evdev_hardware::EvdevHardwareConnector,
  },
};

#[derive(Default, Clone)]
pub struct EvdevCommunicationManagerBuilder {}

impl HardwareCommunicationManagerBuilder for EvdevCommunicationManagerBuilder {
  fn finish(
    &mut self,
    sender: Sender<HardwareCommunicationManagerEvent>,
  ) -> Box<dyn HardwareCommunicationManager> {
    Box::new(TimedRetryCommunicationManager::new(
      EvdevCommunicationManager::new(sender),
    ))
  }
}

pub struct EvdevCommunicationManager {
  sender: Sender<HardwareCommunicationManagerEvent>,
}

impl EvdevCommunicationManager {
  fn new(sender: Sender<HardwareCommunicationManagerEvent>) -> Self {
    Self { sender }
  }
}

#[async_trait]
impl TimedRetryCommunicationManagerImpl for EvdevCommunicationManager {
  fn name(&self) -> &'static str {
    "EvdevCommunicationManager"
  }

  async fn scan(&self) -> Result<(), ButtplugDeviceError> {
    // TODO: Is this blocking? should we try to run this in another thread?
    let device_sender = self.sender.clone();
    let events_dir = fs::read_dir("/dev/input/").expect("owo?");

    for file in events_dir {
      // Check if device is a vaild event thingy
      if file.is_err() {
        continue;
      }
      let event = file.unwrap();
      if !event.file_name().to_str().expect(":<").starts_with("event") {
        continue;
      }

      let device = evdev::Device::open(event.path());
      if let Ok(device) = device {
        // TODO: Check more?
        if device.supported_ff().is_none() {
          continue;
        }

        if device_sender
          .send(HardwareCommunicationManagerEvent::DeviceFound {
            name: device.name().unwrap_or("Unnamed device").to_string(),
            address: device.input_id().product().to_string(),
            creator: Box::new(EvdevHardwareConnector::new(device)),
          })
          .await
          .is_err()
        {
          error!("Oh no.");
          return Ok(());
        }
      }
    }

    Ok(())
  }

  fn can_scan(&self) -> bool {
    true
  }
}
