use std::{
  fmt::{self, Debug},
  io::{self, Cursor},
  sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
  },
  thread,
};

use async_trait::async_trait;
use byteorder::{LittleEndian, ReadBytesExt};
use evdev::{FFReplay, FFTrigger};
use futures_util::{future::BoxFuture, FutureExt};
use tokio::sync::{broadcast, mpsc};

use crate::{
  core::{errors::ButtplugDeviceError, message::Endpoint},
  server::device::{
    configuration::{EvdevSpecifier, ProtocolCommunicationSpecifier},
    hardware::{
      GenericHardwareSpecializer, Hardware, HardwareConnector, HardwareEvent, HardwareInternal,
      HardwareReadCmd, HardwareReading, HardwareSpecializer, HardwareSubscribeCmd,
      HardwareUnsubscribeCmd, HardwareWriteCmd,
    },
  },
};

pub struct EvdevHardwareConnector {
  device: Arc<Mutex<evdev::Device>>,
}

impl EvdevHardwareConnector {
  pub fn new(device: evdev::Device) -> Self {
    Self {
      device: Arc::new(Mutex::new(device)),
    }
  }
}

impl Debug for EvdevHardwareConnector {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let device = self.device.lock().expect("UwU");
    f.debug_struct("EvdevHardwareConnector")
      .field("vid", &device.input_id().vendor())
      .field("pid", &device.input_id().product())
      .field("ver", &device.input_id().version())
      .finish()
  }
}

#[async_trait]
impl HardwareConnector for EvdevHardwareConnector {
  fn specifier(&self) -> ProtocolCommunicationSpecifier {
    let device = self.device.lock().expect("UwU");
    info!(
      "Specifier for {}: {:#04x} {:#04x} v{:#04x}",
      &device.name().unwrap_or("Unnamed device"),
      &device.input_id().vendor(),
      &device.input_id().product(),
      &device.input_id().version(),
    );
    ProtocolCommunicationSpecifier::Evdev(EvdevSpecifier::default())
  }

  async fn connect(&mut self) -> Result<Box<dyn HardwareSpecializer>, ButtplugDeviceError> {
    let device = self.device.lock().expect("UwU");
    info!(
      "New Evdev device created: {}",
      &device.name().unwrap_or("Unnamed Device")
    );
    let hardware = Hardware::new(
      &device.name().unwrap_or("Unnamed Device"),
      &device.input_id().product().to_string().as_str(),
      &[Endpoint::Rx, Endpoint::Tx],
      Box::new(EvdevDeviceImpl::new(self.device.clone())),
    );
    Ok(Box::new(GenericHardwareSpecializer::new(hardware)))
  }
}

pub struct EvdevDeviceImpl {
  connected: Arc<AtomicBool>,
  device_event_sender: broadcast::Sender<HardwareEvent>, // TODO: Do we need this?
  write_sender: mpsc::Sender<Vec<u8>>,

  // TODO: Do we need to keep these?
  _write_thread: thread::JoinHandle<()>,
  device: Arc<Mutex<evdev::Device>>,
}

impl EvdevDeviceImpl {
  pub fn new(device: Arc<Mutex<evdev::Device>>) -> Self {
    let (device_event_sender, _) = broadcast::channel(256);
    let (write_sender, write_receiver) = mpsc::channel(256);

    let thread_device = device.clone();
    let write_thread = thread::Builder::new()
      .name("Serial Writer Thread".to_string())
      .spawn(move || {
        write_thread(thread_device, write_receiver);
      })
      .expect("Should always be able to create thread");

    Self {
      device,
      write_sender,
      _write_thread: write_thread,
      connected: Arc::new(AtomicBool::new(true)),
      device_event_sender,
    }
  }
}

fn vibrate(
  device: &mut evdev::Device,
  magnitude: &Vec<u8>,
  length_ms: u16,
) -> io::Result<evdev::FFEffect> {
  let mut cursor = Cursor::new(magnitude);
  //TODO: Maybe we can use both motors?
  let magnitude = cursor
    .read_u16::<LittleEndian>()
    .expect("Packed in protocol, infallible");
  println!("[Evdev] Vibrating at: {magnitude} for {length_ms}ms");
  let effect = device.upload_ff_effect(evdev::FFEffectData {
    // direction: 0x4000,
    direction: 0,
    trigger: FFTrigger {
      button: 0,
      interval: 0,
    },
    replay: FFReplay {
      delay: 0,
      length: length_ms,
    },
    // kind: evdev::FFEffectKind::Periodic {
    //   waveform: evdev::FFWaveform::Sine,
    //   period: 100,
    //   magnitude: magnitude as i16,
    //   offset: 0,
    //   phase: 0,
    //   envelope: evdev::FFEnvelope {
    //     attack_length: 0,
    //     attack_level: u16::MAX,
    //     fade_length: 0,
    //     fade_level: u16::MAX,
    //   },
    // },
    kind: evdev::FFEffectKind::Rumble {
      weak_magnitude: magnitude,
      // strong_magnitude: magnitude as u16,
      strong_magnitude: magnitude,
    },
  })?;

  // effect.play(i32::MAX)?; //TODO: Change this?
  // thread::sleep(Duration::from_millis(length_ms as u64 + 10000));
  Ok(effect)
}

fn write_thread(device: Arc<Mutex<evdev::Device>>, receiver: mpsc::Receiver<Vec<u8>>) {
  let mut recv = receiver;
  // Instead of waiting on a token here, we'll expect that we'll break on our
  // channel going away.
  //
  // This is a blocking recv so we don't have to worry about the port.
  let mut device = device.lock().expect("Couldnt lock device :<");
  // Dont drop effect else it stops
  let mut effect_nodrop = None;
  while let Some(v) = recv.blocking_recv() {
    match vibrate(&mut device, &v, 100) {
      Ok(mut effect) => {
        drop(effect_nodrop.take());
        effect.play(i32::MAX).expect("Ohno :<");
        effect_nodrop = Some(effect);
      }
      Err(err) => {
        error!("Cannot vibrate, exiting thread: {}", err);
        return;
      }
    }
  }
  drop(effect_nodrop);
}

impl HardwareInternal for EvdevDeviceImpl {
  fn event_stream(&self) -> broadcast::Receiver<HardwareEvent> {
    self.device_event_sender.subscribe()
  }

  fn disconnect(&self) -> BoxFuture<'static, Result<(), ButtplugDeviceError>> {
    let connected = self.connected.clone();
    Box::pin(async move {
      connected.store(false, Ordering::SeqCst);
      Ok(())
    })
  }

  fn read_value(
    &self,
    _msg: &HardwareReadCmd,
  ) -> BoxFuture<'static, Result<HardwareReading, ButtplugDeviceError>> {
    unimplemented!();
  }

  fn write_value(
    &self,
    msg: &HardwareWriteCmd,
  ) -> BoxFuture<'static, Result<(), ButtplugDeviceError>> {
    let sender = self.write_sender.clone();
    let data = msg.data.clone();
    // TODO Should check endpoint validity
    async move {
      sender
        .send(data)
        .await
        .expect("Tasks should exist if we get here.");
      Ok(())
    }
    .boxed()
  }

  fn subscribe(
    &self,
    _msg: &HardwareSubscribeCmd,
  ) -> BoxFuture<'static, Result<(), ButtplugDeviceError>> {
    unimplemented!();
  }

  fn unsubscribe(
    &self,
    _msg: &HardwareUnsubscribeCmd,
  ) -> BoxFuture<'static, Result<(), ButtplugDeviceError>> {
    unimplemented!();
  }
}
