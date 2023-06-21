use std::collections::HashMap;
use sifis_dht::domocache::DomoEvent;
use sifis_dht::utils::get_epoch_ms;
use std::error::Error;

use crate::{command_parser, get_topic_from_actuator_topic};

pub enum DHTCommand {
    ActuatorCommand(serde_json::Value),
    ValveCommand(serde_json::Value),
}


pub struct ConnElem {
    pub source_topic_name: String,
    pub  source_topic_uuid: String,
    pub target_channel_number: u64
}

pub struct DHTManager {
    pub cache: sifis_dht::domocache::DomoCache,
    pub actuators_index: HashMap<String, Vec<ConnElem>>
}

impl DHTManager {
    pub async fn new(cache_config: sifis_config::Cache) -> Result<DHTManager, Box<dyn Error>> {
        let sifis_cache = sifis_dht::domocache::DomoCache::new(cache_config).await?;
        let actuators_index = HashMap::new();
        Ok(DHTManager { cache: sifis_cache, actuators_index })
    }


    pub async fn update_actuator_connections(&mut self, topic_name: &str, topic_uuid: &str, actuator_topic: &serde_json::Value){
        let k = topic_name.to_owned() +"-"+ topic_uuid;

        if let Some(conns) = self.actuators_index.get(&k) {
            for conn in conns {


                if let Ok(status) = get_topic_from_actuator_topic(
                    self,
                    &conn.source_topic_name,
                    &conn.source_topic_uuid,
                    conn.target_channel_number,
                    actuator_topic,
                    topic_name
                ).await
                {

                    println!("Updating");
                    println!("{} {} ", conn.source_topic_name, conn.source_topic_uuid);

                    self.cache
                        .write_value(&conn.source_topic_name, &conn.source_topic_uuid, status)
                        .await;
                }
            }
        }
    }

    pub async fn build_actuators_index(&mut self) -> Result<(), Box<dyn Error>> {

        println!("BUILD ACT INDEX");

        self.actuators_index.clear();

        let connections = self.cache.get_topic_name("domo_actuator_connection").unwrap();

        for conn in connections.as_array().unwrap().iter() {
            if let Some(value) = conn.get("value") {
                if let Some(target_topic_name) = value.get("target_topic_name") {
                    if let Some(target_topic_uuid) = value.get("target_topic_uuid") {
                        if let Some(target_channel_number) = value.get("target_channel_number") {
                            if let Some(source_topic_name) = value.get("source_topic_name") {
                                let target_topic_name = target_topic_name.as_str().unwrap();
                                let target_topic_uuid = target_topic_uuid.as_str().unwrap();
                                let target_channel_number = target_channel_number.as_u64().unwrap();
                                let source_topic_name = source_topic_name.as_str().unwrap();
                                let source_topic_uuid = conn["topic_uuid"].as_str().unwrap();

                                let c: ConnElem = ConnElem{
                                    source_topic_name: source_topic_name.to_string(),
                                    source_topic_uuid: source_topic_uuid.to_string(),
                                    target_channel_number
                                };

                                let k = target_topic_name.to_owned() + "-" + target_topic_uuid;

                                if let Some(conns) = self.actuators_index.get_mut(&k) {
                                    conns.push(c)
                                } else {
                                    let mut v: Vec<ConnElem> = vec![];
                                    v.push(c);
                                    self.actuators_index.insert(k, v);
                                }
                            }
                        }
                    }
                }
            }
        }


        println!("ACTUATORS_INDEX");
        for (k, v) in &self.actuators_index {
            println!("{}", k);
            for c in v {
                println!(" {} {} ", c.source_topic_name, c.source_topic_uuid);
            }
        }


        Ok(())

    }

    pub async fn get_auth_cred(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        let shelly_plus_topic_names = vec!["shelly_1plus", "shelly_1pm_plus", "shelly_2pm_plus"];

        for topic in shelly_plus_topic_names {
            let shelly_plus_topics = self.cache.get_topic_name(topic)?;

            let topics = shelly_plus_topics.as_array().unwrap();
            for t in topics.iter() {
                if let Some(value) = t.get("value") {
                    if let Some(user_login) = value.get("user_login") {
                        if let Some(user_password) = value.get("user_password") {
                            let user_login_str = user_login.as_str().unwrap();
                            let user_password_str = user_password.as_str().unwrap();
                            if user_login_str == user && user_password_str == password {
                                let mac = value.get("mac_address").unwrap().as_str().unwrap();
                                if let Some(topic_name) = t.get("topic_name") {
                                    let topic_name = topic_name.as_str().unwrap().to_owned();
                                    let json_ret = serde_json::json!({ "mac_address": mac, "topic": topic_name });
                                    return Ok(json_ret);
                                }
                            }
                        }
                    }
                }
            }
        }

        Err("cred not found".into())
    }

    pub fn get_topic(
        &mut self,
        topic_name: &str,
        mac_address: &str,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        if let Ok(actuators) = self.cache.get_topic_name(topic_name) {
            for act in actuators.as_array().unwrap() {
                if let Some(value) = act.get("value") {
                    if let Some(mac) = value.get("mac_address") {
                        if mac == mac_address {
                            return Ok(act.to_owned());
                        }
                    }
                }
            }
        }

        Err("act not found".into())
    }

    pub async fn get_actuator_from_mac_address(
        &mut self,
        mac_address_req: &str,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        let actuator_topics = [
            "shelly_1",
            "shelly_1pm",
            "shelly_1plus",
            "shelly_em",
            "shelly_1pm_plus",
            "shelly_2pm_plus",
            "shelly_25",
            "shelly_dimmer",
            "shelly_rgbw",
            "domo_ble_thermometer",
            "domo_ble_valve",
            "domo_ble_contact",
        ];

        for act_type in actuator_topics {
            if let Ok(actuators) = self.cache.get_topic_name(act_type) {
                for act in actuators.as_array().unwrap() {
                    if let Some(value) = act.get("value") {
                        if let Some(mac) = value.get("mac_address") {
                            if mac == mac_address_req {
                                return Ok(act.to_owned());
                            }
                        }
                    }
                }
            }
        }

        Err("err".into())
    }

    pub async fn write_topic(
        &mut self,
        topic_name: &str,
        topic_uuid: &str,
        value: &serde_json::Value,
    ) {
        self.cache
            .write_value(topic_name, topic_uuid, value.to_owned())
            .await;
    }

    async fn handle_volatile_command(
        &self,
        command: serde_json::Value,
    ) -> Result<DHTCommand, Box<dyn Error>> {
        if let Some(command) = command.get("command") {
            if let Some(command_type) = command.get("command_type") {
                if command_type == "shelly_actuator_command" {
                    if let Some(value) = command.get("value") {
                        return Ok(DHTCommand::ActuatorCommand(value.to_owned()));
                    }
                }

                if command_type == "radiator_valve_command" {
                    if let Some(value) = command.get("value") {
                        return Ok(DHTCommand::ValveCommand(value.to_owned()));
                    }
                }

                if command_type == "turn_command" {
                    return command_parser::handle_turn_command(self, command).await;
                }

                if command_type == "valve_command" {
                    return command_parser::handle_valve_command(self, command).await;
                }

                if command_type == "dim_command" {
                    return command_parser::handle_dim_command(self, command).await;
                }

                if command_type == "rgbw_command" {
                    return command_parser::handle_rgbw_command(self, command).await;
                }

                if command_type == "shutter_command" {
                    return command_parser::handle_shutter_command(self, command).await;
                }
            }
        }

        Err("not able to parse message".into())
    }

    pub async fn wait_dht_messages(&mut self) -> Result<DHTCommand, Box<dyn Error>> {
        let data = self.cache.cache_event_loop().await?;

        if let DomoEvent::VolatileData(m) = data {
            println!(
                "WAIT_DHT_MESSAGES RECEIVED COMMAND {} {}",
                m,
                get_epoch_ms()
            );
            return self.handle_volatile_command(m.to_owned()).await;
        }

        if let DomoEvent::PersistentData(m) = data {
            if m.topic_name == "domo_actuator_connection" {
                self.build_actuators_index().await?;
            }
        }

        Err("not a volatile message".into())
    }
}
