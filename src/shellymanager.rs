use base64::encode;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use std::error::Error;
use std::time::SystemTime;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::{http, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub struct ShellyManager {
    pub ip: String,
    pub mac_address: String,
    pub url: String,
    pub write_shelly: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    pub read_shelly: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    pub last_pong_timestamp: std::time::SystemTime,
    pub last_action_timestamp: std::time::SystemTime,
    pub user_login: String,
    pub user_password: String,
}

impl ShellyManager {
    pub async fn connect_to_shelly(
        ip: &str,
        url: &str,
        user_login: &str,
        user_password: &str,
    ) -> Result<
        (
            SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
            SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        ),
        Box<dyn Error>,
    > {
        let _url_shelly = url::Url::parse(url).unwrap();
        let mut connect_attempts_counter = 0;

        let enc = encode(user_login.to_owned() + ":" + user_password);
        let header = format!("Basic {}", enc);

        loop {
            let ws_request = http::Request::builder()
                .method("GET")
                .header("Host", url.to_owned().clone())
                .header("Origin", url.to_owned().clone())
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Version", "13")
                .header(
                    "Sec-WebSocket-Key",
                    tokio_tungstenite::tungstenite::handshake::client::generate_key(),
                )
                .header("Authorization", header.clone())
                .uri(url)
                .body(())?;

            //let connector = native_tls::TlsConnector::builder().danger_accept_invalid_hostnames(true).build().unwrap();

            let connector = native_tls::TlsConnector::builder().build()?;

            println!("Perform TCP Connection");

            tokio::select! {

                shelly_tcp_stream = TcpStream::connect(ip.to_owned() + ":443") => {

                    if let Ok(shelly_tcp_stream) = shelly_tcp_stream {

                        tokio::select! {
                            ws_shelly_res = tokio_tungstenite::client_async_tls_with_config(
                                ws_request, shelly_tcp_stream, None,
                                Some(tokio_tungstenite::Connector::NativeTls(connector))) => {

                                if let Ok(ws_sh) = ws_shelly_res {
                                    let ws_shelly = ws_sh.0;
                                    let (write_shelly, read_shelly) = ws_shelly.split();
                                    println!("Return ok");
                                    return Ok((write_shelly, read_shelly));
                                   } else {
                                     println!("Increment connect_attempts counter");
                                     connect_attempts_counter += 1;
                                     //println!("{:?}", ws_shelly_res);
                                     if connect_attempts_counter == 2 {
                                        println!("Error while connecting");
                                        return Err("connect error".into());
                                     }
                                }
                            }
                            _ = tokio::time::sleep(Duration::from_millis(5000)) => {
                                println!("Connect to shelly timeout TLS");
                                connect_attempts_counter += 1;
                                if connect_attempts_counter == 2 {
                                    return Err("connect error".into());
                                }
                            }

                        }
                    }

                }

                _ = tokio::time::sleep(Duration::from_millis(3000)) => {
                       println!("Connect to shelly timeout");
                       connect_attempts_counter += 1;

                       if connect_attempts_counter == 2 {
                        return Err("connect error".into());
                       }
                }

            }

            println!("Exit from select");
        }
    }

    pub async fn new(
        ip: &str,
        topic_name: &str,
        mac_address: &String,
        mdns_name: &str,
        user_login: &str,
        user_password: &str,
    ) -> Result<ShellyManager, Box<dyn Error>> {
        let mac = mac_address.replace(':', "");
        let mac = mac.as_str();
        let url = "wss://".to_owned() + mdns_name + "/things/" + topic_name + "-" + mac;

        let (write_shelly, read_shelly) =
            ShellyManager::connect_to_shelly(ip, &url, user_login, user_password).await?;

        Ok(ShellyManager {
            ip: ip.to_owned(),
            mac_address: mac_address.to_owned(),
            url: url.to_owned(),
            write_shelly,
            read_shelly,
            last_pong_timestamp: SystemTime::now(),
            last_action_timestamp: SystemTime::UNIX_EPOCH,
            user_login: user_login.to_owned(),
            user_password: user_password.to_owned(),
        })
    }

    pub async fn reconnect(&mut self) -> Result<(), Box<dyn Error>> {
        let (write_shelly, read_shelly) = ShellyManager::connect_to_shelly(
            &self.ip,
            &self.url,
            &self.user_login,
            &self.user_password,
        )
        .await?;
        self.last_pong_timestamp = SystemTime::now();
        self.last_action_timestamp = SystemTime::UNIX_EPOCH;
        self.write_shelly = write_shelly;
        self.read_shelly = read_shelly;

        self.send_get_update().await;

        Ok(())
    }

    pub async fn send_ping(&mut self) {
        let _ret = self.write_shelly.send(Message::Ping(vec![])).await;
        //println!("ping message to shelly sent ");
    }

    pub async fn send_get_update(&mut self) {
        //println!("Requesting status update");
        let action_payload = serde_json::json!({});

        let action_payload_string = action_payload.to_string();

        let shelly_action = serde_json::json!({
            "shelly_action" : {
                "input" : {
                    "action": {
                        "action_name": "get_status_update",
                        "action_payload": action_payload_string
                    }
                }
            }
        });

        let message = serde_json::json!({
            "messageType": "requestAction",
            "data": shelly_action
        });

        self.send_action(&message).await;
    }

    pub async fn send_action(&mut self, message: &serde_json::Value) {
        /*if self.last_action_timestamp != SystemTime::UNIX_EPOCH {
            if self.last_action_timestamp.elapsed().unwrap().as_millis() < 300 {
                //printlnn!("Skipping action");
                return;
            }
        }*/

        //println!("{}", message);
        let _ret = self
            .write_shelly
            .send(Message::Text(message.to_string()))
            .await;
        //println!("action message to shelly sent ");
        self.last_action_timestamp = SystemTime::now();
    }

    pub async fn wait_for_shelly_message(&mut self) -> Result<serde_json::Value, Box<dyn Error>> {
        loop {
            let data = self.read_shelly.next().await;
            match data {
                Some(Ok(Message::Text(t))) => {
                    let message: serde_json::Value = serde_json::from_str(&t)?;
                    return Ok(message);
                }
                Some(Ok(Message::Pong(_t))) => {
                    //println!("Received Pong Message from shelly");
                    self.last_pong_timestamp = SystemTime::now();
                }
                Some(Ok(Message::Close(_t))) => {
                    let ret = self.mac_address.clone();
                    return Err(ret.into());
                }
                Some(Ok(Message::Ping(_t))) => {}
                Some(Err(_m)) => {
                    let ret = self.mac_address.clone();
                    return Err(ret.into());
                }
                _ => {}
            }
        }
    }
}
