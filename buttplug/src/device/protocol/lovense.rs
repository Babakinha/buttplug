use super::{ButtplugDeviceResultFuture, ButtplugProtocol, ButtplugProtocolCommandHandler};
use crate::{
    core::errors::ButtplugDeviceError,
    device::{ButtplugDeviceEvent, DeviceSubscribeCmd},
};
use crate::{
    core::{
        errors::ButtplugError,
        messages::{self, ButtplugDeviceCommandMessageUnion, MessageAttributesMap},
    },
    device::{
        protocol::{generic_command_manager::GenericCommandManager, ButtplugProtocolProperties},
        DeviceImpl, DeviceWriteCmd, Endpoint,
    },
};
use futures::future::BoxFuture;
use futures::StreamExt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{broadcast, Mutex};

#[derive(ButtplugProtocolProperties)]
pub struct Lovense {
    name: String,
    message_attributes: MessageAttributesMap,
    manager: Arc<Mutex<GenericCommandManager>>,
    stop_commands: Vec<ButtplugDeviceCommandMessageUnion>,
    rotation_direction: Arc<AtomicBool>,
}

impl ButtplugProtocol for Lovense {
    fn new_protocol(name: &str, attrs: MessageAttributesMap) -> Box<dyn ButtplugProtocol> {
        let manager = GenericCommandManager::new(&attrs);
        Box::new(Self {
            name: name.to_owned(),
            message_attributes: attrs,
            stop_commands: manager.get_stop_commands(),
            manager: Arc::new(Mutex::new(manager)),
            rotation_direction: Arc::new(AtomicBool::new(false)),
        })
    }

    fn initialize(
        device_impl: &DeviceImpl,
    ) -> BoxFuture<'static, Result<Option<String>, ButtplugError>> {
        let subscribe_fut = device_impl.subscribe(DeviceSubscribeCmd::new(Endpoint::Rx));
        let msg = DeviceWriteCmd::new(Endpoint::Tx, b"DeviceType;".to_vec(), false);
        let info_fut = device_impl.write_value(msg);
        Box::pin(async move {
            let identifier;
            let mut event_receiver: broadcast::Receiver<Vec<u8>> = subscribe_fut.await?;
            info_fut.await?;
            // TODO Put some sort of very quick timeout here, we should just fail if
            // we don't get something back quickly.
            if let Ok(data) = event_receiver.recv().await {
                let type_response = std::str::from_utf8(&data).unwrap().to_owned();
                info!("Lovense Device Type Response: {}", type_response);
                identifier = type_response.split(':').collect::<Vec<&str>>()[0].to_owned();
                Ok(Some(identifier))
            } else {
              Err(ButtplugDeviceError::DeviceConnectionError("Cannot retreive Lovense device information.".to_owned()).into())
            }
        })
    }
}

impl ButtplugProtocolCommandHandler for Lovense {
    fn handle_vibrate_cmd(
        &self,
        device: Arc<DeviceImpl>,
        msg: messages::VibrateCmd,
    ) -> ButtplugDeviceResultFuture {
        let manager = self.manager.clone();
        Box::pin(async move {
            // Store off result before the match, so we drop the lock ASAP.
            let result = manager.lock().await.update_vibration(&msg, false)?;
            // Lovense is the same situation as the Lovehoney Desire, where commands
            // are different if we're addressing all motors or seperate motors.
            // Difference here being that there's Lovense variants with different
            // numbers of motors.
            //
            // Neat way of checking if everything is the same via
            // https://sts10.github.io/2019/06/06/is-all-equal-function.html.
            //
            // Just make sure we're not matching on None, 'cause if that's the case
            // we ain't got shit to do.
            let mut fut_vec = vec![];
            if let Some(cmds) = result {
                if cmds[0].is_some() && (cmds.len() == 1 || cmds.windows(2).all(|w| w[0] == w[1])) {
                    let lovense_cmd = format!("Vibrate:{};", cmds[0].unwrap()).as_bytes().to_vec();
                    let fut =
                        device.write_value(DeviceWriteCmd::new(Endpoint::Tx, lovense_cmd, false));
                    fut.await?;
                    return Ok(messages::Ok::default().into());
                }
                for (i, cmd) in cmds.iter().enumerate() {
                    if let Some(speed) = cmd {
                        let lovense_cmd =
                            format!("Vibrate{}:{};", i + 1, speed).as_bytes().to_vec();
                        fut_vec.push(device.write_value(DeviceWriteCmd::new(
                            Endpoint::Tx,
                            lovense_cmd,
                            false,
                        )));
                    }
                }
            }
            for fut in fut_vec {
                fut.await?;
            }
            Ok(messages::Ok::default().into())
        })
    }

    fn handle_rotate_cmd(
        &self,
        device: Arc<DeviceImpl>,
        msg: messages::RotateCmd,
    ) -> ButtplugDeviceResultFuture {
        let manager = self.manager.clone();
        let direction = self.rotation_direction.clone();
        Box::pin(async move {
            let result = manager.lock().await.update_rotation(&msg)?;
            if let Some((speed, clockwise)) = result[0] {
                let lovense_cmd = format!("Rotate:{};", speed).as_bytes().to_vec();
                let fut = device.write_value(DeviceWriteCmd::new(Endpoint::Tx, lovense_cmd, false));
                fut.await?;
                let dir = direction.load(Ordering::SeqCst);
                // TODO Should we store speed and direction as an option for rotation caching? This is weird.
                if dir != clockwise {
                    direction.store(clockwise, Ordering::SeqCst);
                    let fut = device.write_value(DeviceWriteCmd::new(
                        Endpoint::Tx,
                        b"RotateChange;".to_vec(),
                        false,
                    ));
                    fut.await?;
                }
            }
            Ok(messages::Ok::default().into())
        })
    }

    fn handle_battery_level_cmd(
        &self,
        device: Arc<DeviceImpl>,
        message: messages::BatteryLevelCmd,
    ) -> ButtplugDeviceResultFuture {
        let mut subscribe_fut = device.subscribe(DeviceSubscribeCmd::new(Endpoint::Rx));
        Box::pin(async move {
            let mut device_notification_receiver = subscribe_fut.await?;
            let write_fut = device.write_value(DeviceWriteCmd::new(
                Endpoint::Tx,
                b"Battery;".to_vec(),
                false,
            ));
            write_fut.await?;
            if let Ok(data) = device_notification_receiver.recv().await {
                if let Ok(data_str) = std::str::from_utf8(&data) {
                    let len = data_str.len();
                    // Chop the semicolon at the end of the received line.
                    if let Ok(level) = data_str[0..(len - 1)].parse::<u8>() {
                        return Ok(messages::BatteryLevelReading::new(
                            message.device_index,
                            level as f64 / 100f64,
                        )
                        .into());
                    }
                }
            }
            Err(ButtplugDeviceError::DeviceNotConnected(
                "Device disconnected while waiting for battery message.".to_owned(),
            )
            .into())
        })
    }
}

// TODO Gonna need to add the ability to set subscribe data in tests before
// writing Lovense tests. Oops.
