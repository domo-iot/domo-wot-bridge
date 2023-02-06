use crate::dhtmanager::{DHTCommand, DHTManager};
use crate::globalshellymanager::GlobalShellyManager;
use crate::shellymanager::ShellyManager;
use mdns::{Record, RecordKind};
use std::error::Error;
use std::time::Duration;
use tokio::time::Interval;

use crate::bleutils::ContactStatus;
use crate::messages::{AuthCredMessage, BleBeaconMessage, ESP32CommandMessage, ESP32CommandType};
use crate::wssmanager::WssManager;
use futures_util::{pin_mut, stream::StreamExt};

use std::net::Ipv4Addr;

mod bleutils;
mod dhtmanager;
mod globalshellymanager;
mod messages;
mod shellymanager;
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
    pub fn new() -> PingManager {
        let interval = tokio::time::interval(tokio::time::Duration::from_secs(2));
        PingManager {
            ping_timer: interval,
        }
    }

    pub async fn wait_ping_timer(&mut self) {
        self.ping_timer.tick().await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut ping_mgr = PingManager::new();

    let mut shelly_manager = GlobalShellyManager::new().await;

    let mut dht_manager = dhtmanager::DHTManager::new("ws://localhost:3000/ws").await;

    let mut wss_mgr = WssManager::new(5000).await;

    let stream = mdns::discover::interface(
        SERVICE_NAME,
        Duration::from_secs(5),
        Ipv4Addr::new(10, 0, 1, 1),
    )?
    .listen();

    pin_mut!(stream);

    let mut counter = 0;
    loop {
        println!("Waiting {counter}");
        counter += 1;
        tokio::select! {

            Some(auth_cred_message) = wss_mgr.rx_auth_cred.recv() => {
                    println!("Received auth cred from esp32");
                    handle_cred_message(auth_cred_message, &mut dht_manager).await;
            }

            esp32_actuator_update = wss_mgr.channel_of_actuator_updates_rx.recv() => {
                if let Ok(msg) = esp32_actuator_update {
                    handle_shelly_message(msg, &mut dht_manager).await;
                }
            }
            // listener for ble beacons adv
            ble_update = wss_mgr.channel_of_updates_rx.recv() => {

                if let Ok(msg) = ble_update {
                    println!( "HERE {}", msg.mac_address);
                    handle_ble_update_message(msg, &mut dht_manager).await;
                }

            },
            // mdns await
            res = stream.next() =>  {
                println!("stream.next");
                if let Some(Ok(response)) = res {
                    println!("{response:?}");


                    let shelly_res = response.records()
                                     .filter_map(get_shelly_discovery_result)
                                     .next();



                    if let Some(shelly) = shelly_res {

                        println!("{} {} {}", shelly.topic_name, shelly.mac_address, shelly.ip_address);

                        let topic = dht_manager.get_topic(&shelly.mac_address).await;
                        match topic {
                            Ok(t) => {
                                    println!("{t}");

                                    if let Some(value) = t.get("value"){
                                        println!("{value}");
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
            _ = ping_mgr.wait_ping_timer() => {
                println!("wait_ping_timer");
                dht_manager.send_ping().await;
                println!("ping message to domo-dht-manager sent");

                shelly_manager.send_ping().await;
                println!("ping message to shellys sent");


                if dht_manager.last_pong_timestamp.elapsed().unwrap().as_secs() > 6 {
                    println!("Trying reconnect to dht");
                    dht_manager.reconnect().await;
                }

                println!("Executed dht check if reconnect");

                shelly_manager.check_if_reconnect_needed().await;

                println!("Executed check if reconnect");


            },
            command = dht_manager.wait_dht_messages() => {
                println!("wait_dht_messages");
                // received a command
                if let Ok(cmd) = command {
                    match cmd {
                        DHTCommand::ActuatorCommand(value) => {
                            println!("Received command for shelly: {value}");


                            if let Some(mac_address) = value.get("mac_address") {

                                let mac_string = mac_address.as_str().unwrap();

                                let cmd = ESP32CommandMessage {
                                    command_type: ESP32CommandType::ActuatorCommand,
                                    mac_address: mac_string.to_owned(),
                                    payload: value.clone()
                                };

                                let _ret = wss_mgr.command_channel_tx.send(cmd);

                            }

                            handle_shelly_command(value, &mut dht_manager, &mut shelly_manager).await;
                        }
                        DHTCommand::ValveCommand(value) => {
                            if let Some(mac_address) = value.get("mac_address") {

                                let mac_string = mac_address.as_str().unwrap();

                                let cmd = ESP32CommandMessage {
                                    command_type: ESP32CommandType::ValveCommand,
                                    mac_address: mac_string.to_owned(),
                                    payload: value
                                };

                                let _ret = wss_mgr.command_channel_tx.send(cmd);

                            }
                        }
                    }
                }

            },

            shelly_message = shelly_manager.wait_for_shelly_message() => {
                println!("wait_for_shelly_message");
                if let Ok(message) = shelly_message {
                    println!("From shelly: {message}");
                    handle_shelly_message(message, &mut dht_manager).await;
                }
            }

        }
    }
}

async fn handle_cred_message(auth_cred_message: AuthCredMessage, dht_manager: &mut DHTManager) {
    let ret = dht_manager
        .get_auth_cred(&auth_cred_message.user, &auth_cred_message.pass)
        .await;

    match ret {
        Ok(m) => {
            let _r = auth_cred_message.responder.send(Ok(m));
        }
        _ => {
            let _r = auth_cred_message
                .responder
                .send(Err("cred not found".to_owned()));
        }
    }
}

async fn handle_shelly_message(shelly_message: serde_json::Value, dht_manager: &mut DHTManager) {
    if let Some(message_type) = shelly_message.get("messageType") {
        if message_type.as_str().unwrap() == "propertyStatus" {
            if let Some(data) = shelly_message.get("data") {
                if let Some(status) = data.get("status") {
                    let status_string = status.as_str().unwrap();

                    let status_result: serde_json::Value =
                        serde_json::from_str(status_string).unwrap();

                    let mac_address = status_result.get("mac_address").unwrap().as_str().unwrap();

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

                    let topic = dht_manager.get_topic(&mac_address_with_points).await;

                    let mut new_status = status_result.clone();

                    if let Ok(topic_res) = topic {
                        if let Some(value) = topic_res.get("value") {
                            if let Some(user_login) = value.get("user_login") {
                                let user_login = user_login.as_str().unwrap();

                                if let Some(user_password) = value.get("user_password") {
                                    let user_password = user_password.as_str().unwrap();

                                    new_status["user_login"] =
                                        serde_json::Value::String(user_login.to_owned());
                                    new_status["user_password"] =
                                        serde_json::Value::String(user_password.to_string());

                                    println!("New status {new_status}");
                                    let client = reqwest::Client::new();
                                    let _res = client
                                        .post(
                                            "http://localhost:3000/topic_name/".to_owned()
                                                + topic_name
                                                + "/topic_uuid/"
                                                + &mac_address_with_points,
                                        )
                                        .json(&new_status)
                                        .send()
                                        .await;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
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

            let _ret = shelly_manager.send_action(mac_address_str, &message).await;
        }
    }
}

pub fn get_shelly_discovery_result(record: &Record) -> Option<ShellyDiscoveryResult> {
    if record.name.contains("shelly_1plus") {
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

async fn handle_ble_update_message(message: BleBeaconMessage, dht_manager: &mut DHTManager) {
    let ret = dht_manager.get_topic(&message.mac_address).await;
    if let Ok(topic) = ret {
        println!("Search for {} got {}", &message.mac_address, topic);

        let topic_name = topic["topic_name"].as_str().unwrap();

        if topic_name == "ble_thermometer" {
            handle_ble_thermometer_update(
                dht_manager,
                &message.mac_address,
                &message.payload,
                &topic,
            )
            .await;
        }

        if topic_name == "ble_contact" {
            handle_ble_contact_update(
                dht_manager,
                &message.mac_address,
                &message.payload,
                &message.rssi,
                &topic,
            )
            .await;
        }

        if topic_name == "ble_valve" && (message.payload == "0" || message.payload == "1") {
            handle_ble_valve_update(dht_manager, &message.mac_address, &message.payload, &topic)
                .await;
        }
    }
}

async fn handle_ble_thermometer_update(
    dht_manager: &mut DHTManager,
    mac_address: &str,
    message: &str,
    topic: &serde_json::Value,
) {
    let value_of_topic = &topic["value"];
    let token = value_of_topic["token"].as_str().unwrap();

    let ret = bleutils::parse_atc(mac_address, message, token);

    if let Ok(m) = ret {
        let value = serde_json::json!({
            "temperature": m.temperature,
            "humidity": m.humidity,
            "battery":  m.battery,
            "token": token
        });

        dht_manager
            .write_topic("ble_thermometer", mac_address, value)
            .await;
    }
}

async fn handle_ble_contact_update(
    dht_manager: &mut DHTManager,
    mac_address: &str,
    message: &str,
    rssi: &i64,
    topic: &serde_json::Value,
) {
    if message.len() >= 58 {
        let value_of_topic = &topic["value"];
        let token = value_of_topic["token"].as_str().unwrap();

        let len_hex_value = "1d";
        let rssi_i = *rssi as i8;
        let rssi_hex = format!("{rssi_i:x}");

        let data = len_hex_value.to_owned() + message + &rssi_hex;

        let ret = bleutils::parse_contact_sensor(mac_address, &data, token);
        if let Ok(m) = ret {
            let val = u64::from(m.state != ContactStatus::Open);

            let val_in_topic = value_of_topic["status"].as_u64().unwrap();
            println!("val {val}, value_of_topic {val_in_topic}");
            if val != val_in_topic {
                let value = serde_json::json!({
                    "status": val,
                    "token": token
                });

                dht_manager
                    .write_topic("ble_contact", mac_address, value)
                    .await;
            }
        }
    }
}

async fn handle_ble_valve_update(
    dht_manager: &mut DHTManager,
    mac_address: &str,
    message: &String,
    _topic: &serde_json::Value,
) {
    let value = message == "1";

    let value = serde_json::json!({ "status": value });

    dht_manager
        .write_topic("ble_valve", mac_address, value)
        .await;
}
