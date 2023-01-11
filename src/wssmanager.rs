use axum::{extract::Extension, http, response::IntoResponse, routing::get, Router};

use axum_auth::AuthBasic;

use crate::messages::{AuthCredMessage, BleBeaconMessage, ESP32CommandMessage, ESP32CommandType};
use axum::extract::ws::Message;
use axum::extract::ws::WebSocketUpgrade;
use std::{net::SocketAddr, path::PathBuf};
use tokio::sync::mpsc::Sender;
use tokio::sync::{broadcast, mpsc, oneshot};
use tower_http::cors::{Any, CorsLayer};

use axum_server::tls_rustls::RustlsConfig;

fn parse_esp32_message(
    shelly_message: &serde_json::Value,
    updates_channel: &broadcast::Sender<BleBeaconMessage>,
) -> bool {
    if let Some(message_type) = shelly_message.get("messageType") {
        if message_type.as_str().unwrap() == "propertyStatus" {
            if let Some(data) = shelly_message.get("data") {
                if let Some(status) = data.get("status") {
                    let status_string = status.as_str().unwrap();
                    let status_result: serde_json::Value =
                        serde_json::from_str(status_string).unwrap();

                    if let Some(updated_properties) = status_result.get("updated_properties") {
                        let vec_prop = updated_properties.as_array().unwrap();
                        for prop in vec_prop {
                            let prop_str = prop.as_str().unwrap();
                            if prop_str == "beacon_adv" {
                                if let Some(beacon_adv) = status_result.get("beacon_adv") {
                                    let beacon_adv_string = beacon_adv.as_str().unwrap();

                                    let b = BleBeaconMessage::from(beacon_adv_string);
                                    let _ret = updates_channel.send(b);
                                    return true;
                                }
                            } else {
                                if prop_str == "valve_operation" {
                                    if let Some(valve_operation) =
                                        status_result.get("valve_operation")
                                    {
                                        let valve_operation_string =
                                            valve_operation.as_str().unwrap();

                                        let b = BleBeaconMessage::from(valve_operation_string);
                                        let _ret = updates_channel.send(b);
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

pub struct WssManager {
    //  listening port
    pub http_port: u16,
    pub channel_of_updates_tx: broadcast::Sender<BleBeaconMessage>,
    pub channel_of_updates_rx: broadcast::Receiver<BleBeaconMessage>,
    pub command_channel_tx: broadcast::Sender<ESP32CommandMessage>,
    pub command_channel_rx: broadcast::Receiver<ESP32CommandMessage>,
    pub channel_of_actuator_updates_tx: broadcast::Sender<serde_json::Value>,
    pub channel_of_actuator_updates_rx: broadcast::Receiver<serde_json::Value>,
    pub rx_auth_cred: mpsc::Receiver<AuthCredMessage>,
}

impl WssManager {
    pub async fn new(http_port: u16) -> WssManager {
        let addr = SocketAddr::from(([0, 0, 0, 0], http_port));

        let config = RustlsConfig::from_pem_file(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("data")
                .join("Cert.pem"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("data")
                .join("Key.pem"),
        )
        .await
        .unwrap();

        let (tx_auth_cred, rx_auth_cred) = mpsc::channel(32);

        let tx_auth_cred_copy = tx_auth_cred.clone();

        let (command_channel_tx, command_channel_rx) =
            broadcast::channel::<ESP32CommandMessage>(16);

        let command_channel_tx_copy = command_channel_tx.clone();

        let (channel_of_updates_tx, channel_of_updates_rx) =
            broadcast::channel::<BleBeaconMessage>(16);

        let channel_of_updates_tx_copy = channel_of_updates_tx.clone();

        let (channel_of_actuator_updates_tx, channel_of_actuator_updates_rx) =
            broadcast::channel::<serde_json::Value>(16);

        let channel_of_actuator_updates_tx_copy = channel_of_actuator_updates_tx.clone();

        let app = Router::new()
            .route(
                "/",
                get(WssManager::handle_websocket_req)
                    .layer(Extension(command_channel_tx_copy))
                    .layer(Extension(channel_of_updates_tx_copy))
                    .layer(Extension(channel_of_actuator_updates_tx_copy))
                    .layer(Extension(tx_auth_cred_copy)),
            )
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_headers([http::header::CONTENT_TYPE]),
            );

        tokio::spawn(async move {
            axum_server::bind_rustls(addr, config)
                .serve(app.into_make_service())
                .await
        });

        WssManager {
            http_port,
            channel_of_updates_tx,
            channel_of_updates_rx,
            command_channel_tx,
            command_channel_rx,
            channel_of_actuator_updates_tx,
            channel_of_actuator_updates_rx,
            rx_auth_cred,
        }
    }

    async fn handle_websocket_req(
        ws: WebSocketUpgrade,
        Extension(command_channel): Extension<broadcast::Sender<ESP32CommandMessage>>,
        Extension(updates_channel): Extension<broadcast::Sender<BleBeaconMessage>>,
        Extension(updates_actuator_channel): Extension<broadcast::Sender<serde_json::Value>>,
        Extension(tx_cred): Extension<Sender<AuthCredMessage>>,
        AuthBasic((user, password)): AuthBasic,
    ) -> impl IntoResponse {
        let mut command_receive_channel = command_channel.subscribe();

        ws.on_upgrade(|mut socket| async move {

            let mut esp32_mac_address = String::from("");

            let mut pass = String::from("");

            if let Some(password) = password {
                pass = password;
            }

            let (tx_resp, rx_resp) = oneshot::channel();

            let m = AuthCredMessage {
                user,
                pass,
                responder: tx_resp,
            };

            tx_cred.send(m).await.unwrap();

            let resp = rx_resp.await.unwrap();

            match resp {
                Ok(m) => {
                    if let Some(mac_address) = m.get("mac_address") {
                        esp32_mac_address = mac_address.as_str().unwrap().to_owned();
                    }
                },
                _=> {
                    return;
                }
            }

            loop {
                tokio::select! {
                        // received command from the dht
                        command = command_receive_channel.recv() => {
                            match command {
                                Ok(cmd) => {
                                    match cmd.command_type {
                                        ESP32CommandType::ValveCommand => {
                                            println!("Received valve command");
                                            if let Some(shelly_action_payload) = cmd.payload.get("shelly_action") {
                                                        let shelly_action = serde_json::json!({ "shelly_action": shelly_action_payload });

                                                        let message = serde_json::json!({
                                                            "messageType": "requestAction",
                                                            "data": shelly_action
                                                        });
                                                        let m = Message::Text(serde_json::to_string(&message).unwrap());
                                                        let _ret = socket.send(m).await;

                                            }
                                        }
                                        ESP32CommandType::ActuatorCommand => {
                                            println!("Received Actuator command");
                                            if cmd.mac_address == esp32_mac_address {
                                                if let Some(shelly_action_payload) = cmd.payload.get("shelly_action") {
                                                            let shelly_action = serde_json::json!({ "shelly_action": shelly_action_payload });

                                                            let message = serde_json::json!({
                                                                "messageType": "requestAction",
                                                                "data": shelly_action
                                                            });
                                                            let m = Message::Text(serde_json::to_string(&message).unwrap());
                                                            let _ret = socket.send(m).await;
                                                }
                                            }
                                        }
                                    }
                                },
                                _=> {}
                            }

                        }
                        // received message from an esp32
                        Some(msg) = socket.recv() => {

                            if let Err(_e) = msg {
                                return;
                            }

                            match msg.unwrap() {
                                Message::Text(message) => {
                                    println!("Received {message}");
                                    let shelly_message: serde_json::Value = serde_json::from_str(&message).unwrap();

                                    /*
                                    if(esp32_mac_address == ""){
                                       esp32_mac_address = get_mac_from_message(&shelly_message);
                                    }
                                     */

                                    if !parse_esp32_message(&shelly_message, &updates_channel) {
                                        let _ret = updates_actuator_channel.send(shelly_message);
                                    }
                                }
                                Message::Close(_) => {
                                    return;
                                }
                                _ => {}
                            }
                        }
                }
            }
        })
    }
}
