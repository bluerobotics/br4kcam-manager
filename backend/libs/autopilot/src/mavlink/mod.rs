mod connection;
pub mod parameters;

pub use connection::Message;

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use indexmap::IndexMap;
use mavlink::{
    self, MavHeader, Message as _, MessageData,
    ardupilotmega::{COMMAND_LONG_DATA, MavCmd, MavMessage, MavResult, SERVO_OUTPUT_RAW_DATA},
};
use once_cell::sync::OnceCell;
use tokio::sync::{RwLock, broadcast};
use tracing::*;

use crate::{
    health::ObserveSource,
    mavlink::{connection::Connection, parameters::ParamEncodingType},
    parameters::{ParamType, Parameter},
};

/// Process-lifetime owner of MAVLink tasks. Never dropped while the process runs
/// (soft restart keeps the existing connection string).
static MAVLINK_COMPONENT: OnceCell<MavlinkComponent> = OnceCell::new();

/// Shared MAVLink API. Independent of [`crate::manager::MANAGER`] so I/O need not
/// hold the settings lock.
pub fn component() -> Result<&'static MavlinkComponent> {
    MAVLINK_COMPONENT
        .get()
        .context("MAVLink component not initialized")
}

pub(crate) fn init_component(component: MavlinkComponent) -> Result<()> {
    MAVLINK_COMPONENT
        .set(component)
        .map_err(|_| anyhow!("MAVLink component already initialized"))
}

#[derive(Debug)]
pub struct MavlinkComponent {
    pub(crate) inner: Arc<ComponentInner>,
    sender_task_handle: tokio::task::JoinHandle<()>,
    receiver_task_handle: tokio::task::JoinHandle<()>,
    params_sync_task_handle: tokio::task::JoinHandle<()>,
    heartbeat_task_handle: tokio::task::JoinHandle<()>,
}

impl MavlinkComponent {
    pub fn system_id(&self) -> u8 {
        self.inner.system_id
    }

    #[instrument(level = "debug")]
    pub async fn try_new(address: String, system_id: u8, component_id: u8) -> Result<Self> {
        let inner = Arc::new(ComponentInner::try_new(address, system_id, component_id).await?);

        let sender_task_handle = tokio::spawn(Self::sender_task(inner.clone()));
        let receiver_task_handle = tokio::spawn(Self::receiver_task(inner.clone()));
        let heartbeat_task_handle = tokio::spawn(Self::heartbeat_task(inner.clone()));

        Self::configure_parameter_encoding(inner.clone()).await;
        crate::health::set_syncing(true);
        crate::manager::owned_parameters::rebuild().await;
        Self::update_all_params(inner.clone()).await;
        {
            let cache = inner.parameters.read().await;
            crate::manager::owned_parameters::observe_cached(&cache, ObserveSource::Connect);
        }
        crate::health::set_syncing(false);

        let params_sync_task_handle = tokio::spawn(Self::params_sync_task(inner.clone()));

        Ok(Self {
            inner,
            sender_task_handle,
            receiver_task_handle,
            params_sync_task_handle,
            heartbeat_task_handle,
        })
    }

    #[instrument(level = "debug", skip(inner))]
    async fn sender_task(inner: Arc<ComponentInner>) {
        let mut receiver;
        let timeout = std::time::Duration::from_secs(10);

        let mut connection;

        {
            receiver = inner.get_receiver().await;
            connection = inner.connection.clone()
        }

        loop {
            // Receive messages from the local components
            let (header, message) = match receiver.recv().await {
                Ok(Message::ToBeSent(inner)) => inner,
                Ok(Message::Received(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    unreachable!(
                        "Closed channel: This should never happen, this channel is owned by ComponentInner!"
                    );
                }
                Err(broadcast::error::RecvError::Lagged(samples)) => {
                    warn!(
                        "Channel is lagged behind by {samples} messages. Expect degraded performance on the mavlink responsiviness."
                    );

                    continue;
                }
            };

            // Send the response from the local components to the Mavlink network
            if let Err(error) = connection.send(&header, &message, timeout).await {
                error!("Failed sending message to Mavlink Connection: {error:?}");

                continue;
            }
        }
    }

    #[instrument(level = "debug", skip(inner))]
    async fn receiver_task(inner: Arc<ComponentInner>) {
        let sender = inner.get_sender().await;
        let timeout = std::time::Duration::from_secs(10);

        loop {
            // Receive from the Mavlink network
            let (header, message) = inner.connection.clone().recv(timeout).await;

            // Send the received message to the components
            if let Err(error) = sender.send(Message::Received((header, message))) {
                warn!("Failed receiving mavlink message: {error:?}");

                continue;
            }
        }
    }

    #[instrument(level = "debug", skip(inner))]
    async fn heartbeat_task(inner: Arc<ComponentInner>) {
        let sender = inner.get_sender().await;
        let system_id = inner.system_id;
        let component_id = inner.component_id;

        let mut header = MavHeader {
            system_id,
            component_id,
            sequence: 0,
        };

        let message = MavMessage::HEARTBEAT(mavlink::ardupilotmega::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: mavlink::ardupilotmega::MavType::MAV_TYPE_CAMERA,
            autopilot: mavlink::ardupilotmega::MavAutopilot::MAV_AUTOPILOT_INVALID,
            base_mode: mavlink::ardupilotmega::MavModeFlag::empty(),
            system_status: mavlink::ardupilotmega::MavState::MAV_STATE_STANDBY,
            mavlink_version: 0x3,
        });

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

            if let Err(error) = sender.send(Message::ToBeSent((header, message.clone()))) {
                warn!("Failed sending message: {error:?}");
                continue;
            }

            header.sequence = header.sequence.wrapping_add(1);
        }
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn enable_lua_script(&self, overwrite: bool) -> Result<bool> {
        let mut autopilot_reboot_required = overwrite;

        let encoding = self.encoding().await;

        let mut param = self.get_param("SCR_ENABLE", false).await?;
        let old_value = param.value;
        param.value.set_value(ParamType::REAL32(1.0), encoding)?;
        let new_value = param.value;

        if overwrite || old_value != new_value {
            self.set_param(param).await?;
            autopilot_reboot_required = true;
        }

        Ok(autopilot_reboot_required)
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn reload_lua_scripts(&self, overwrite: bool) -> Result<()> {
        let target_system = self.inner.system_id;
        let target_component = mavlink::ardupilotmega::MavComponent::MAV_COMP_ID_AUTOPILOT1 as u8;

        // The fresh run reports its own errors within seconds, so keeping the previous
        // complaint would only leave the user's fix looking like it did nothing.
        crate::health::clear_lua_script_failure();

        const SCRIPTING_CMD_STOP_AND_RESTART: u8 = 3;
        self.send_command(COMMAND_LONG_DATA {
            target_system,
            target_component,
            confirmation: 0,
            command: MavCmd::MAV_CMD_SCRIPTING,
            param1: SCRIPTING_CMD_STOP_AND_RESTART as f32,
            ..Default::default()
        })
        .await
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn reboot_autopilot(&self) -> Result<()> {
        struct RebootingGuard;

        impl Drop for RebootingGuard {
            fn drop(&mut self) {
                crate::health::set_rebooting(false);
            }
        }

        crate::health::set_rebooting(true);
        let _guard = RebootingGuard;

        // This is a workaround to this issue: https://github.com/bluerobotics/br4kcam-manager/issues/57
        blueos_client::reboot_autopilot().await?;

        // FIXME: once the aforementioned issue is fixed, we can use the code below:
        // let target_system = { self.inner.read().await.system_id };
        // let target_component = mavlink::ardupilotmega::MavComponent::MAV_COMP_ID_AUTOPILOT1 as u8;

        // self.send_command(COMMAND_LONG_DATA {
        //     target_system,
        //     target_component,
        //     confirmation: 0,
        //     command: MavCmd::MAV_CMD_PREFLIGHT_REBOOT_SHUTDOWN,
        //     param1: 1.0, // autopilot
        //     ..Default::default()
        // })
        // .await?;

        // Give autopilot some time to shutdown before waiting for it to come back online
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        self.wait_autopilot().await?;

        Self::configure_parameter_encoding(self.inner.clone()).await;
        crate::health::set_syncing(true);
        crate::manager::owned_parameters::rebuild().await;
        if tokio::time::timeout(
            tokio::time::Duration::from_secs(60),
            Self::update_all_params(self.inner.clone()),
        )
        .await
        .is_err()
        {
            warn!("Timed out refreshing parameters after autopilot reboot");
        }
        crate::health::set_syncing(false);

        Ok(())
    }

    pub async fn wait_autopilot(&self) -> Result<()> {
        info!("Waiting for autopilot to come back online...");

        let target_system = self.inner.system_id;
        let target_component = mavlink::ardupilotmega::MavComponent::MAV_COMP_ID_AUTOPILOT1 as u8;
        let mut receiver = self.inner.get_receiver().await;

        let wait_heartbeat = async {
            loop {
                use broadcast::error::RecvError;

                match receiver.recv().await {
                    Ok(Message::Received((recv_header, recv_message)))
                        if recv_header.system_id == target_system
                            && recv_header.component_id == target_component
                            && matches!(recv_message, MavMessage::HEARTBEAT(_)) =>
                    {
                        if let MavMessage::HEARTBEAT(heartbeat) = recv_message {
                            use mavlink::ardupilotmega::MavState;

                            if matches!(
                                heartbeat.system_status,
                                MavState::MAV_STATE_ACTIVE | MavState::MAV_STATE_STANDBY
                            ) {
                                info!(
                                    "Received autopilot {target_system}:{target_component} heartbeat: {heartbeat:?}!"
                                );

                                return Ok(());
                            }
                        }
                    }
                    Ok(_) => continue,
                    Err(RecvError::Closed) => {
                        return Err(anyhow!("Receiver channel closed"));
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!("Receiver lagged by {n} messages");
                        continue;
                    }
                }
            }
        };

        match tokio::time::timeout(tokio::time::Duration::from_secs(15), wait_heartbeat).await {
            Ok(res) => res,
            Err(_) => {
                warn!(
                    "Timeout waiting for autopilot {target_system}:{target_component} heartbeat; continuing"
                );
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn send_command(&self, command: COMMAND_LONG_DATA) -> Result<()> {
        let _txn = self.inner.txn.lock().await;
        self.send_command_locked(command).await
    }

    /// Send a command and wait for ACK. Caller must hold [`ComponentInner::txn`].
    #[instrument(level = "debug", skip(self))]
    async fn send_command_locked(&self, mut command: COMMAND_LONG_DATA) -> Result<()> {
        let target_system = self.inner.system_id;
        let target_component = mavlink::ardupilotmega::MavComponent::MAV_COMP_ID_AUTOPILOT1 as u8;
        let this_system = self.inner.system_id;
        let this_component = self.inner.component_id;
        let sender = self.inner.get_sender().await;
        let mut receiver = self.inner.get_receiver().await;

        let header = MavHeader {
            system_id: this_system,
            component_id: this_component,
            sequence: 0,
        };
        let message = MavMessage::COMMAND_LONG(command.clone());

        let max_retries = 5;
        while command.confirmation < max_retries {
            debug!("Sent command {:?}", command.command);
            sender.send(Message::ToBeSent((header, message.clone())))?;
            command.confirmation += 1;

            let wait_command_ack = async {
                loop {
                    use broadcast::error::RecvError;

                    match receiver.recv().await {
                        Ok(Message::Received((recv_header, recv_message)))
                            if recv_header.system_id == target_system
                                && recv_header.component_id == target_component
                                && matches!(recv_message, MavMessage::COMMAND_ACK(_)) =>
                        {
                            if let MavMessage::COMMAND_ACK(command_ack) = recv_message
                                && command_ack.command == command.command
                            {
                                return match command_ack.result {
                                    MavResult::MAV_RESULT_ACCEPTED => {
                                        debug!("Command {:?} accepted", command_ack.command);
                                        Ok(())
                                    }
                                    reason => {
                                        warn!(
                                            "Command {:?} rejected: {reason:?}",
                                            command_ack.command
                                        );
                                        Err(anyhow!(
                                            "Command {:?} rejected: {reason:?}",
                                            command_ack.command
                                        ))
                                    }
                                };
                            }
                        }
                        Ok(_) => continue,
                        Err(RecvError::Closed) => {
                            return Err(anyhow!("Receiver channel closed"));
                        }
                        Err(RecvError::Lagged(n)) => {
                            warn!("Receiver lagged by {n} messages");
                            continue;
                        }
                    }
                }
            };

            match tokio::time::timeout(tokio::time::Duration::from_secs(1), wait_command_ack).await
            {
                Ok(Ok(())) => {
                    crate::health::rpc_ok();
                    return Ok(());
                }
                Ok(Err(error)) => {
                    if error.to_string().contains("MAV_RESULT_UNSUPPORTED") {
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        continue;
                    }
                    return Err(error);
                }
                Err(_) => {
                    warn!("Timeout for command {:?}, retrying", command.command);
                }
            }
        }

        let reason = format!(
            "Command {:?} timed out after {max_retries} retries",
            command.command
        );
        crate::health::rpc_failed(&reason);
        Err(anyhow!(reason))
    }

    /// Subscribe to the shared MAVLink message broadcast for this component.
    #[instrument(level = "debug", skip(self))]
    pub async fn get_receiver(&self) -> broadcast::Receiver<Message> {
        self.inner.get_receiver().await
    }

    /// Request the autopilot to stream `message_id` at `interval_us` microseconds.
    #[instrument(level = "debug", skip(self))]
    pub async fn set_message_interval(&self, message_id: u32, interval_us: f32) -> Result<()> {
        let target_system = self.inner.system_id;
        let target_component = mavlink::ardupilotmega::MavComponent::MAV_COMP_ID_AUTOPILOT1 as u8;

        self.send_command(COMMAND_LONG_DATA {
            command: MavCmd::MAV_CMD_SET_MESSAGE_INTERVAL,
            target_system,
            target_component,
            confirmation: 0,
            param1: message_id as f32,
            param2: interval_us,
            ..Default::default()
        })
        .await
    }

    /// One-shot request for `SERVO_OUTPUT_RAW`. Holds the mavlink txn for the whole RPC.
    #[instrument(level = "debug", skip(self))]
    pub async fn request_servo_output_raw(&self) -> Result<SERVO_OUTPUT_RAW_DATA> {
        let target_system = self.inner.system_id;
        let target_component = mavlink::ardupilotmega::MavComponent::MAV_COMP_ID_AUTOPILOT1 as u8;

        let _txn = self.inner.txn.lock().await;
        // Subscribe before send so the first SERVO frame cannot be missed.
        let mut receiver = self.inner.get_receiver().await;

        self.send_command_locked(COMMAND_LONG_DATA {
            command: MavCmd::MAV_CMD_REQUEST_MESSAGE,
            target_system,
            target_component,
            confirmation: 0,
            param1: SERVO_OUTPUT_RAW_DATA::ID as f32,
            ..Default::default()
        })
        .await?;

        Self::wait_servo_output_raw_on(&mut receiver, target_system, target_component).await
    }

    async fn wait_servo_output_raw_on(
        receiver: &mut broadcast::Receiver<Message>,
        target_system: u8,
        target_component: u8,
    ) -> Result<SERVO_OUTPUT_RAW_DATA> {
        let wait_message = async {
            loop {
                use broadcast::error::RecvError;

                match receiver.recv().await {
                    Ok(Message::Received((recv_header, recv_message)))
                        if recv_header.system_id == target_system
                            && recv_header.component_id == target_component
                            && recv_message.message_id() == SERVO_OUTPUT_RAW_DATA::ID =>
                    {
                        if let MavMessage::SERVO_OUTPUT_RAW(servo_output_raw) = recv_message {
                            if servo_output_raw.port == 0 {
                                return Ok(servo_output_raw);
                            }

                            continue;
                        }
                    }
                    Ok(_) => continue,
                    Err(RecvError::Closed) => {
                        return Err(anyhow!("Receiver channel closed"));
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!("Receiver lagged by {n} messages");
                        continue;
                    }
                }
            }
        };

        match tokio::time::timeout(tokio::time::Duration::from_secs(1), wait_message).await {
            Ok(res) => res,
            Err(_) => {
                crate::health::rpc_failed("SERVO_OUTPUT_RAW not delivered within 1s after ACK");
                Err(anyhow!("Timeout waiting"))
            }
        }
    }
}

impl Drop for MavlinkComponent {
    fn drop(&mut self) {
        self.sender_task_handle.abort();
        self.receiver_task_handle.abort();
        self.heartbeat_task_handle.abort();
        self.params_sync_task_handle.abort();
    }
}

pub(crate) struct ComponentInner {
    pub system_id: u8,
    pub component_id: u8,
    pub encoding: Arc<RwLock<ParamEncodingType>>,
    pub parameters: Arc<RwLock<IndexMap<String, Parameter>>>,
    /// Serializes correlated MAVLink RPCs (command ACK / PARAM_VALUE matching).
    /// Lock order: never acquire MANAGER while holding this.
    pub txn: tokio::sync::Mutex<()>,
    connection: Connection,
}

impl std::fmt::Debug for ComponentInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentInner")
            .field("system_id", &self.system_id)
            .field("component_id", &self.component_id)
            .finish()
    }
}

impl ComponentInner {
    #[instrument(level = "debug")]
    pub async fn try_new(address: String, system_id: u8, component_id: u8) -> Result<Self> {
        use mavlink::ardupilotmega::*;

        let mut connection = Connection::new(address.clone()).await;

        // Send initial heartbeat to establish proper connection tracking state
        //
        // WHY THIS IS CRITICAL:
        // When running in Docker, Linux conntrack drops "unsolicited" UDP replies
        // because they don't match any existing connection state. By sending this
        // first packet, we create a conntrack entry that allows the kernel to
        // recognize BlueOS's responses as valid replies rather than invalid traffic.
        //
        // Without this:
        // 1. BlueOS sends MAVLink data to our ephemeral port
        // 2. Kernel sees it as "invalid" (no matching conntrack entry)
        // 3. Packets are silently dropped before reaching our socket
        //
        // This is NOT a MAVLink protocol requirement - it's a Linux networking
        // behavior specific to Docker containers. Bare-metal UDP clients don't
        // need this, but Dockerized ones do due to NAT and connection tracking.
        {
            let header = MavHeader {
                system_id,
                component_id,
                sequence: 0,
            };
            let heartbeat = MavMessage::HEARTBEAT(HEARTBEAT_DATA {
                custom_mode: 0,
                mavtype: MavType::MAV_TYPE_CAMERA,
                autopilot: MavAutopilot::MAV_AUTOPILOT_INVALID,
                base_mode: MavModeFlag::empty(),
                system_status: MavState::MAV_STATE_STANDBY,
                mavlink_version: 3,
            });

            connection
                .send(&header, &heartbeat, std::time::Duration::from_secs(2))
                .await
                .context("Failed to send initial heartbeat")?;

            connection.set_reconnect_heartbeat(header, heartbeat).await;

            info!("Initial heartbeat sent.");
        }

        Ok(Self {
            system_id,
            component_id,
            encoding: Arc::new(RwLock::new(ParamEncodingType::default())),
            parameters: Arc::new(RwLock::new(IndexMap::with_capacity(2048))),
            txn: tokio::sync::Mutex::new(()),
            connection,
        })
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn get_sender(&self) -> broadcast::Sender<Message> {
        self.connection.get_sender()
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn get_receiver(&self) -> broadcast::Receiver<Message> {
        self.connection.get_receiver()
    }
}

pub(crate) fn reconnect_count() -> u64 {
    connection::reconnect_count()
}
