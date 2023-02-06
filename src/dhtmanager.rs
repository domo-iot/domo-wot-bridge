use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::error::Error;
use std::time;
use std::time::SystemTime;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

pub enum DHTCommand {
    ActuatorCommand(serde_json::Value),
    ValveCommand(serde_json::Value),
}

pub struct TopicEntry {
    topic: serde_json::Value,
    last_update_timestamp: std::time::SystemTime,
}

pub struct DHTManager {
    pub url: String,
    pub write_channel: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    pub read_channel: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    pub topic_cache: HashMap<String, TopicEntry>,
    pub last_pong_timestamp: std::time::SystemTime,
}

impl DHTManager {
    pub async fn connect_to_dht_manager(
        url: &str,
    ) -> (
        SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
        SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    ) {
        let url_dht_manager = url::Url::parse(url).unwrap();

        let ws_dht;

        loop {
            let ws_dht_res = connect_async(url_dht_manager.clone()).await;

            if let Ok(ws_d) = ws_dht_res {
                ws_dht = ws_d.0;
                break;
            }

            tokio::time::sleep(Duration::from_secs(10)).await;
            println!("Reconnect to dht manager");
        }

        let (write_channel, read_channel) = ws_dht.split();

        (write_channel, read_channel)
    }

    pub async fn new(url: &str) -> DHTManager {
        let (write_channel, read_channel) = DHTManager::connect_to_dht_manager(url).await;

        let topic_cache = HashMap::<String, TopicEntry>::new();

        DHTManager {
            url: url.to_owned(),
            write_channel,
            read_channel,
            topic_cache,
            last_pong_timestamp: SystemTime::now(),
        }
    }

    pub async fn reconnect(&mut self) {
        let (write_channel, read_channel) = DHTManager::connect_to_dht_manager(&self.url).await;
        self.last_pong_timestamp = SystemTime::now();
        self.write_channel = write_channel;
        self.read_channel = read_channel;
    }

    pub async fn get_auth_cred(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        let shelly_1plus_topics = reqwest::get("http://localhost:3000/topic_name/shelly_1plus")
            .await?
            .json::<serde_json::Value>()
            .await?;

        let topics = shelly_1plus_topics.as_array().unwrap();
        for t in topics.iter() {
            if let Some(value) = t.get("value") {
                if let Some(user_login) = value.get("user_login") {
                    if let Some(user_password) = value.get("user_password") {
                        let user_login_str = user_login.as_str().unwrap();
                        let user_password_str = user_password.as_str().unwrap();
                        if user_login_str == user && user_password_str == password {
                            let mac = t.get("topic_uuid").unwrap().as_str().unwrap();
                            let json_ret = serde_json::json!({ "mac_address": mac });
                            return Ok(json_ret);
                        }
                    }
                }
            }
        }

        Err("cred not found".into())
    }

    pub async fn get_topic_with_http(
        &mut self,
        mac_address_req: &str,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        let topics = reqwest::get("http://localhost:3000/get_all")
            .await?
            .json::<serde_json::Value>()
            .await?;

        let topics = topics.as_array().unwrap();
        for topic in topics.iter() {
            if let Some(topic_name) = topic.get("topic_name") {
                let topic_name_str = topic_name.as_str().unwrap();
                if [
                    "shelly_1",
                    "shelly_1pm",
                    "shelly_25",
                    "shelly_dimmer",
                    "shelly_em",
                    "shelly_rgbw",
                    "shelly_1plus",
                    "geeklink_ir",
                    "ble_contact",
                    "ble_thermometer",
                    "ble_valve",
                ]
                .contains(&topic_name_str)
                {
                    if let Some(mac_address) = topic.get("topic_uuid") {
                        let mac_address_str = mac_address.as_str().unwrap();
                        if mac_address_str == mac_address_req {
                            let topic_owned = topic.to_owned();
                            let topic_entry = TopicEntry {
                                topic: topic_owned.clone(),
                                last_update_timestamp: SystemTime::now(),
                            };
                            self.topic_cache
                                .insert(mac_address_str.to_string(), topic_entry);
                            println!("Insert in topic_cache {topic_owned}");
                            return Ok(topic_owned);
                        }
                    }
                }
            }
        }

        Err("mac_address not found".into())
    }

    pub async fn get_topic(
        &mut self,
        mac_address_req: &str,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        if self.topic_cache.contains_key(mac_address_req) {
            return match self.topic_cache.get(mac_address_req) {
                Some(topic_entry) => {
                    if topic_entry
                        .last_update_timestamp
                        .elapsed()
                        .unwrap()
                        .as_millis()
                        < 150
                    {
                        println!("Avoiding http call");
                        Ok(topic_entry.topic.clone())
                    } else {
                        self.get_topic_with_http(mac_address_req).await
                    }
                }
                None => self.get_topic_with_http(mac_address_req).await,
            };
        } else {
            return self.get_topic_with_http(mac_address_req).await;
        }
    }

    pub async fn send_ping(&mut self) {
        let _ret = self.write_channel.send(Message::Ping(vec![])).await;
        println!("ping message to domo-dht-manager sent ");
    }

    pub async fn write_topic(
        &mut self,
        topic_name: &str,
        topic_uuid: &str,
        value: serde_json::Value,
    ) {
        let message = serde_json::json!({
        "RequestPostTopicUUID": {
                "topic_name": topic_name,
                "topic_uuid": topic_uuid,
                "value": value
            }
        });

        let _ret = self
            .write_channel
            .send(Message::Text(message.to_string()))
            .await;

        println!("Sent message: {message} ");
    }

    fn handle_volatile_command(
        &self,
        command: serde_json::Value,
    ) -> Result<DHTCommand, Box<dyn Error>> {
        if let Some(value) = command.get("value") {
            if let Some(command) = value.get("command") {
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
                }
            }
        }

        Err("not able to parse message".into())
    }

    pub async fn wait_dht_messages(&mut self) -> Result<DHTCommand, Box<dyn Error>> {
        let data = self.read_channel.next().await;
        //println!("Received something");
        match data {
            Some(Ok(Message::Text(t))) => {
                //println!("Received {}", t);
                let message: serde_json::Value = serde_json::from_str(&t)?;

                if let Some(volatile) = message.get("Volatile") {
                    return self.handle_volatile_command(volatile.to_owned());
                }
            }
            Some(Ok(Message::Pong(_t))) => {
                println!("Received Pong Message from domo-dht-manager");
                self.last_pong_timestamp = time::SystemTime::now();
            }
            _ => {}
        }

        Err("not able to parse message".into())
    }
}
