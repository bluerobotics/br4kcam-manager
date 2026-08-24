#[macro_export]
macro_rules! generate_update_channel_param_function {
    (
        $fn_name:ident,
        $field_name:ident,
        $param_prefix:expr,
        $param_suffix:expr,
        $ty:ident,
        $channel_field:ident
    ) => {
        #[instrument(level = "debug", skip(parameters))]
        async fn $fn_name(
            camera_uuid: &Uuid,
            parameters: &$crate::api::ActuatorsParametersConfig,
            force_apply: bool,
        ) -> Result<()> {
            // Snapshot under a short write with no await inside, so the MAVLink I/O
            // below never runs while MANAGER is locked.
            let (param_name, new_value, old_value) = {
                let mut manager = $crate::manager::MANAGER
                    .get()
                    .context("Not available")?
                    .write()
                    .await;
                let current_parameters = &mut manager
                    .settings
                    .actuators
                    .entry(*camera_uuid)
                    .or_default()
                    .parameters;
                let channel = current_parameters.$channel_field as u8;
                let param_name = format!("{}{}_{}", $param_prefix, channel, $param_suffix);
                let new_value = match (parameters.$field_name, force_apply) {
                    (Some(value), _) => value,
                    (None, true) => current_parameters.$field_name,
                    (None, false) => return Ok(()),
                };
                let old_value = current_parameters.$field_name;
                (param_name, new_value, old_value)
            };

            if (old_value == new_value) && !force_apply {
                trace!("Parameter {param_name:?} skipped");
                return Ok(());
            }

            let mavlink = $crate::mavlink::component()?;
            let encoding = mavlink.encoding().await;
            let mut param = mavlink.get_param(&param_name, false).await?;
            param.value.set_value(ParamType::$ty(new_value), encoding)?;

            match mavlink.set_param(param).await {
                Ok(_) => {
                    if old_value != new_value {
                        info!(
                            "{} changed from {:?} to {:?}",
                            stringify!($field_name),
                            old_value,
                            new_value
                        );
                    }
                    let mut manager = $crate::manager::MANAGER
                        .get()
                        .context("Not available")?
                        .write()
                        .await;
                    if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid) {
                        actuators.parameters.$field_name = new_value;
                    }
                }
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "Failed setting parameter {}: {error:?}",
                        stringify!($field_name)
                    ));
                }
            }

            Ok(())
        }
    };
}

#[macro_export]
macro_rules! generate_update_mount_param_function {
    (
        $fn_name:ident,
        $expect_fn:ident,
        $field_name:ident,
        $param_suffix:expr,
        $ty:ident
    ) => {
        #[instrument(level = "debug", skip(parameters))]
        pub async fn $fn_name(
            camera_uuid: &Uuid,
            parameters: &$crate::api::ActuatorsParametersConfig,
            force_apply: bool,
        ) -> Result<bool> {
            let (param_name, new_value, old_value) = {
                let mut manager = $crate::manager::MANAGER
                    .get()
                    .context("Not available")?
                    .write()
                    .await;
                let current_parameters = &mut manager
                    .settings
                    .actuators
                    .entry(*camera_uuid)
                    .or_default()
                    .parameters;
                let mount_id = $crate::manager::tilt::tilt_mount_id(current_parameters.camera_id);
                let param_name = format!("{mount_id:?}_{}", $param_suffix);
                let new_value = match (parameters.$field_name, force_apply) {
                    (Some(value), _) => value,
                    (None, true) => current_parameters.$field_name,
                    (None, false) => return Ok(false),
                };
                let old_value = current_parameters.$field_name;
                (param_name, new_value, old_value)
            };

            if (old_value == new_value) && !force_apply {
                trace!("Parameter {param_name:?} skipped");
                return Ok(false);
            }

            let mavlink = $crate::mavlink::component()?;
            let encoding = mavlink.encoding().await;
            let mut param = mavlink.get_param(&param_name, false).await?;
            param.value.set_value(ParamType::$ty(new_value), encoding)?;

            match mavlink.set_param(param).await {
                Ok(_) => {
                    if old_value != new_value {
                        info!(
                            "{} changed from {:?} to {:?}",
                            stringify!($field_name),
                            old_value,
                            new_value
                        );
                    }
                    let mut manager = $crate::manager::MANAGER
                        .get()
                        .context("Not available")?
                        .write()
                        .await;
                    if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid) {
                        actuators.parameters.$field_name = new_value;
                    }
                    Ok(old_value != new_value)
                }
                Err(error) => Err(anyhow::anyhow!(
                    "Failed setting parameter {}: {error:?}",
                    stringify!($field_name)
                )),
            }
        }

        fn $expect_fn(
            parameters: &$crate::parameters::ActuatorsParameters,
            map: &mut indexmap::IndexMap<String, $crate::parameters::ParamType>,
        ) {
            let mount_id = $crate::manager::tilt::tilt_mount_id(parameters.camera_id);
            map.insert(
                format!("{mount_id:?}_{}", $param_suffix),
                $crate::parameters::ParamType::$ty(parameters.$field_name),
            );
        }
    };
}
