use crate::bleutils::ContactStatus;
use crate::dhtmanager::{DHTCommand, DHTManager};
use crate::globalshellymanager::GlobalShellyManager;
use crate::messages::{AuthCredMessage, BleBeaconMessage, ESP32CommandMessage, ESP32CommandType};
use crate::shellymanager::ShellyManager;
use crate::utils::{ValveCommandManager, ValveData};
use crate::wssmanager::WssManager;
use clap::Parser;
use futures_util::{pin_mut, stream::StreamExt};
use mdns::{Record, RecordKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Number};
use sifis_config::{Cache, ConfigParser};
use sifis_dht::utils::get_epoch_ms;
use std::error::Error;
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::time::Interval;

mod bleutils;
mod command_parser;
mod dhtmanager;
mod globalshellymanager;
mod messages;
mod shellymanager;
mod utils;
mod wssmanager;

const SERVICE_NAME: &str = "_webthing._tcp.local";

pub struct ShellyDiscoveryResult {
    pub ip_address: String,
    pub topic_name: String,
    pub mac_address: String,
    pub mdns_name: String,
}

struct PingManager {
    ping_timer: Interval,
}

impl PingManager {
    pub fn new(period_secs: u64) -> PingManager {
        let interval = tokio::time::interval(tokio::time::Duration::from_secs(period_secs));
        PingManager {
            ping_timer: interval,
        }
    }

    pub async fn wait_ping_timer(&mut self) {
        self.ping_timer.tick().await;
    }
}

#[derive(Parser, Debug, Serialize, Deserialize)]
struct DomoWotBridge {
    #[clap(flatten)]
    pub cache: Cache,

    /// node_id
    #[arg(short, long, default_value_t = 1)]
    pub node_id: u8,
}

#[derive(Parser, Debug, Serialize, Deserialize)]
struct Opt {
    #[clap(flatten)]
    domo_wot_bridge: DomoWotBridge,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let opt = ConfigParser::<Opt>::new()
        .with_config_path("/etc/domo/domo_wot_bridge.toml")
        .parse();

    let opt = opt.domo_wot_bridge;

    env_logger::init();

    let mut ping_mgr = PingManager::new(10);

    let mut check_shelly_mode = PingManager::new(10);

    let mut check_radiator_valve_commands = PingManager::new(20);

    let mut shelly_plus_actuators = vec![];

    let mut valve_command_manager = ValveCommandManager::new();

    let mut shelly_manager = GlobalShellyManager::new().await;

    let mut dht_manager = dhtmanager::DHTManager::new(opt.cache).await?;

    dht_manager.build_actuators_index().await?;



    let mut wss_mgr = WssManager::new(5000).await;

    let stream = mdns::discover::interface(
        SERVICE_NAME,
        Duration::from_secs(30),
        Ipv4Addr::new(10, 0, opt.node_id, 1),
    )?
    .listen();

    pin_mut!(stream);

    let mut counter = 0;
    loop {
        counter += 1;
        tokio::select! {
            Some(auth_cred_message) = wss_mgr.rx_auth_cred.recv() => {
                    //println!("Received auth cred from esp32");
                    let ret = handle_cred_message(auth_cred_message, &mut dht_manager).await;
                    if let Ok(m) = ret {
                        if let Some(mac_address) = m.get("mac_address") {
                            if let Some(topic) = m.get("topic") {
                                //println!("TOPIC {} mac_address {}", topic, mac_address.to_string());
                                let topic = topic.as_str().unwrap().to_owned();
                                if topic == "shelly_1plus" || topic == "shelly_1pm_plus" || topic == "shelly_2pm_plus" {
                                    println!("Shelly plus {}, {} connected" , topic, mac_address);
                                    shelly_plus_actuators.push(mac_address.as_str().unwrap().to_owned());
                                }
                            }
                        }
                    }
            },

            esp32_actuator_update = wss_mgr.channel_of_actuator_updates_rx.recv() => {
                //println!("Received esp32 actuator update");
                if let Ok(msg) = esp32_actuator_update {
                    handle_shelly_message(msg, &mut dht_manager).await;
                }
            }
            // listener for ble beacons adv

            ble_update = wss_mgr.channel_of_updates_rx.recv() => {

                println!("Received ble beacon update");

                if let Ok(msg) = ble_update {
                    handle_ble_update_message(msg, &mut dht_manager, &mut valve_command_manager).await;
                }

            },

            // mdns await
            res = stream.next() =>  {

                println!("Received mdns message");

                if let Some(Ok(response)) = res {

                    let shelly_res = response.records()
                                     .filter_map(get_shelly_discovery_result)
                                     .next();

                    if let Some(shelly) = shelly_res {

                        println!("{} {} {}", shelly.topic_name, shelly.mac_address, shelly.ip_address);

                        let topic = dht_manager.get_actuator_from_mac_address(&shelly.mac_address).await;
                        match topic {
                            Ok(t) => {

                                    if let Some(value) = t.get("value"){

                                        if let Some(user_login) = value.get("user_login"){
                                            if let Some(user_password) = value.get("user_password") {

                                                let user_login_str = user_login.as_str();
                                                let user_password_str = user_password.as_str();

                                                if let Some(user) = user_login_str {
                                                    if let Some(password) = user_password_str {
                                                        shelly_manager.insert_shelly(shelly, user.to_owned(), password.to_owned()).await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                            },
                            Err(_e) => {
                            }
                        }

                    }
                }

            },
            _ = check_radiator_valve_commands.wait_ping_timer() => {
                //println!("RADIATOR VALVE QUEUE CHECK");
                if !valve_command_manager.valve_commands.is_empty() && !shelly_plus_actuators.is_empty() {

                    let mut to_remove = vec![];
                    let valves = dht_manager.cache.get_topic_name("domo_ble_valve").unwrap();

                    let valve_commands = valve_command_manager.valve_commands.clone();

                    for (key, val) in valve_commands {
                            let mut ok = false;
                                for valve in valves.as_array().unwrap() {
                                    if let Some(value) = valve.get("value") {
                                        if let Some(mac_address) = value.get("mac_address") {
                                            let mac = mac_address.as_str().unwrap();
                                            if mac == key {
                                                if let Some(status) = value.get("status") {
                                                   let status = status.as_bool().unwrap();
                                                    //println!("Status: {} ", status);
                                                    //println!("Desired state: {}", val.desired_state);

                                                    if let Some(desired_state) = val.desired_state.get("desired_state") {
                                                        let desired_state = desired_state.as_bool().unwrap();
                                                        if status == desired_state {
                                                            //println!("Removing valve command from queue");
                                                            to_remove.push(key.clone());
                                                            ok = true;
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                        if ok {
                            continue;
                        } else if val.attempts < 100 {

                            if let Some(next_act_mac) = valve_command_manager.get_best_actuator_for_valve(&key) {
                                //println!("RE-SEND VALVE COMMAND TO {}", next_act_mac.clone());
                                let cmd = ESP32CommandMessage {
                                        command_type: ESP32CommandType::Valve,
                                        mac_address: key.to_string(),
                                        payload: val.desired_state.clone(),
                                        actuator_mac_address: next_act_mac
                                };

                                let _ret = wss_mgr.command_channel_tx.send(cmd);

                                let mut val = val.clone();
                                val.attempts += 1;
                                valve_command_manager.valve_commands.insert(key, val);
                            }

                        }
                        else {
                            to_remove.push(key.clone());
                        }

                    }

                    for r in to_remove {
                        valve_command_manager.remove(&r);
                    }

                }
            },
            _ = ping_mgr.wait_ping_timer() => {
                println!("PING_TIMER {}", counter);

                shelly_manager.send_ping().await;
                shelly_manager.check_if_reconnect_needed().await;

                let cmd = ESP32CommandMessage {
                                                command_type: ESP32CommandType::Ping,
                                                actuator_mac_address: String::from(""),
                                                mac_address: String::from(""),
                                                payload: json!({})
                                        };

                let _ret = wss_mgr.command_channel_tx.send(cmd);

            },
            _ = check_shelly_mode.wait_ping_timer() => {
                log::info!("CHECK_SHELLY_MODE {}", counter);

                if let Ok(actuator_connections) = dht_manager.cache.get_topic_name("domo_actuator_connection") {
                    let actuator_connections = actuator_connections.as_array().unwrap();

                    check_shelly_esp8266_mode(actuator_connections, &mut shelly_manager, &mut dht_manager).await;

                    check_shelly_esp32_mode(actuator_connections, &shelly_plus_actuators, &mut dht_manager, &mut wss_mgr).await;

                }
            },
            command = dht_manager.wait_dht_messages() => {

                if let Ok(cmd) = command {
                        //println!("Received command from dht {}", get_epoch_ms());
                        match cmd {
                            DHTCommand::ActuatorCommand(value) => {

                                //println!("Received actuator command");

                                if let Some(mac_address) = value.get("mac_address") {
                                    let mac_string = mac_address.as_str().unwrap();
                                    let cmd = ESP32CommandMessage {
                                        command_type: ESP32CommandType::Actuator,
                                        mac_address: mac_string.to_owned(),
                                        payload: value.clone(),
                                        actuator_mac_address: String::from("")
                                    };

                                    let _ret = wss_mgr.command_channel_tx.send(cmd);

                                }

                                handle_shelly_command(value, &mut dht_manager, &mut shelly_manager).await;
                            }
                            DHTCommand::ValveCommand(value) => {

                                if !shelly_plus_actuators.is_empty() {
                                    if let Some(mac_address) = value.get("mac_address") {

                                        //println!("Valve command {}", value);

                                        let mac_string = mac_address.as_str().unwrap();

                                        if let Some(best_act) = valve_command_manager.get_best_actuator_for_valve(mac_string) {

                                            let vd = ValveData {
                                                desired_state: value.clone(),
                                                attempts: 1
                                            };

                                            valve_command_manager.insert(mac_string, vd);

                                            let cmd = ESP32CommandMessage {
                                                command_type: ESP32CommandType::Valve,
                                                mac_address: mac_string.to_owned(),
                                                payload: value,
                                                actuator_mac_address: best_act.clone()
                                            };

                                            //println!("SENDING VALVE COMMAND TO {} ", best_act);

                                            let _ret = wss_mgr.command_channel_tx.send(cmd);
                                        } else {
                                            //println!("NO ACTUATOR for {} ", mac_string);

                                            let vd = ValveData {
                                                desired_state: value.clone(),
                                                attempts: 0
                                            };

                                            valve_command_manager.insert(mac_string, vd);

                                        }

                                    }
                                }
                            }
                        }
                    }
            },

            shelly_message = shelly_manager.wait_for_shelly_message() => {

                if let Ok(message) = shelly_message {
                        handle_shelly_message(message, &mut dht_manager).await;
                }
            }

        }
    }
}

async fn handle_cred_message(
    auth_cred_message: AuthCredMessage,
    dht_manager: &mut DHTManager,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let ret = dht_manager
        .get_auth_cred(&auth_cred_message.user, &auth_cred_message.pass)
        .await;

    match ret {
        Ok(m) => {
            let _r = auth_cred_message.responder.send(Ok(m.clone()));
            Ok(m)
        }
        _ => {
            let _r = auth_cred_message
                .responder
                .send(Err("cred not found".to_owned()));
            Err("cred not found".into())
        }
    }
}

async fn handle_shelly_message(shelly_message: serde_json::Value, dht_manager: &mut DHTManager) {
    println!("Received shelly message {}", get_epoch_ms());

    if let Some(message_type) = shelly_message.get("messageType") {
        if message_type.as_str().unwrap() == "propertyStatus" {
            if let Some(data) = shelly_message.get("data") {
                if let Some(status) = data.get("status") {
                    let status_string = status.as_str().unwrap();

                    if let Ok(status_result) =
                        serde_json::from_str::<serde_json::Value>(status_string)
                    {
                        let mac_address =
                            status_result.get("mac_address").unwrap().as_str().unwrap();

                        let mac_address_with_points = mac_address[0..2].to_owned()
                            + ":"
                            + &mac_address[2..4]
                            + ":"
                            + &mac_address[4..6]
                            + ":"
                            + &mac_address[6..8]
                            + ":"
                            + &mac_address[8..10]
                            + ":"
                            + &mac_address[10..12];

                        let topic_name = status_result.get("topic_name").unwrap().as_str().unwrap();

                        if let Ok(topic) =
                            dht_manager.get_topic(topic_name, &mac_address_with_points)
                        {
                            let mut new_status = status_result.clone();

                            if let Some(value) = topic.get("value") {
                                if let Some(user_login) = value.get("user_login") {
                                    let user_login = user_login.as_str().unwrap();

                                    if let Some(user_password) = value.get("user_password") {
                                        let user_password = user_password.as_str().unwrap();
                                        if let Some(mac_address) = value.get("mac_address") {
                                            let mac_address = mac_address.as_str().unwrap();
                                            if let Some(id) = value.get("id") {
                                                new_status["user_login"] =
                                                    serde_json::Value::String(
                                                        user_login.to_owned(),
                                                    );
                                                new_status["user_password"] =
                                                    serde_json::Value::String(
                                                        user_password.to_string(),
                                                    );

                                                new_status["mac_address"] =
                                                    serde_json::Value::String(
                                                        mac_address.to_string(),
                                                    );

                                                new_status["id"] = id.to_owned();

                                                new_status["last_update_timestamp"] =
                                                    serde_json::Value::Number(Number::from(
                                                        sifis_dht::utils::get_epoch_ms() as u64,
                                                    ));

                                                let topic_uuid =
                                                    topic["topic_uuid"].as_str().unwrap();
                                                dht_manager
                                                    .write_topic(
                                                        topic_name,
                                                        topic_uuid,
                                                        &new_status,
                                                    )
                                                    .await;

                                                let _ret = update_actuator_connection(
                                                    dht_manager,
                                                    topic_name,
                                                    topic_uuid,
                                                    &new_status,
                                                )
                                                .await;
                                                println!("DOMO: UPDATE TOPICS {}", get_epoch_ms());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn get_topic_from_actuator_topic(
    dht_manager: &DHTManager,
    source_topic_name: &str,
    source_topic_uuid: &str,
    channel_number: u64,
    actuator_topic: &serde_json::Value,
    target_topic_name: &str,
) -> Result<serde_json::Value, Box<dyn Error>> {
    //println!("ACTUATOR_TOPIC {}", actuator_topic);
    let mut source_topic = dht_manager
        .cache
        .get_topic_uuid(source_topic_name, source_topic_uuid)?;

    let channel_number_str = channel_number.to_string();

    let channel_number_str = channel_number_str.as_str();

    if source_topic_name == "domo_power_energy_sensor" {
        let updated_props = actuator_topic["updated_properties"].as_array().unwrap();

        let mut found = false;
        for prop in updated_props {
            let prop = prop.as_str().unwrap();
            if prop == "power_data" {
                found = true;
            }
        }

        if !found {
            return Err("not update".into());
        }

        source_topic["value"]["power"] = actuator_topic["power_data"]
            ["channel".to_owned() + channel_number_str]["active_power"]
            .clone();

        let old_ene = source_topic["value"]["energy"].as_f64();

        let mut old_value: f64 = 0.0;
        if let Some(old_ene) = old_ene {
            old_value = old_ene;
        }

        let current_ene = actuator_topic["power_data"]["channel".to_owned() + channel_number_str]
            ["energy"]
            .as_f64()
            .unwrap();

        let total_ene = old_value + current_ene;

        source_topic["value"]["energy"] = serde_json::Value::from(total_ene);

        let props = vec![
            serde_json::Value::String("power".to_owned()),
            serde_json::Value::String("energy".to_owned()),
        ];

        source_topic["value"]["updated_properties"] = serde_json::Value::Array(props);
    }

    if source_topic_name == "domo_light_dimmable" {
        if target_topic_name == "shelly_dimmer" {
            source_topic["value"]["status"] = actuator_topic["dimmer_status"].clone();
            source_topic["value"]["power"] = actuator_topic["power1"].clone();

            let old_ene = source_topic["value"]["energy"].as_f64();

            let mut old_value: f64 = 0.0;
            if let Some(old_ene) = old_ene {
                old_value = old_ene;
            }

            let current_ene = actuator_topic["energy1"].as_f64().unwrap();

            let total_ene = old_value + current_ene;

            source_topic["value"]["energy"] = serde_json::Value::from(total_ene);

            let updated_props = actuator_topic["updated_properties"].as_array().unwrap();

            let mut props = Vec::new();

            for prop in updated_props {
                if prop == "power1" {
                    props.push(serde_json::Value::String("power".to_owned()));
                }
                if prop == "energy1" {
                    props.push(serde_json::Value::String("energy".to_owned()));
                }
            }

            source_topic["value"]["updated_properties"] = serde_json::Value::Array(props);
        } else if target_topic_name == "shelly_rgbw" {
            let rgbw_status_value_string = actuator_topic["rgbw_status"]
                .as_str()
                .ok_or("Error in rgbw_status")?;

            let rgbw_status: serde_json::Value = serde_json::from_str(rgbw_status_value_string)?;

            let _val = 0;
            if channel_number == 1 {
                source_topic["value"]["status"] = rgbw_status["r"].clone();
            }
            if channel_number == 2 {
                source_topic["value"]["status"] = rgbw_status["g"].clone();
            }

            if channel_number == 3 {
                source_topic["value"]["status"] = rgbw_status["b"].clone();
            }

            if channel_number == 4 {
                source_topic["value"]["status"] = rgbw_status["w"].clone();
            }
        }
    }

    if source_topic_name == "domo_rgbw_light" {
        let rgbw_status_value_string = actuator_topic["rgbw_status"]
            .as_str()
            .ok_or("Error in rgbw_status")?;

        let rgbw_status: serde_json::Value = serde_json::from_str(rgbw_status_value_string)?;

        println!(
            "HERE DOMO_RGBW_LIGHT {} {} {} {}",
            rgbw_status["r"], rgbw_status["g"], rgbw_status["b"], rgbw_status["w"]
        );

        source_topic["value"]["r"] = rgbw_status["r"].clone();
        source_topic["value"]["g"] = rgbw_status["g"].clone();
        source_topic["value"]["b"] = rgbw_status["b"].clone();
        source_topic["value"]["w"] = rgbw_status["w"].clone();
    }

    if source_topic_name == "domo_light"
        || source_topic_name == "domo_siren"
        || source_topic_name == "domo_switch"
    {
        source_topic["value"]["status"] =
            actuator_topic["output".to_owned() + channel_number_str].clone();

        if target_topic_name != "shelly_1" && target_topic_name != "shelly_1plus" {
            source_topic["value"]["power"] =
                actuator_topic["power".to_owned() + channel_number_str].clone();

            let old_ene = source_topic["value"]["energy"].as_f64();

            let mut old_value: f64 = 0.0;
            if let Some(old_ene) = old_ene {
                old_value = old_ene;
            }

            let current_ene = actuator_topic["energy".to_owned() + channel_number_str]
                .as_f64()
                .unwrap();

            let total_ene = old_value + current_ene;

            source_topic["value"]["energy"] = serde_json::Value::from(total_ene);
        }

        let updated_props = actuator_topic["updated_properties"].as_array().unwrap();

        //println!("UPDATED PROPS {:?}", updated_props);

        let mut props = Vec::new();

        for prop in updated_props {
            let prop_str = prop.as_str().unwrap();

            //println!("prop_str {}", prop_str);

            if prop_str == ("power".to_owned() + channel_number_str) {
                //println!("pushing power {}", channel_number_str);
                props.push(serde_json::Value::String("power".to_owned()));
            }

            if prop_str == ("energy".to_owned() + channel_number_str) {
                //println!("pushing power {}", channel_number_str);
                props.push(serde_json::Value::String("energy".to_owned()));
            }
        }

        source_topic["value"]["updated_properties"] = serde_json::Value::Array(props);
    }

    if source_topic_name == "domo_floor_valve" {
        source_topic["value"]["status"] =
            actuator_topic["output".to_owned() + channel_number_str].clone();
    }

    if source_topic_name == "domo_roller_shutter" || source_topic_name == "domo_garage_gate" {
        source_topic["value"]["shutter_status"] = actuator_topic["shutter_status"].clone();
    }

    if source_topic_name == "domo_pir_sensor"
        || source_topic_name == "domo_radar_sensor"
        || source_topic_name == "domo_button"
        || source_topic_name == "domo_bistable_button"
    {
        let updated_props = actuator_topic["updated_properties"].as_array().unwrap();

        //println!(
        //    "UPDATED_PROPS {:?} channel_number_str {}",
        //    updated_props, channel_number_str
        //);

        let mut found = false;
        for prop in updated_props {
            let prop = prop.as_str().unwrap();
            if prop == ("input".to_owned() + channel_number_str) {
                source_topic["value"]["status"] =
                    actuator_topic["input".to_owned() + channel_number_str].clone();
                found = true;
            }
        }

        if !found {
            return Err("not update".into());
        }
    }

    if source_topic_name == "domo_window_sensor" || source_topic_name == "domo_door_sensor" {
        if target_topic_name == "domo_ble_contact" {
            source_topic["value"]["status"] = actuator_topic["status"].clone();
        } else {
            source_topic["value"]["status"] =
                actuator_topic["input".to_owned() + channel_number_str].clone();
        }
    }

    Ok(source_topic["value"].clone())
}

async fn update_actuator_connection(
    dht_manager: &mut DHTManager,
    topic_name: &str,
    topic_uuid: &str,
    actuator_topic: &serde_json::Value,
) -> Result<(), Box<dyn Error>> {

    dht_manager.update_actuator_connections(&topic_name, &topic_uuid, actuator_topic).await;

    Ok(())
}

async fn handle_shelly_command(
    shelly_command: serde_json::Value,
    _dht_manager: &mut DHTManager,
    shelly_manager: &mut GlobalShellyManager,
) {
    if let Some(mac_address) = shelly_command.get("mac_address") {
        if let Some(shelly_action_payload) = shelly_command.get("shelly_action") {
            let shelly_action = serde_json::json!({ "shelly_action": shelly_action_payload });

            let message = serde_json::json!({
                "messageType": "requestAction",
                "data": shelly_action
            });

            let mac_address_str = mac_address.as_str().unwrap();

            //println!("DOMO: SENDING ACTION {}", get_epoch_ms());

            let _ret = shelly_manager.send_action(mac_address_str, &message).await;

            println!("DOMO: SENT ACTION {}", get_epoch_ms());
        }
    }
}

pub fn get_shelly_discovery_result(record: &Record) -> Option<ShellyDiscoveryResult> {
    if record.name.contains("shelly_1plus") {
        return None;
    }

    if record.name.contains("shelly_1pm_plus") {
        return None;
    }

    if record.name.contains("shelly_2pm_plus") {
        return None;
    }

    if !record.name.contains("shelly") && !record.name.contains("geeklink") {
        return None;
    }

    let record_name = record.name.replace(".local", "");

    match record.kind {
        RecordKind::A(addr) => {
            let name_parts: Vec<&str> = record_name.split('-').collect();
            let topic_name = name_parts[0];
            let mac_address = name_parts[1].to_owned();

            let mac_address_with_points = mac_address[0..2].to_owned()
                + ":"
                + &mac_address[2..4]
                + ":"
                + &mac_address[4..6]
                + ":"
                + &mac_address[6..8]
                + ":"
                + &mac_address[8..10]
                + ":"
                + &mac_address[10..12];

            let res = ShellyDiscoveryResult {
                ip_address: addr.to_string(),
                topic_name: topic_name.to_string(),
                mac_address: mac_address_with_points,
                mdns_name: record.name.to_owned(),
            };
            Some(res)
        }
        RecordKind::AAAA(addr) => {
            let name_parts: Vec<&str> = record.name.split('-').collect();
            let topic_name = name_parts[0];
            let mac_address = name_parts[1];
            let res = ShellyDiscoveryResult {
                ip_address: addr.to_string(),
                topic_name: topic_name.to_string(),
                mac_address: mac_address.to_string(),
                mdns_name: record.name.to_owned(),
            };
            Some(res)
        }
        _ => None,
    }
}

async fn handle_ble_update_message(
    message: BleBeaconMessage,
    dht_manager: &mut DHTManager,
    valve_command_manager: &mut ValveCommandManager,
) {
    let ret = dht_manager
        .get_actuator_from_mac_address(&message.mac_address)
        .await;

    if let Ok(topic) = ret {
        let topic_name = topic["topic_name"].as_str().unwrap();

        if topic_name == "domo_ble_thermometer" {
            //println!("THERMO UPDATE {}", message.payload);

            if let Ok(bytes) = base64::decode(&message.payload) {
                use hex::ToHex;
                let beacon_adv_string = bytes.encode_hex::<String>();
                //println!("BEACON THERMO ADV from {}: {}", message.mac_address, beacon_adv_string);

                handle_ble_thermometer_update(
                    dht_manager,
                    &message.mac_address,
                    &beacon_adv_string,
                    &topic,
                )
                .await;
            }
        }

        if topic_name == "domo_ble_contact" {
            //println!("CONTACT UPDATE {}", message.payload);

            if let Ok(bytes) = base64::decode(&message.payload) {
                use hex::ToHex;
                let beacon_adv_string = bytes.encode_hex::<String>();
                //println!("BEACON CONTACT ADV from {}: {}", message.mac_address, beacon_adv_string);

                handle_ble_contact_update(
                    dht_manager,
                    &message.mac_address,
                    &beacon_adv_string,
                    &message.rssi,
                    &topic,
                )
                .await;
            }
        }

        if topic_name == "domo_ble_valve" {
            if message.payload == "0" || message.payload == "1" {
                handle_ble_valve_update(
                    dht_manager,
                    &message.mac_address,
                    &message.payload,
                    &topic,
                )
                .await;
            } else {
                // update best actuator to use for valve depending on rssi
                valve_command_manager.update_best_actuator(
                    &message.mac_address,
                    &message.actuator,
                    message.rssi,
                );
            }
        }
    }
}

async fn handle_ble_thermometer_update(
    dht_manager: &mut DHTManager,
    _mac_address: &str,
    message: &str,
    topic: &serde_json::Value,
) {
    let topic_uuid = topic["topic_uuid"].as_str().unwrap();
    let value_of_topic = &topic["value"];
    let token = value_of_topic["token"].as_str().unwrap();
    let mac_address = value_of_topic["mac_address"].as_str().unwrap();
    let name = value_of_topic["name"].as_str().unwrap();
    let area_name = value_of_topic["area_name"].as_str().unwrap();

    let ret = bleutils::parse_atc(mac_address, message, token);

    if let Ok(m) = ret {
        //println!("DECRITTATO {} {} {}", m.temperature, m.humidity, m.battery);
        let value = serde_json::json!({
            "temperature": m.temperature,
            "humidity": m.humidity,
            "battery":  m.battery,
            "token": token,
            "mac_address": mac_address,
            "last_update_timestamp": serde_json::Value::Number(Number::from(sifis_dht::utils::get_epoch_ms() as u64)),
            "name": name,
            "area_name": area_name
        });

        dht_manager
            .write_topic("domo_ble_thermometer", topic_uuid, &value)
            .await;
    }
}

async fn handle_ble_contact_update(
    dht_manager: &mut DHTManager,
    _mac_address: &str,
    message: &str,
    rssi: &i64,
    topic: &serde_json::Value,
) {
    if message.len() >= 58 {
        println!("MESSAGE: {}", message);
        let topic_uuid = topic["topic_uuid"].as_str().unwrap();
        let value_of_topic = &topic["value"];
        let token = value_of_topic["token"].as_str().unwrap();
        let id = value_of_topic["id"].as_str().unwrap();
        let mac_address = value_of_topic["mac_address"].as_str().unwrap();
        let area_name = value_of_topic["area_name"].as_str().unwrap();

        let len_hex_value = "1d";
        let rssi_i = *rssi as i8;
        let rssi_hex = format!("{:02x}", rssi_i);

        let rssi_hex = rssi_hex.as_str();

        let data = len_hex_value.to_owned() + message + rssi_hex;

        //println!("mac {} token {} payload {}", mac_address, token, message);

        let ret = bleutils::parse_contact_sensor(mac_address, &data, token);
        if let Ok(m) = ret {
            let val = u64::from(m.state != ContactStatus::Open);
            //println!("Value_of_topic {}", value_of_topic);
            if let Some(val_in_topic) = value_of_topic.get("status") {
                //println!("{}", val_in_topic);
                let val_in_topic = val_in_topic.as_u64().unwrap();
                //println!("val {}, value_of_topic {}", val, val_in_topic);
                if val != val_in_topic {
                    let value = serde_json::json!({
                    "status": val,
                    "token": token,
                    "last_update_timestamp": serde_json::Value::Number(Number::from(sifis_dht::utils::get_epoch_ms() as u64)),
                    "mac_address": mac_address,
                        "id": id,
                        "area_name": area_name
                     });

                    dht_manager
                        .write_topic("domo_ble_contact", topic_uuid, &value)
                        .await;
                    let _ret = update_actuator_connection(
                        dht_manager,
                        "domo_ble_contact",
                        topic_uuid,
                        &value,
                    )
                    .await;
                }
            } else {
                let value = serde_json::json!({
                "status": val,
                "token": token,
                "mac_address": mac_address,
                "last_update_timestamp": serde_json::Value::Number(Number::from(sifis_dht::utils::get_epoch_ms() as u64)),
                "id": id,
                "area_name": area_name
                });

                dht_manager
                    .write_topic("domo_ble_contact", topic_uuid, &value)
                    .await;
                let _ret =
                    update_actuator_connection(dht_manager, "domo_ble_contact", topic_uuid, &value)
                        .await;
            }
        }
    }
}

async fn handle_ble_valve_update(
    dht_manager: &mut DHTManager,
    _mac_address: &str,
    message: &String,
    topic: &serde_json::Value,
) {
    let topic_uuid = topic["topic_uuid"].as_str().unwrap();
    let value_of_topic = &topic["value"];
    let mac_address = value_of_topic["mac_address"].as_str().unwrap();
    let name = value_of_topic["name"].as_str().unwrap();
    let area_name = value_of_topic["area_name"].as_str().unwrap();

    let value: bool = message == "1";

    let value = serde_json::json!(
    {   "status": value,
        "mac_address": mac_address,
        "last_update_timestamp": serde_json::Value::Number(Number::from(sifis_dht::utils::get_epoch_ms() as u64)),
        "name": name,
        "area_name": area_name
    });

    dht_manager
        .write_topic("domo_ble_valve", topic_uuid, &value)
        .await;
}

async fn calculate_mode(
    act_connections: &Vec<serde_json::Value>,
    act_topic_name: &str,
    act_topic_uuid: &str,
) -> u64 {
    if [
        "shelly_1",
        "shelly_1plus",
        "shelly_1pm",
        "shelly_em",
        "shelly_1pm_plus",
    ]
    .contains(&act_topic_name)
    {
        return 0; // RELAY
    }

    if ["shelly_dimmer"].contains(&act_topic_name) {
        return 2; // DIMMER
    }

    let mut ret = 0;

    if act_topic_name == "shelly_25" || act_topic_name == "shelly_2pm_plus" {
        ret = 0; // RELAY
    }

    if act_topic_name == "shelly_rgbw" {
        ret = 4; // LED_DIMMER
    }

    for conn in act_connections {
        if let Some(value) = conn.get("value") {
            if let Some(connection_type) = value.get("connection_type") {
                let connection_type = connection_type.as_str().unwrap();
                if connection_type != "output" {
                    continue;
                }
            }

            if let Some(target_topic_name) = value.get("target_topic_name") {
                if let Some(target_topic_uuid) = value.get("target_topic_uuid") {
                    if let Some(source_topic_name) = value.get("source_topic_name") {
                        if let Some(source_topic_uuid) = value.get("source_topic_uuid") {
                            let target_topic_name = target_topic_name.as_str().unwrap();
                            let target_topic_uuid = target_topic_uuid.as_str().unwrap();
                            let source_topic_name = source_topic_name.as_str().unwrap();
                            let _source_topic_uuid = source_topic_uuid.as_str().unwrap();

                            if target_topic_uuid == act_topic_uuid
                                && target_topic_name == act_topic_name
                            {
                                if (target_topic_name == "shelly_25"
                                    || target_topic_name == "shelly_2pm_plus")
                                    && (source_topic_name == "domo_roller_shutter"
                                        || source_topic_name == "domo_garage_gate")
                                {
                                    return 1; // SHUTTER
                                }
                                if target_topic_name == "shelly_rgbw"
                                    && source_topic_name == "domo_rgbw_light"
                                {
                                    return 3; // RGBW
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    ret
}

async fn check_shelly_esp8266_mode(
    actuator_connections: &Vec<serde_json::Value>,
    shelly_manager: &mut GlobalShellyManager,
    dht_manager: &mut DHTManager,
) {
    let mut to_remove = Vec::new();
    for (idx, act) in &mut shelly_manager.shelly_list.iter_mut().enumerate() {
        if let Ok(topic_of_act) = dht_manager
            .get_actuator_from_mac_address(&act.mac_address)
            .await
        {
            if let Some(value) = topic_of_act.get("value") {
                if let Some(mode) = value.get("mode") {
                    let mode = mode.as_u64().unwrap();
                    let act_topic_name = topic_of_act["topic_name"].as_str().unwrap();
                    let act_topic_uuid = topic_of_act["topic_uuid"].as_str().unwrap();
                    let desired_mode =
                        calculate_mode(actuator_connections, act_topic_name, act_topic_uuid).await;

                    let mut inverted = false;
                    if let Some(inv) = value.get("inverted") {
                        inverted = inv.as_bool().unwrap();
                    }

                    if mode != desired_mode {
                        //println!(
                        //    "Change mode of {} {} to {} ",
                        //    act_topic_name, act_topic_uuid, desired_mode
                        //);

                        let action_payload = serde_json::json!({
                            "mode": desired_mode,
                            "inverted": inverted
                        });

                        let action_payload_string = action_payload.to_string();

                        let shelly_action = serde_json::json!({
                            "shelly_action" : {
                                "input" : {
                                    "action": {
                                        "action_name": "change_mode",
                                        "action_payload": action_payload_string
                                    }
                                }
                            }
                        });

                        let message = serde_json::json!({
                            "messageType": "requestAction",
                            "data": shelly_action
                        });

                        act.send_action(&message).await;
                        to_remove.push(idx);
                    }
                }
            }
        }
    }

    for id in to_remove {
        shelly_manager.shelly_list.remove(id);
    }
}

async fn check_shelly_esp32_mode(
    actuator_connections: &Vec<serde_json::Value>,
    shelly_plus_list: &Vec<String>,
    dht_manager: &mut DHTManager,
    wss_mgr: &mut WssManager,
) {
    for act in shelly_plus_list {
        if let Ok(topic_of_act) = dht_manager.get_actuator_from_mac_address(act).await {
            if let Some(value) = topic_of_act.get("value") {
                if let Some(mode) = value.get("mode") {
                    let mode = mode.as_u64().unwrap();
                    let act_topic_name = topic_of_act["topic_name"].as_str().unwrap();
                    let act_topic_uuid = topic_of_act["topic_uuid"].as_str().unwrap();
                    let desired_mode =
                        calculate_mode(actuator_connections, act_topic_name, act_topic_uuid).await;

                    let mut inverted = false;
                    if let Some(inv) = value.get("inverted") {
                        inverted = inv.as_bool().unwrap();
                    }

                    if mode != desired_mode {
                        //println!(
                        //    "Change mode of {} {} to {} ",
                        //    act_topic_name, act_topic_uuid, desired_mode
                        //);
                        let action_payload = serde_json::json!({
                            "mode": desired_mode,
                            "inverted": inverted
                        });

                        let action_payload_string = action_payload.to_string();

                        let shelly_action = serde_json::json!({
                            "shelly_action" : {
                                "input" : {
                                    "action": {
                                        "action_name": "change_mode",
                                        "action_payload": action_payload_string
                                    }
                                }
                            }
                        });

                        let cmd = ESP32CommandMessage {
                            command_type: ESP32CommandType::Actuator,
                            mac_address: act.to_owned(),
                            payload: shelly_action.clone(),
                            actuator_mac_address: String::from(""),
                        };

                        let _ret = wss_mgr.command_channel_tx.send(cmd);
                    }
                }
            }
        }
    }
}
