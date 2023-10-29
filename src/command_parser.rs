use crate::dhtmanager::{DHTCommand, DHTManager};
use std::error::Error;

pub async fn handle_turn_command(
    dht_manager: &DHTManager,
    command: &serde_json::Value,
) -> Result<DHTCommand, Box<dyn Error>> {
    //println!("DOMO: handle_turn_command {}", get_epoch_ms());
    if let Some(value) = command.get("value") {
        let mut topic_uuid = "";
        let mut desired_state = false;

        if let Some(t_uuid) = value.get("topic_uuid") {
            if let Some(t) = t_uuid.as_str() {
                topic_uuid = t;
            } else {
                return Err("err_topic_uuid".into());
            }
        }

        if let Some(d_state) = value.get("desired_state") {
            if let Some(d) = d_state.as_bool() {
                desired_state = d;
            } else {
                return Err("err_desired_state".into());
            }
        }

        let dht_connection_topic = dht_manager
            .cache
            .get_topic_uuid("domo_actuator_connection", topic_uuid)?;

        //println!("Connection {}", dht_connection_topic);

        if dht_connection_topic.get("value").is_none() {
            return Err("no connection".into());
        }

        let dht_connection_topic = dht_connection_topic.get("value").unwrap();

        if let Some(t_topic_name) = dht_connection_topic.get("target_topic_name") {
            if let Some(t_topic_uuid) = dht_connection_topic.get("target_topic_uuid") {
                if let Some(t_channel_number) = dht_connection_topic.get("target_channel_number") {
                    let target_topic_name: &str;
                    let target_topic_uuid: &str;
                    let target_channel_number: u64;

                    if let Some(t) = t_topic_name.as_str() {
                        target_topic_name = t;
                    } else {
                        return Err("err_target_topic_name".into());
                    }

                    if let Some(t) = t_topic_uuid.as_str() {
                        target_topic_uuid = t;
                    } else {
                        return Err("err_target_topic_uuid".into());
                    }

                    if let Some(t) = t_channel_number.as_u64() {
                        target_channel_number = t;
                    } else {
                        return Err("err_target_channel_number".into());
                    }

                    //println!(
                    //    "DOMO: handle_turn_command got actuator connection fields {} {}",
                    //    target_topic_name, target_topic_uuid
                    //);

                    let actuator_topic = dht_manager
                        .cache
                        .get_topic_uuid(target_topic_name, target_topic_uuid)?;

                    if let Some(value) = actuator_topic.get("value") {
                        if let Some(mac_address) = value.get("mac_address") {
                            let action_payload = serde_json::json!({
                                "output_number": target_channel_number,
                                "value": desired_state
                            });

                            let value = serde_json::json!({
                                "mac_address": mac_address,
                                "shelly_action": {
                                  "input": {
                                    "action": {
                                      "action_name": "set_output",
                                      "action_payload": action_payload.to_string(),
                                    },
                                  },
                                }
                            });

                            //println!("DOMO: RETURN ACTUATOR COMMAND {}", get_epoch_ms());
                            return Ok(DHTCommand::ActuatorCommand(value));
                        }
                    }
                }
            }
        }
    }

    //println!("DOMO: SKIPPING COMMAND");
    Err("not_able_to_parse_command".into())
}

pub async fn handle_shutter_command(
    dht_manager: &DHTManager,
    command: &serde_json::Value,
) -> Result<DHTCommand, Box<dyn Error>> {
    if let Some(value) = command.get("value") {
        let topic_uuid = value.get("topic_uuid").unwrap().as_str().unwrap();
        let shutter_command = value.get("shutter_command").unwrap().as_str().unwrap();

        let dht_connection_topic = dht_manager
            .cache
            .get_topic_uuid("domo_actuator_connection", topic_uuid)?;

        let dht_connection_topic = dht_connection_topic.get("value").unwrap();

        if let Some(target_topic_name) = dht_connection_topic.get("target_topic_name") {
            if let Some(target_topic_uuid) = dht_connection_topic.get("target_topic_uuid") {
                if let Some(_target_channel_number) =
                    dht_connection_topic.get("target_channel_number")
                {
                    let target_topic_name = target_topic_name.as_str().unwrap();
                    let target_topic_uuid = target_topic_uuid.as_str().unwrap();

                    let actuator_topic = dht_manager
                        .cache
                        .get_topic_uuid(target_topic_name, target_topic_uuid)?;

                    if let Some(value) = actuator_topic.get("value") {
                        if let Some(mac_address) = value.get("mac_address") {
                            let mut shut_cmd = 0;
                            if shutter_command == "up" {
                                shut_cmd = 0;
                            }

                            if shutter_command == "down" {
                                shut_cmd = 1;
                            }

                            if shutter_command == "stop" {
                                shut_cmd = 2;
                            }

                            let action_payload = serde_json::json!({
                                "shutter_command": shut_cmd,
                            });

                            let value = serde_json::json!({
                                "mac_address": mac_address,
                                "shelly_action": {
                                  "input": {
                                    "action": {
                                      "action_name": "set_shutter",
                                      "action_payload": action_payload.to_string(),
                                    },
                                  },
                                }
                            });

                            return Ok(DHTCommand::ActuatorCommand(value));
                        }
                    }
                }
            }
        }
    }

    Err("not_able_to_parse_command".into())
}

pub async fn handle_dim_command(
    dht_manager: &DHTManager,
    command: &serde_json::Value,
) -> Result<DHTCommand, Box<dyn Error>> {
    if let Some(value) = command.get("value") {
        let topic_uuid = value.get("topic_uuid").unwrap().as_str().unwrap();
        let desired_state = value.get("desired_state").unwrap().as_u64().unwrap();

        let dht_connection_topic = dht_manager
            .cache
            .get_topic_uuid("domo_actuator_connection", topic_uuid)?;

        if dht_connection_topic.get("value").is_none() {
            return Err("no connection".into());
        }

        let dht_connection_topic = dht_connection_topic.get("value").unwrap();

        if let Some(target_topic_name) = dht_connection_topic.get("target_topic_name") {
            if let Some(target_topic_uuid) = dht_connection_topic.get("target_topic_uuid") {
                if let Some(target_channel_number) =
                    dht_connection_topic.get("target_channel_number")
                {
                    let target_topic_name = target_topic_name.as_str().unwrap();
                    let target_topic_uuid = target_topic_uuid.as_str().unwrap();
                    let target_channel_number = target_channel_number.as_u64().unwrap();

                    let actuator_topic = dht_manager
                        .cache
                        .get_topic_uuid(target_topic_name, target_topic_uuid)?;

                    if let Some(value) = actuator_topic.get("value") {
                        if let Some(mac_address) = value.get("mac_address") {
                            if target_topic_name == "shelly_dimmer" {
                                let action_payload =
                                    serde_json::json!({ "dim_value": desired_state });

                                let value = serde_json::json!({
                                    "mac_address": mac_address,
                                    "shelly_action": {
                                      "input": {
                                        "action": {
                                          "action_name": "set_dimmer",
                                          "action_payload": action_payload.to_string(),
                                        },
                                      },
                                    }
                                });
                                return Ok(DHTCommand::ActuatorCommand(value));
                            }

                            if target_topic_name == "shelly_rgbw" {
                                let mut channel = "r";
                                if target_channel_number == 1 {
                                    channel = "r";
                                }

                                if target_channel_number == 2 {
                                    channel = "g";
                                }

                                if target_channel_number == 3 {
                                    channel = "b";
                                }

                                if target_channel_number == 4 {
                                    channel = "w";
                                }

                                let action_payload = serde_json::json!({
                                    "led_dimmer_status": {
                                            "channel": channel,
                                            "value": desired_state
                                    }

                                });

                                let value = serde_json::json!({
                                    "mac_address": mac_address,
                                    "shelly_action": {
                                      "input": {
                                        "action": {
                                          "action_name": "set_led_dimmer",
                                          "action_payload": action_payload.to_string(),
                                        },
                                      },
                                    }
                                });
                                return Ok(DHTCommand::ActuatorCommand(value));
                            }
                        }
                    }
                }
            }
        }
    }

    Err("not_able_to_parse_command".into())
}

pub async fn handle_rgbw_command(
    dht_manager: &DHTManager,
    command: &serde_json::Value,
) -> Result<DHTCommand, Box<dyn Error>> {
    if let Some(value) = command.get("value") {
        let topic_uuid = value.get("topic_uuid").unwrap().as_str().unwrap();
        let desired_state: serde_json::Value = value.get("desired_state").unwrap().to_owned();

        let dht_connection_topic = dht_manager
            .cache
            .get_topic_uuid("domo_actuator_connection", topic_uuid)?;

        if let Some(dht_connection_topic) = dht_connection_topic.get("value") {
            if let Some(target_topic_name) = dht_connection_topic.get("target_topic_name") {
                if let Some(target_topic_uuid) = dht_connection_topic.get("target_topic_uuid") {
                    if let Some(_target_channel_number) =
                        dht_connection_topic.get("target_channel_number")
                    {
                        let target_topic_name = target_topic_name.as_str().unwrap();
                        let target_topic_uuid = target_topic_uuid.as_str().unwrap();

                        let actuator_topic = dht_manager
                            .cache
                            .get_topic_uuid(target_topic_name, target_topic_uuid)?;

                        if let Some(value) = actuator_topic.get("value") {
                            if let Some(mac_address) = value.get("mac_address") {
                                let action_payload = serde_json::json!({
                                    "rgbw_status": {
                                        "r_value": desired_state.get("r_value").unwrap().as_u64().unwrap(),
                                        "g_value": desired_state.get("g_value").unwrap().as_u64().unwrap(),
                                        "b_value": desired_state.get("b_value").unwrap().as_u64().unwrap(),
                                        "w_value": desired_state.get("w_value").unwrap().as_u64().unwrap()
                                    }
                                });

                                let value = serde_json::json!({
                                    "mac_address": mac_address,
                                    "shelly_action": {
                                      "input": {
                                        "action": {
                                          "action_name": "set_rgbw",
                                          "action_payload": action_payload.to_string(),
                                        },
                                      },
                                    }
                                });

                                return Ok(DHTCommand::ActuatorCommand(value));
                            }
                        }
                    }
                }
            }
        }
    }

    Err("not_able_to_parse_command".into())
}

pub async fn handle_valve_command(
    dht_manager: &DHTManager,
    command: &serde_json::Value,
) -> Result<DHTCommand, Box<dyn Error>> {
    if let Some(value) = command.get("value") {
        let topic_uuid = value.get("topic_uuid").unwrap().as_str().unwrap();
        let desired_state = value.get("desired_state").unwrap().as_bool().unwrap();

        let valve_topic = dht_manager
            .cache
            .get_topic_uuid("domo_ble_valve", topic_uuid)?;

        if let Some(value) = valve_topic.get("value") {
            if let Some(mac_address) = value.get("mac_address") {
                let action_payload = serde_json::json!({
                    "mac_address": mac_address,
                    "value": desired_state
                });

                let value = serde_json::json!({
                    "mac_address": mac_address,
                    "desired_state": desired_state,
                    "shelly_action": {
                      "input": {
                        "action": {
                          "action_name": "control_radiator_valve",
                          "action_payload": action_payload.to_string(),
                        },
                      },
                    }
                });

                return Ok(DHTCommand::ValveCommand(value));
            }
        }
    }

    Err("not_able_to_parse_command".into())
}
